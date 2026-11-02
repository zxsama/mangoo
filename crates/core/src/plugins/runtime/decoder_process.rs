//! Accountless activation of the versioned file-decoder capability.
use std::collections::{BTreeMap, HashMap};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::io::AsyncReadExt;
use tokio::time::Instant;

use super::frame::FrameLimits;
use super::process::{ChildProcess, LaunchSpec};
use super::rpc::FramedSession;
use super::supervisor::RpcLimits;
use super::trust::{verify_bundled_entry, TrustEntry};
use crate::plugins::decoder::{
    DecoderError, DecoderPublicationGuard, DecoderPurpose, CHUNK_BYTES, MAX_HEADER_BYTES,
    MAX_INPUT_BYTES, MAX_OUTPUT_BYTES, MAX_OUTPUT_PIXELS, MAX_SOURCE_PIXELS,
};
use crate::plugins::manifest::{Capability, ManifestRecord, Permission, Platform};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecoderHello {
    nonce: [u8; 32],
    wire_versions: Vec<u16>,
    plugin_id: String,
    version: String,
    capabilities: Vec<String>,
    granted_permissions: Vec<Permission>,
    method_versions: BTreeMap<String, u16>,
    limits: RpcLimits,
}
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DecoderHelloAck {
    nonce: [u8; 32],
    wire_version: u16,
    plugin_id: String,
    version: String,
    capabilities: Vec<String>,
    method_versions: BTreeMap<String, u16>,
    limits: RpcLimits,
}
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RasterResponse {
    job_id: String,
    stream_id: u64,
    width: u32,
    height: u32,
    format: String,
    size: u64,
    sha256: String,
}

