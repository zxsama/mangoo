use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};

use super::frame::FrameLimits;
use super::process::{ChildProcess, LaunchSpec};
use super::provider_adapter::FramedSyncProvider;
use super::rpc::{redact_stderr_jsonl, FramedSession};
use super::trust::{verify_bundled_entry, TrustEntry};
use crate::plugins::manifest::{ManifestRecord, Platform};
use crate::sync::provider::{ProviderManager, SyncProvider};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuntimeState {
    Disabled,
    PermissionRequired,
    Starting,
    Ready,
    Degraded,
    CrashedBackoff,
    Incompatible,
    ConfigurationRequired,
}
#[derive(Debug, Clone)]
pub struct BackoffPolicy {
    pub steps: Vec<Duration>,
    pub breaker_limit: usize,
    pub breaker_window: Duration,
}
impl Default for BackoffPolicy {
    fn default() -> Self {
        Self {
            steps: vec![
                Duration::from_secs(1),
                Duration::from_secs(2),
                Duration::from_secs(4),
                Duration::from_secs(8),
                Duration::from_secs(16),
                Duration::from_secs(30),
            ],
            breaker_limit: 5,
            breaker_window: Duration::from_secs(600),
        }
    }
}
pub struct Supervisor {
    state: RuntimeState,
    policy: BackoffPolicy,
    crashes: VecDeque<Instant>,
    backoff_until: Option<Instant>,
    breaker_open: bool,
    consecutive_health_failures: usize,
}
impl Supervisor {
    pub fn new(policy: BackoffPolicy) -> Self {
        Self {
            state: RuntimeState::Disabled,
            policy,
            crashes: VecDeque::new(),
            backoff_until: None,
            breaker_open: false,
            consecutive_health_failures: 0,
        }
    }
    pub fn state(&self) -> RuntimeState {
        self.state
    }
    pub fn begin_start(&mut self, now: Instant) -> bool {
        if self.breaker_open || self.backoff_until.is_some_and(|v| v > now) {
            return false;
        }
        self.state = RuntimeState::Starting;
        true
    }
    pub fn ready(&mut self) {
        self.state = RuntimeState::Ready;
        self.backoff_until = None;
        self.consecutive_health_failures = 0;
    }
    pub fn health_result(&mut self, healthy: bool, degraded_threshold: usize) -> bool {
        if healthy {
            self.consecutive_health_failures = 0;
            self.state = RuntimeState::Ready;
            return false;
        }
        self.consecutive_health_failures = self.consecutive_health_failures.saturating_add(1);
        if self.consecutive_health_failures >= degraded_threshold.max(1) {
            self.state = RuntimeState::Degraded;
            return true;
        }
        false
    }
    pub fn abnormal_exit(&mut self, now: Instant) {
        while self
            .crashes
            .front()
            .is_some_and(|v| now.duration_since(*v) > self.policy.breaker_window)
        {
            self.crashes.pop_front();
        }
        self.crashes.push_back(now);
        if self.crashes.len() >= self.policy.breaker_limit {
            self.breaker_open = true;
        }
        let index = self
            .crashes
            .len()
            .saturating_sub(1)
            .min(self.policy.steps.len() - 1);
        self.backoff_until = Some(now + self.policy.steps[index]);
        self.state = RuntimeState::CrashedBackoff;
    }
    pub fn manual_retry(&mut self) {
        self.breaker_open = false;
        self.crashes.clear();
        self.backoff_until = None;
        self.state = RuntimeState::Disabled;
        self.consecutive_health_failures = 0;
    }

    pub fn restart_delay(&self, now: Instant) -> Option<Duration> {
        self.backoff_until
            .map(|until| until.saturating_duration_since(now))
    }

    pub fn breaker_open(&self) -> bool {
        self.breaker_open
    }
}

#[derive(Debug, Clone)]
pub struct HealthPolicy {
    pub interval: Duration,
    pub deadline: Duration,
    pub degraded_threshold: usize,
    pub probation_successes: usize,
}
impl Default for HealthPolicy {
    fn default() -> Self {
        Self {
            interval: Duration::from_secs(30),
            deadline: Duration::from_secs(5),
            degraded_threshold: 3,
            probation_successes: 3,
        }
    }
}

