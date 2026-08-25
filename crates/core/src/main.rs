mod api;
mod db;
mod decode;
#[macro_use]
mod debug;
mod fsops;
mod host_registry;
mod library_manager;
mod library_path;
mod plugins;
mod roots;
mod scan;
mod sync;
mod tasks;
mod thumbnails;
mod watch;

use std::net::SocketAddr;
use std::path::PathBuf;

use axum::http::HeaderValue;

use crate::api::BasicAuthCredentials;

fn run_plugin_runtime_test_helper() -> anyhow::Result<bool> {
    let Ok(mode) = std::env::var("MEGLE_PLUGIN_RUNTIME_TEST_HELPER") else {
        return Ok(false);
    };
    match mode.as_str() {
        "exit" => {}
        "stdout-noise" => {
            use std::io::Write;
            std::io::stdout().write_all(b"not-a-framed-provider")?;
            std::io::stdout().flush()?;
        }
        "hang" => loop {
            std::thread::park_timeout(std::time::Duration::from_secs(60));
        },
        "tree" => {
            let child = std::process::Command::new(std::env::current_exe()?)
                .env("MEGLE_PLUGIN_RUNTIME_TEST_HELPER", "hang")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()?;
            println!("{}", child.id());
            loop {
                std::thread::park_timeout(std::time::Duration::from_secs(60));
            }
        }
        "framed" => {
            use std::io::{Read, Write};
            let mut input = std::io::stdin().lock();
            let mut output = std::io::stdout().lock();
            loop {
                let mut header = [0_u8; 17];
                match input.read_exact(&mut header) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => break,
                    Err(error) => return Err(error.into()),
                }
                let length = u32::from_le_bytes(header[..4].try_into().unwrap()) as usize;
                if length > 1024 * 1024 || header[4] != 1 {
                    anyhow::bail!("invalid framed helper request")
                }
                let mut payload = vec![0_u8; length];
                input.read_exact(&mut payload)?;
                let mut envelope: serde_json::Value = serde_json::from_slice(&payload)?;
                let method = envelope["method"].as_str().unwrap_or_default().to_string();
                envelope["messageType"] = serde_json::json!("response");
                envelope["payload"] = match method.as_str() {
                    "runtime.hello" => {
                        let hello = envelope["payload"].clone();
                        serde_json::json!({
                            "nonce": hello["nonce"],
                            "wireVersion": 1,
                            "pluginId": hello["pluginId"],
                            "version": hello["version"],
                            "accountUuid": hello["accountUuid"],
                            "capabilities":["sync-provider.v1"],
                            "methodVersions": hello["methodVersions"],
                            "limits": hello["limits"]
                        })
                    }
                    "describe" => serde_json::json!({
                        "pluginId":"com.megle.fake",
                        "accountUuid":"fake-account",
                        "capabilities":["sync-provider.v1"],
                        "maxObjectBytes":67108864u64
                    }),
                    "health" => serde_json::json!({"ready":true,"detail":null}),
                    "diagnostics" => serde_json::json!({"helper":true}),
                    "shutdown" => serde_json::Value::Null,
                    _ => serde_json::json!({"ok":true}),
                };
                let response = serde_json::to_vec(&envelope)?;
                output.write_all(&(response.len() as u32).to_le_bytes())?;
                output.write_all(&[1])?;
                output.write_all(&0_u64.to_le_bytes())?;
                output.write_all(&0_u32.to_le_bytes())?;
                output.write_all(&response)?;
                output.flush()?;
                if method == "shutdown" {
                    break;
                }
            }
        }
        _ => anyhow::bail!("unknown plugin runtime test helper mode"),
    }
    Ok(true)
}

