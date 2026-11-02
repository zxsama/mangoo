//! Actual child-pipe coverage of Host decoder transport. The private Node peer
//! injects faults; trusted artifact activation is covered by decoder.rs tests.
use super::*;
use std::io::{Read, Seek, SeekFrom};
use std::process::Stdio;
use tokio::process::{Child, Command};

struct DecoderPeer {
    session: FramedSession,
    child: Child,
    directory: tempfile::TempDir,
}

impl DecoderPeer {
    fn start(mode: &str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let mut command = Command::new("node");
        command
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/src/plugins/runtime/decoder_rpc_peer.cjs"
            ))
            .arg(mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);
        #[cfg(windows)]
        command.creation_flags(0x08000000);
        let mut child = command
            .spawn()
            .expect("Node is a workspace test prerequisite");
        let session = FramedSession::start(
            child.stdout.take().unwrap(),
            child.stdin.take().unwrap(),
            FrameLimits {
                max_control: 65536,
                max_binary: 256 * 1024,
                max_buffered: 512 * 1024,
                stream_window: 1024 * 1024,
                max_object: 64 * 1024 * 1024,
            },
            1,
        );
        session.restrict_decoder_output(directory.path().to_owned());
        Self {
            session,
            child,
            directory,
        }
    }

    async fn call(
        &self,
        size: usize,
        timeout: Duration,
    ) -> Result<(Value, OperationGuard), RpcError> {
        let mut input = tempfile::tempfile().unwrap();
        // Generate a large input without creating a matching whole-input Vec.
        let chunk = [0x5au8; 8192];
        let mut hash = Sha256::new();
        let mut remaining = size;
        while remaining > 0 {
            let bytes = &chunk[..remaining.min(chunk.len())];
            input.write_all(bytes).unwrap();
            hash.update(bytes);
            remaining -= bytes.len();
        }
        input.seek(SeekFrom::Start(0)).unwrap();
        self.session
            .call_with_input_file(
                serde_json::json!({"jobId":"test-job"}),
                tokio::fs::File::from_std(input),
                size as u64,
                hash.finalize().into(),
                4,
                Instant::now() + timeout,
            )
            .await
    }

    async fn release(&self) {
        self.session
            .tx
            .send(Frame {
                kind: FrameKind::Ping,
                stream_id: 0,
                sequence: 0,
                payload: vec![],
            })
            .await
            .unwrap();
    }

    async fn join(mut self) {
        self.session.close_admission();
        tokio::time::timeout(Duration::from_secs(3), self.session.join_closed())
            .await
            .unwrap();
        let status = tokio::time::timeout(Duration::from_secs(3), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success(), "fault peer must finish its assertions");
        let incoming = self.session.streams.incoming_data.lock().unwrap();
        assert!(incoming.open.is_empty() && incoming.completed_files.is_empty());
        assert_eq!(std::fs::read_dir(self.directory.path()).unwrap().count(), 0);
    }
}

#[tokio::test]
async fn decoder_rpc_child_large_file_waits_for_credit_and_complete_output() {
    let peer = DecoderPeer::start("valid");
    let (response, mut owner) = peer
        .call(3 * 1024 * 1024, Duration::from_secs(5))
        .await
        .unwrap();
    assert_eq!(response["received"], 3 * 1024 * 1024);
    assert!(response["largest"].as_u64().unwrap() <= 64 * 1024);
    // A final control response cannot release a session with unfinished output.
    assert_eq!(
        peer.session
            .call("health", Value::Null, Duration::from_millis(20), None, None)
            .await,
        Err(RpcError::AdmissionDeadlineExceeded)
    );
    assert!(!peer.session.activity().is_retired());
    peer.release().await;
    let mut file = peer.session.receive_decoder_file(&owner, 2).await.unwrap();
    file.seek(SeekFrom::Start(0)).unwrap();
    let mut pixel = [0; 4];
    file.read_exact(&mut pixel).unwrap();
    assert_eq!(pixel, [17, 83, 211, 255]);
    drop(file);
    owner.complete().unwrap();
    drop(owner);
    peer.join().await;
}

#[tokio::test]
async fn decoder_rpc_child_bad_output_hash_closes_and_cleans() {
    let peer = DecoderPeer::start("bad-hash");
    let (_, owner) = peer.call(32, Duration::from_secs(3)).await.unwrap();
    peer.release().await;
    assert!(matches!(
        peer.session.receive_decoder_file(&owner, 2).await,
        Err(RpcError::Closed)
    ));
    assert!(peer.session.activity().is_retired());
    drop(owner);
    peer.join().await;
}

#[tokio::test]
async fn decoder_rpc_child_extra_output_rejected_before_second_credit() {
    let peer = DecoderPeer::start("extra-output");
    let (_, owner) = peer.call(32, Duration::from_secs(3)).await.unwrap();
    peer.release().await;
    assert!(peer.session.receive_decoder_file(&owner, 2).await.is_err());
    assert!(peer.session.activity().is_retired());
    drop(owner);
    peer.join().await;
}

#[tokio::test]
async fn decoder_rpc_child_response_without_end_keeps_original_deadline() {
    let peer = DecoderPeer::start("late-end");
    let (_, owner) = peer.call(32, Duration::from_secs(1)).await.unwrap();
    peer.release().await;
    assert!(matches!(
        peer.session.receive_decoder_file(&owner, 2).await,
        Err(RpcError::DeadlineExceeded)
    ));
    drop(owner);
    assert!(peer.session.activity().is_retired());
    peer.join().await;
}

#[tokio::test]
async fn decoder_rpc_child_owner_drop_closes_unfinished_output() {
    let peer = DecoderPeer::start("late-end");
    let (_, owner) = peer.call(32, Duration::from_secs(3)).await.unwrap();
    drop(owner);
    assert!(peer.session.activity().is_retired());
    peer.join().await;
}

#[tokio::test]
async fn decoder_rpc_child_early_response_does_not_bypass_input_credit_deadline() {
    let peer = DecoderPeer::start("no-credit");
    assert!(matches!(
        peer.call(1024 * 1024 + 1, Duration::from_secs(1)).await,
        Err(RpcError::DeadlineExceeded)
    ));
    assert!(peer.session.activity().is_retired());
    peer.join().await;
}