pub struct DecoderProcess {
    session: FramedSession,
    job_cancelled: AtomicBool,
    child: tokio::sync::Mutex<Option<ChildProcess>>,
    stderr: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl DecoderProcess {
    pub async fn activate(
        manifest: &ManifestRecord,
        bundled_root: &Path,
        platform: Platform,
        trust: &[TrustEntry],
        runtime_directory: PathBuf,
        granted_permissions: Vec<Permission>,
    ) -> anyhow::Result<Arc<Self>> {
        anyhow::ensure!(
            manifest.rpc.capabilities == [Capability::FileDecoderV1]
                && manifest.rpc.wire_versions.contains(&1),
            "decoder capability mismatch"
        );
        let executable = verify_bundled_entry(manifest, bundled_root, platform, trust)
            .map_err(|_| anyhow::anyhow!("decoder trust verification failed"))?;
        let safe_env = HashMap::from([(
            "MANGOO_DECODER_TEMP_DIR".to_owned(),
            runtime_directory.to_string_lossy().into_owned(),
        )]);
        let mut child = ChildProcess::spawn_verified_with_memory_limit(
            &executable,
            &LaunchSpec {
                runtime_dir: runtime_directory.clone(),
                safe_env,
            },
            Some(256 * 1024 * 1024),
        )
        .await?;
        let (stdin, stdout, mut stderr) = match child.take_stdio() {
            Ok(value) => value,
            Err(error) => {
                child.terminate_tree().await?;
                child.wait().await?;
                return Err(error.into());
            }
        };
        let limits = FrameLimits {
            max_control: MAX_HEADER_BYTES as usize,
            max_binary: CHUNK_BYTES,
            max_buffered: CHUNK_BYTES * 2,
            stream_window: CHUNK_BYTES * 4,
            max_object: MAX_INPUT_BYTES,
        };
        let session = FramedSession::start_with_handler(stdout, stdin, limits.clone(), 1, None);
        session.restrict_decoder_output(runtime_directory);
        // Bounded drain: never accumulate arbitrary stderr lines/content.
        let stderr = tokio::spawn(async move {
            let mut buffer = [0u8; 4096];
            while stderr.read(&mut buffer).await.is_ok_and(|size| size > 0) {}
        });
        let process = Arc::new(Self {
            session,
            job_cancelled: AtomicBool::new(false),
            child: tokio::sync::Mutex::new(Some(child)),
            stderr: tokio::sync::Mutex::new(Some(stderr)),
        });
        let mut nonce = [0; 32];
        rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
        let hello = DecoderHello {
            nonce,
            wire_versions: vec![1],
            plugin_id: manifest.id.clone(),
            version: manifest.version.clone(),
            capabilities: vec![crate::plugins::FILE_DECODER_V1_CAPABILITY.to_owned()],
            granted_permissions,
            method_versions: manifest.rpc.method_versions.clone(),
            limits: RpcLimits {
                max_control_bytes: limits.max_control as u64,
                max_binary_bytes: limits.max_binary as u64,
                max_buffered_bytes: limits.max_buffered as u64,
                stream_window_bytes: limits.stream_window as u64,
                max_concurrent_requests: 1,
                max_concurrent_streams: 2,
                max_object_bytes: limits.max_object,
            },
        };
        let result = async {
            let ack: DecoderHelloAck = serde_json::from_value(
                process
                    .session
                    .call(
                        "runtime.hello",
                        serde_json::to_value(&hello)?,
                        Duration::from_secs(5),
                        None,
                        None,
                    )
                    .await
                    .map_err(DecoderError::from)?,
            )?;
            anyhow::ensure!(
                ack.nonce == hello.nonce
                    && ack.wire_version == 1
                    && ack.plugin_id == hello.plugin_id
                    && ack.version == hello.version
                    && ack.capabilities == hello.capabilities
                    && ack.method_versions == hello.method_versions
                    && ack.limits == hello.limits,
                "decoder hello mismatch"
            );
            process.session.complete_handshake();
            Ok::<(), anyhow::Error>(())
        }
        .await;
        if let Err(error) = result {
            process.shutdown().await?;
            return Err(error);
        }
        Ok(process)
    }

    pub fn close_admission(&self) {
        self.session.close_admission();
    }
    pub fn is_closed(&self) -> bool {
        self.session.activity().is_retired()
    }
    /// A cancelled dispatched job cannot leave a reusable wire session, but is
    /// not a plugin crash. Publish the reason before closing admission so the
    /// supervisor can join this process and restore the unchanged capability.
    pub fn mark_job_cancelled(&self) {
        self.job_cancelled.store(true, Ordering::Release);
        self.session.close_admission();
    }
    pub fn was_job_cancelled(&self) -> bool {
        self.job_cancelled.load(Ordering::Acquire)
    }
    #[cfg(test)]
    pub(crate) fn job_dispatched_for_test(&self) -> bool {
        self.session.decoder_job_dispatched_for_test()
    }
    pub async fn shutdown(&self) -> anyhow::Result<()> {
        self.session.close_admission();
        // Keep ownership on a cleanup error so a later join can retry.
        let mut child = self.child.lock().await;
        if let Some(child) = child.as_mut() {
            child.terminate_tree().await?;
            child.wait().await?;
        }
        child.take();
        self.session.join_closed().await;
        if let Some(stderr) = self.stderr.lock().await.take() {
            let _ = stderr.await;
        }
        Ok(())
    }

    pub async fn decode(
        &self,
        input: File,
        size: u64,
        sha256: [u8; 32],
        purpose: DecoderPurpose,
        maximum_edge: u32,
        deadline: Instant,
        ownership: DecoderPublicationGuard,
        budget: Arc<tokio::sync::OwnedSemaphorePermit>,
    ) -> Result<(Vec<u8>, u32, u32), DecoderError> {
        let job_id = uuid::Uuid::now_v7().to_string();
        let (response, mut operation) = self
            .session
            .call_with_input_file(
                serde_json::json!({
                    "jobId": job_id, "purpose": purpose, "size": size, "sha256": hex(&sha256),
                    "maxEdge": maximum_edge, "maxOutputPixels": MAX_OUTPUT_PIXELS,
                    "maxSourcePixels": MAX_SOURCE_PIXELS, "maxHeaderBytes": MAX_HEADER_BYTES,
                }),
                tokio::fs::File::from_std(input),
                size,
                sha256,
                MAX_OUTPUT_BYTES,
                deadline,
            )
            .await?;
        let response: RasterResponse =
            serde_json::from_value(response).map_err(|_| DecoderError::Protocol)?;
        let expected_size = u64::from(response.width)
            .checked_mul(u64::from(response.height))
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or(DecoderError::ResourceLimit)?;
        if response.job_id != job_id
            || response.format != "rgba8"
            || response.stream_id == 0
            || response.stream_id % 2 != 0
            || response.width == 0
            || response.height == 0
            || response.width > maximum_edge
            || response.height > maximum_edge
            || response.size != expected_size
            || expected_size > MAX_OUTPUT_BYTES
        {
            return Err(DecoderError::Protocol);
        }
        let mut output = self
            .session
            .receive_decoder_file(&operation, response.stream_id)
            .await?;
        let rgba = tokio::task::spawn_blocking(move || {
            let _ownership = ownership;
            let _budget = budget;
            if output.metadata().map_err(|_| DecoderError::Io)?.len() != expected_size {
                return Err(DecoderError::Protocol);
            }
            output
                .seek(SeekFrom::Start(0))
                .map_err(|_| DecoderError::Io)?;
            let mut bytes = vec![0u8; expected_size as usize];
            output
                .read_exact(&mut bytes)
                .map_err(|_| DecoderError::Io)?;
            if hex(&Sha256::digest(&bytes)) != response.sha256 {
                return Err(DecoderError::Protocol);
            }
            Ok(bytes)
        })
        .await
        .map_err(|_| DecoderError::Io)??;
        if Instant::now() >= deadline {
            return Err(DecoderError::Timeout);
        }
        operation.complete()?;
        Ok((rgba, response.width, response.height))
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}