fn initialize_tracing() {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .json()
        .with_writer(std::io::stderr)
        .init();
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    if run_plugin_runtime_test_helper()? {
        return Ok(());
    }
    initialize_tracing();
    let host_path = std::env::var("MEGLE_HOST_DB_PATH")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let libraries_dir = std::env::var("MEGLE_LIBRARIES_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let bind_addr: SocketAddr = std::env::var("MEGLE_CORE_ADDR")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "127.0.0.1:47321".to_string())
        .parse()?;
    let basic_auth = std::env::var("MEGLE_BASIC_AUTH")
        .ok()
        .filter(|s| !s.is_empty())
        .and_then(|raw| {
            BasicAuthCredentials::parse(&raw).or_else(|| {
                tracing::warn!("MEGLE_BASIC_AUTH is set but is not in `user:pass` form; ignoring");
                None
            })
        });
    // Treat unset and empty-string `MEGLE_SESSION_TOKEN` identically: an empty
    // env var (e.g. from `${MEGLE_SESSION_TOKEN:-}` in compose) means "no
    // session token configured", not "configured to the empty token".
    let session_token = std::env::var("MEGLE_SESSION_TOKEN")
        .ok()
        .filter(|s| !s.is_empty());
    if session_token.is_none() && basic_auth.is_none() {
        // When Basic auth is enabled the operator can opt out of the
        // desktop session-token mechanism by leaving `MEGLE_SESSION_TOKEN`
        // unset. Otherwise the token is mandatory.
        anyhow::bail!("MEGLE_SESSION_TOKEN must be set before starting Megle Core");
    }
    let allowed_origin = std::env::var("MEGLE_ALLOWED_ORIGIN")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|origin| origin.parse::<HeaderValue>())
        .transpose()?;

    let host_path = host_path.ok_or_else(|| {
        anyhow::anyhow!("MEGLE_HOST_DB_PATH must be set before starting Megle Core")
    })?;
    let libraries_dir = libraries_dir
        .or_else(|| host_path.parent().map(|parent| parent.join("libraries")))
        .ok_or_else(|| anyhow::anyhow!("MEGLE_LIBRARIES_DIR is required for Core Host mode"))?;
    let state = api::AppState::new_host(host_path, libraries_dir).await?;
    let web_dir = resolve_web_dir();

    let listener = bind_with_retry(bind_addr).await?;
    let ready_addr = listener.local_addr()?;
    tracing::info!(
        event_kind = "startup_ready",
        bind_addr = %ready_addr,
        "Megle Core ready"
    );
    let serve_result = axum::serve(
        listener,
        api::router_with_state(
            state.clone(),
            api::ApiConfig {
                session_token,
                allowed_origin,
                web_dir,
                basic_auth,
            },
        ),
    )
    .with_graceful_shutdown(async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::warn!(%error, "failed to install Core shutdown signal");
        }
    })
    .await;
    state.shutdown_plugin_auth_workers().await;
    if let Some(manager) = &state.library_manager {
        manager.shutdown_sync_coordinator().await?;
    }
    serve_result?;
    Ok(())
}

/// Bind the API listener, retrying briefly on transient "address in use"
/// failures. The desktop shell allocates a free loopback port by opening and
/// immediately closing a probe socket, then hands that port to Core; on Windows
/// the just-closed socket can linger in TIME_WAIT so Core's first bind races and
/// fails, which used to crash Core (exit 1) and force the desktop to retry the
/// whole Core-start on a fresh port — a multi-second, user-visible startup
/// stall. Retrying the bind here absorbs that race at the source.
async fn bind_with_retry(bind_addr: SocketAddr) -> anyhow::Result<tokio::net::TcpListener> {
    const MAX_ATTEMPTS: usize = 20;
    const RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
    let mut last_error = None;
    for attempt in 1..=MAX_ATTEMPTS {
        match tokio::net::TcpListener::bind(bind_addr).await {
            Ok(listener) => return Ok(listener),
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                tracing::warn!(
                    %bind_addr,
                    attempt,
                    "port not yet free (likely TIME_WAIT after the desktop port probe); retrying bind"
                );
                last_error = Some(error);
                tokio::time::sleep(RETRY_DELAY).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
    Err(last_error
        .map(anyhow::Error::from)
        .unwrap_or_else(|| anyhow::anyhow!("failed to bind {bind_addr}")))
}

/// Resolve the directory holding the built web UI when static-serve mode is
/// requested. Returns `None` (and the API runs without a static UI) when:
/// - `MEGLE_SERVE_WEB` is not `1`,
/// - or the configured directory does not exist on disk.
fn resolve_web_dir() -> Option<PathBuf> {
    let enabled = std::env::var("MEGLE_SERVE_WEB")
        .ok()
        .filter(|s| !s.is_empty())
        .map(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let configured = std::env::var("MEGLE_WEB_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("apps/web/dist"));
    if !configured.is_dir() {
        tracing::warn!(
            "MEGLE_SERVE_WEB=1 but {} is not a directory; static UI disabled",
            configured.display()
        );
        return None;
    }
    Some(configured)
}