/// Runs one Provider account until cancellation. Activation is supplied as a
/// factory so every failure performs a full trust check, spawn and handshake;
/// no stale child/session can be reused across a restart.
pub async fn supervise_until_cancelled<F, Fut>(
    registry: Arc<crate::host_registry::HostRegistry>,
    manager: ProviderManager,
    plugin_id: String,
    account_uuid: String,
    launch_fingerprint: String,
    cancellation: tokio_util::sync::CancellationToken,
    backoff: BackoffPolicy,
    health_policy: HealthPolicy,
    mut activate: F,
) -> anyhow::Result<()>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<ActivatedProvider>>,
{
    let mut state = Supervisor::new(backoff);
    loop {
        if cancellation.is_cancelled() {
            return Ok(());
        }
        let now = Instant::now();
        if !state.begin_start(now) {
            if state.breaker_open() {
                if !registry.record_plugin_runtime_failure(
                    &plugin_id,
                    &launch_fingerprint,
                    "crashed-backoff",
                    "circuit-breaker-open",
                    "automatic Provider restart breaker is open",
                    None,
                )? {
                    return Ok(());
                }
                loop {
                    tokio::select! {
                        _ = cancellation.cancelled() => return Ok(()),
                        _ = tokio::time::sleep(Duration::from_secs(1)) => {
                            if registry.plugin_runtime_retry_requested(&plugin_id)? {
                                state.manual_retry();
                                break;
                            }
                        }
                    }
                }
                continue;
            }
            let delay = state
                .restart_delay(now)
                .unwrap_or(Duration::from_millis(50));
            tokio::select! {
                _ = cancellation.cancelled() => return Ok(()),
                _ = tokio::time::sleep(delay) => continue,
            }
        }
        let activated = match activate().await {
            Ok(runtime) => runtime,
            Err(error) => {
                state.abnormal_exit(Instant::now());
                let until = state.restart_delay(Instant::now()).map(|delay| {
                    crate::host_registry::unix_timestamp_millis_for_runtime()
                        .saturating_add(delay.as_millis() as i64)
                });
                if !registry.record_plugin_runtime_failure(
                    &plugin_id,
                    &launch_fingerprint,
                    "crashed-backoff",
                    "activation-failed",
                    &error.to_string(),
                    until,
                )? {
                    return Ok(());
                }
                continue;
            }
        };
        state.ready();
        let session_id = uuid::Uuid::now_v7().to_string();
        let description = match activated.provider.describe().await {
            Ok(description) => description,
            Err(error) => {
                let detail = error.detail.clone();
                let _ = activated.shutdown().await;
                manager.unregister(&plugin_id, &account_uuid)?;
                state.abnormal_exit(Instant::now());
                if !registry.record_plugin_runtime_failure(
                    &plugin_id,
                    &launch_fingerprint,
                    "crashed-backoff",
                    "describe-failed",
                    &detail,
                    state.restart_delay(Instant::now()).map(|delay| {
                        crate::host_registry::unix_timestamp_millis_for_runtime()
                            .saturating_add(delay.as_millis() as i64)
                    }),
                )? {
                    return Ok(());
                }
                continue;
            }
        };
        if !registry.record_plugin_runtime_ready_for_launch(
            &plugin_id,
            &launch_fingerprint,
            &session_id,
            &description.capabilities,
        )? {
            activated.shutdown().await?;
            manager.unregister(&plugin_id, &account_uuid)?;
            return Ok(());
        }
        let mut healthy_successes = 0usize;
        let failure = loop {
            tokio::select! {
                _ = cancellation.cancelled() => {
                    activated.shutdown().await?;
                    manager.unregister(&plugin_id, &account_uuid)?;
                    return Ok(());
                }
                _ = tokio::time::sleep(health_policy.interval) => {
                    if !registry.plugin_runtime_launch_is_current(&plugin_id, &launch_fingerprint)? {
                        activated.shutdown().await?;
                        manager.unregister(&plugin_id, &account_uuid)?;
                        return Ok(());
                    }
                    let healthy = matches!(
                        tokio::time::timeout(health_policy.deadline, activated.provider.health()).await,
                        Ok(Ok(crate::sync::provider::ProviderHealth { ready: true, .. }))
                    );
                    if healthy {
                        healthy_successes = healthy_successes.saturating_add(1);
                        state.health_result(true, health_policy.degraded_threshold);
                        if healthy_successes >= health_policy.probation_successes
                            && registry.plugin_has_pending_probation(&plugin_id)?
                        {
                            if !registry.commit_plugin_probation_for_launch(
                                &plugin_id,
                                &launch_fingerprint,
                            )? {
                                activated.shutdown().await?;
                                manager.unregister(&plugin_id, &account_uuid)?;
                                return Ok(());
                            }
                        }
                    } else if state.health_result(false, health_policy.degraded_threshold) {
                        break "health-degraded";
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(1)) => {
                    if !registry.plugin_runtime_launch_is_current(&plugin_id, &launch_fingerprint)? {
                        activated.shutdown().await?;
                        manager.unregister(&plugin_id, &account_uuid)?;
                        return Ok(());
                    }
                    if activated.child_exited().await? { break "child-exited"; }
                }
            }
        };
        if registry.plugin_has_pending_probation(&plugin_id)? {
            if registry
                .rollback_plugin_probation_for_launch(&plugin_id, &launch_fingerprint, failure)?
                .is_none()
            {
                activated.shutdown().await?;
                manager.unregister(&plugin_id, &account_uuid)?;
                return Ok(());
            }
        }
        activated.shutdown().await?;
        manager.unregister(&plugin_id, &account_uuid)?;
        state.abnormal_exit(Instant::now());
        let detail = if failure == "child-exited" {
            "Provider process exited unexpectedly"
        } else {
            "Provider failed consecutive health checks"
        };
        let until = state.restart_delay(Instant::now()).map(|delay| {
            crate::host_registry::unix_timestamp_millis_for_runtime()
                .saturating_add(delay.as_millis() as i64)
        });
        if !registry.record_plugin_runtime_failure(
            &plugin_id,
            &launch_fingerprint,
            "crashed-backoff",
            failure,
            detail,
            until,
        )? {
            return Ok(());
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub nonce: [u8; 32],
    pub wire_versions: Vec<u16>,
    pub plugin_id: String,
    pub version: String,
    pub account_uuid: String,
    pub capabilities: Vec<String>,
    pub granted_permissions: Vec<crate::plugins::manifest::Permission>,
    pub method_versions: std::collections::BTreeMap<String, u16>,
    pub limits: RpcLimits,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HelloAck {
    pub nonce: [u8; 32],
    pub wire_version: u16,
    pub plugin_id: String,
    pub version: String,
    pub account_uuid: String,
    pub capabilities: Vec<String>,
    pub method_versions: std::collections::BTreeMap<String, u16>,
    pub limits: RpcLimits,
}
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RpcLimits {
    pub max_control_bytes: u64,
    pub max_binary_bytes: u64,
    pub max_buffered_bytes: u64,
    pub stream_window_bytes: u64,
    pub max_concurrent_requests: u32,
    pub max_concurrent_streams: u32,
    pub max_object_bytes: u64,
}
impl RpcLimits {
    fn host_default() -> Self {
        let limits = FrameLimits::default();
        Self {
            max_control_bytes: limits.max_control as u64,
            max_binary_bytes: limits.max_binary as u64,
            max_buffered_bytes: limits.max_buffered as u64,
            stream_window_bytes: limits.stream_window as u64,
            max_concurrent_requests: 64,
            max_concurrent_streams: 32,
            max_object_bytes: limits.max_object,
        }
    }
}

pub fn verify_hello(h: &Hello, a: &HelloAck) -> Result<u16, &'static str> {
    if h.nonce != a.nonce {
        return Err("nonce mismatch");
    }
    if h.plugin_id != a.plugin_id || h.version != a.version || h.account_uuid != a.account_uuid {
        return Err("identity mismatch");
    }
    if !h.wire_versions.contains(&a.wire_version) {
        return Err("wire mismatch");
    }
    if a.capabilities != h.capabilities {
        return Err("capability mismatch");
    }
    if a.method_versions != h.method_versions {
        return Err("method version mismatch");
    }
    if a.limits != h.limits {
        return Err("limit mismatch");
    }
    Ok(a.wire_version)
}

/// Owns the child for as long as the registered provider is usable. Dropping it
/// closes the Windows Job Object / Tokio kill-on-drop guard.
pub struct ActivatedProvider {
    pub provider: Arc<dyn SyncProvider>,
    child: tokio::sync::Mutex<Option<ChildProcess>>,
    stderr_task: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}
impl ActivatedProvider {
    pub async fn child_exited(&self) -> std::io::Result<bool> {
        let mut child = self.child.lock().await;
        match child.as_mut() {
            Some(child) => Ok(child.try_wait()?.is_some()),
            None => Ok(true),
        }
    }
    pub async fn shutdown(&self) -> std::io::Result<()> {
        // The provider gets a cooperative deadline first; process-tree kill is
        // the deterministic fallback and the child remains joined by wait().
        let _ = tokio::time::timeout(Duration::from_secs(5), self.provider.shutdown()).await;
        let child = self.child.lock().await.take();
        if let Some(mut child) = child {
            child.terminate_tree().await?;
            let _ = child.wait().await?;
        }
        if let Some(stderr_task) = self.stderr_task.lock().await.take() {
            let _ = stderr_task.await;
        }
        Ok(())
    }
}

pub async fn activate_and_register(
    manager: &ProviderManager,
    manifest: &ManifestRecord,
    account_uuid: &str,
    granted_permissions: Vec<crate::plugins::manifest::Permission>,
    bundled_root: &std::path::Path,
    platform: Platform,
    allowlist: &[TrustEntry],
    runtime_dir: std::path::PathBuf,
    reverse: Option<Arc<dyn super::rpc::ReverseRpcHandler>>,
    onedrive_client_id: Option<String>,
) -> anyhow::Result<ActivatedProvider> {
    if !manifest.rpc.wire_versions.contains(&1) {
        anyhow::bail!("Provider manifest does not support Host wire version 1");
    }
    let executable = verify_bundled_entry(manifest, bundled_root, platform, allowlist)
        .map_err(|e| anyhow::anyhow!("plugin trust verification failed: {e:?}"))?;
    let safe_env = provider_safe_env(&manifest.id, &runtime_dir, onedrive_client_id);
    let mut child = ChildProcess::spawn_verified(
        &executable,
        &LaunchSpec {
            runtime_dir,
            safe_env,
        },
    )?;
    let (stdin, stdout, stderr) = child.take_stdio()?;
    let session =
        FramedSession::start_with_handler(stdout, stdin, FrameLimits::default(), 1, reverse);
    let mut nonce = [0_u8; 32];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut nonce);
    let hello = Hello {
        nonce,
        wire_versions: vec![1],
        plugin_id: manifest.id.clone(),
        version: manifest.version.clone(),
        account_uuid: account_uuid.to_owned(),
        capabilities: manifest
            .rpc
            .capabilities
            .iter()
            .map(|capability| match capability {
                crate::plugins::manifest::Capability::SyncProviderV1 => {
                    "sync-provider.v1".to_owned()
                }
            })
            .collect(),
        granted_permissions,
        method_versions: manifest.rpc.method_versions.clone(),
        limits: RpcLimits::host_default(),
    };
    let ack: HelloAck = serde_json::from_value(
        session
            .call(
                "runtime.hello",
                serde_json::to_value(&hello)?,
                Duration::from_secs(5),
                None,
                None,
            )
            .await
            .map_err(|error| anyhow::anyhow!("provider hello failed: {error:?}"))?,
    )?;
    verify_hello(&hello, &ack)
        .map_err(|error| anyhow::anyhow!("provider hello rejected: {error}"))?;
    session.complete_handshake();
    let provider: Arc<dyn SyncProvider> =
        Arc::new(FramedSyncProvider::new(session, Duration::from_secs(30)));
    let description = tokio::time::timeout(Duration::from_secs(5), provider.describe())
        .await
        .map_err(|_| anyhow::anyhow!("provider handshake deadline exceeded"))??;
    if description.plugin_id != manifest.id
        || description.account_uuid != account_uuid
        || !description
            .capabilities
            .iter()
            .any(|v| v == "sync-provider.v1")
        || description.max_object_bytes == 0
        || description.max_object_bytes > hello.limits.max_object_bytes
    {
        child.terminate_tree().await?;
        anyhow::bail!("provider handshake identity or capability mismatch");
    }
    let plugin_id = manifest.id.clone();
    let stderr_task = tokio::spawn(async move {
        let mut lines = BufReader::new(stderr).split(b'\n');
        while let Ok(Some(line)) = lines.next_segment().await {
            let safe = redact_stderr_jsonl(&line, 8 * 1024);
            tracing::debug!(plugin_id = %plugin_id, message = %safe, "provider stderr");
        }
    });
    manager.register(&manifest.id, account_uuid, Arc::clone(&provider))?;
    Ok(ActivatedProvider {
        provider,
        child: tokio::sync::Mutex::new(Some(child)),
        stderr_task: tokio::sync::Mutex::new(Some(stderr_task)),
    })
}

fn provider_safe_env(
    plugin_id: &str,
    runtime_dir: &std::path::Path,
    onedrive_client_id: Option<String>,
) -> std::collections::HashMap<String, String> {
    if plugin_id == "com.megle.fake-sync" {
        let mut values = std::collections::HashMap::from([(
            "MEGLE_FAKE_PROVIDER_STATE_DIR".to_owned(),
            runtime_dir
                .join("provider-state")
                .to_string_lossy()
                .into_owned(),
        )]);
        if let Ok(fault) = std::env::var("MEGLE_FAKE_PROVIDER_FAULT") {
            values.insert("MEGLE_FAKE_PROVIDER_FAULT".to_owned(), fault);
        }
        return values;
    }
    if plugin_id != "com.megle.onedrive" {
        return Default::default();
    }
    let mut values = std::collections::HashMap::new();
    if let Some(client_id) = onedrive_client_id {
        values.insert("MEGLE_ONEDRIVE_CLIENT_ID".to_owned(), client_id);
    }
    let mut keys = vec!["MEGLE_ONEDRIVE_AUTHORITY", "MEGLE_ONEDRIVE_AUTH_MODE"];
    if cfg!(debug_assertions) {
        keys.push("MEGLE_ONEDRIVE_EMULATOR_URL");
    }
    values.extend(
        keys.into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value))),
    );
    values
}
#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicUsize, Ordering};
    #[test]
    fn breaker_and_manual_retry_are_deterministic() {
        let p = BackoffPolicy {
            steps: vec![Duration::from_secs(1)],
            breaker_limit: 2,
            breaker_window: Duration::from_secs(10),
        };
        let mut s = Supervisor::new(p);
        let now = Instant::now();
        assert!(s.begin_start(now));
        s.abnormal_exit(now);
        assert!(!s.begin_start(now));
        s.abnormal_exit(now);
        s.manual_retry();
        assert!(s.begin_start(now));
    }
    #[test]
    fn handshake_checks_nonce_identity_wire_and_capability() {
        let h = Hello {
            nonce: [7; 32],
            wire_versions: vec![1],
            plugin_id: "p".into(),
            version: "1".into(),
            account_uuid: "account".into(),
            capabilities: vec!["sync-provider.v1".into()],
            granted_permissions: Vec::new(),
            method_versions: crate::plugins::manifest::required_sync_provider_method_versions(),
            limits: RpcLimits::host_default(),
        };
        let mut a = HelloAck {
            nonce: [7; 32],
            wire_version: 1,
            plugin_id: "p".into(),
            version: "1".into(),
            account_uuid: "account".into(),
            capabilities: vec!["sync-provider.v1".into()],
            method_versions: crate::plugins::manifest::required_sync_provider_method_versions(),
            limits: RpcLimits::host_default(),
        };
        assert_eq!(verify_hello(&h, &a), Ok(1));
        a.nonce = [8; 32];
        assert!(verify_hello(&h, &a).is_err());
    }

    #[test]
    fn health_threshold_degrades_and_recovery_resets_counter() {
        let mut supervisor = Supervisor::new(BackoffPolicy::default());
        supervisor.ready();
        assert!(!supervisor.health_result(false, 2));
        assert!(supervisor.health_result(false, 2));
        assert_eq!(supervisor.state(), RuntimeState::Degraded);
        assert!(!supervisor.health_result(true, 2));
        assert_eq!(supervisor.state(), RuntimeState::Ready);
        assert!(!supervisor.health_result(false, 2));
    }

    #[tokio::test]
    async fn health_supervisor_restarts_and_joins_on_cancellation() {
        let root = std::env::temp_dir().join(format!(
            "megle_supervisor_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let registry =
            Arc::new(crate::host_registry::HostRegistry::open(root.join("host.sqlite")).unwrap());
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "com.megle.fake",
                version: "1.0.0",
                manifest_json: "{}",
                entry_relative_path: "fake",
                entry_sha256: &[0; 32],
                install_root: &root,
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .upsert_plugin_account("fake-account", "com.megle.fake", None, "authenticated")
            .unwrap();
        registry
            .set_plugin_intent("com.megle.fake", true, true)
            .unwrap();
        let launch_fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
        let manager = ProviderManager::default();
        let cancellation = tokio_util::sync::CancellationToken::new();
        let attempts = Arc::new(AtomicUsize::new(0));
        let attempts_for_factory = Arc::clone(&attempts);
        let cancellation_for_task = cancellation.clone();
        let task = tokio::spawn(supervise_until_cancelled(
            Arc::clone(&registry),
            manager.clone(),
            "com.megle.fake".into(),
            "fake-account".into(),
            launch_fingerprint,
            cancellation_for_task,
            BackoffPolicy {
                steps: vec![Duration::from_millis(1)],
                breaker_limit: 4,
                breaker_window: Duration::from_secs(1),
            },
            HealthPolicy {
                interval: Duration::from_millis(1),
                deadline: Duration::from_millis(10),
                degraded_threshold: 1,
                probation_successes: 1,
            },
            move || {
                let attempt = attempts_for_factory.fetch_add(1, Ordering::SeqCst);
                async move {
                    if attempt == 0 {
                        anyhow::bail!("scripted activation failure")
                    }
                    let provider: Arc<dyn SyncProvider> =
                        Arc::new(crate::sync::provider::InMemorySyncProvider::default());
                    Ok(ActivatedProvider {
                        provider,
                        child: tokio::sync::Mutex::new(None),
                        stderr_task: tokio::sync::Mutex::new(None),
                    })
                }
            },
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            while attempts.load(Ordering::SeqCst) < 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        cancellation.cancel();
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(manager
            .resolve("com.megle.fake", "fake-account")
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn supervisor_stops_when_account_generation_invalidates_its_launch() {
        let root = std::env::temp_dir().join(format!(
            "megle_supervisor_stale_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let registry =
            Arc::new(crate::host_registry::HostRegistry::open(root.join("host.sqlite")).unwrap());
        registry
            .register_bundled_plugin(crate::host_registry::RegisterBundledPlugin {
                plugin_id: "com.megle.fake",
                version: "1.0.0",
                manifest_json: "{}",
                entry_relative_path: "fake",
                entry_sha256: &[0; 32],
                install_root: &root,
                trust_policy_id: "test",
                requested_permissions_json: "[]",
            })
            .unwrap();
        registry
            .upsert_plugin_account("fake-account", "com.megle.fake", None, "authenticated")
            .unwrap();
        registry
            .set_plugin_intent("com.megle.fake", true, true)
            .unwrap();
        let launch_fingerprint = registry.plugin_runtime_launches().unwrap()[0].fingerprint();
        let manager = ProviderManager::default();
        let activation_manager = manager.clone();
        let task = tokio::spawn(supervise_until_cancelled(
            Arc::clone(&registry),
            manager.clone(),
            "com.megle.fake".into(),
            "fake-account".into(),
            launch_fingerprint.clone(),
            tokio_util::sync::CancellationToken::new(),
            Default::default(),
            HealthPolicy {
                interval: Duration::from_millis(1),
                deadline: Duration::from_millis(10),
                degraded_threshold: 3,
                probation_successes: 1,
            },
            move || {
                let manager = activation_manager.clone();
                async move {
                    let provider: Arc<dyn SyncProvider> =
                        Arc::new(crate::sync::provider::InMemorySyncProvider::default());
                    manager.register("com.megle.fake", "fake-account", Arc::clone(&provider))?;
                    Ok(ActivatedProvider {
                        provider,
                        child: tokio::sync::Mutex::new(None),
                        stderr_task: tokio::sync::Mutex::new(None),
                    })
                }
            },
        ));
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if manager
                    .resolve("com.megle.fake", "fake-account")
                    .unwrap()
                    .is_some()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(registry
            .require_plugin_reauthorization_for_launch("com.megle.fake", &launch_fingerprint)
            .unwrap());
        tokio::time::timeout(Duration::from_secs(1), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(manager
            .resolve("com.megle.fake", "fake-account")
            .unwrap()
            .is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}
