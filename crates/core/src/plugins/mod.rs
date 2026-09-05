pub mod auth_flow;
pub mod manifest;
pub mod runtime;
pub mod secret_store;

pub const SYNC_PROVIDER_V1_CAPABILITY: &str = "sync-provider.v1";

#[allow(unused_imports)]
pub use manifest::{parse_manifest, ManifestError, ManifestRecord};
use std::fs;
use std::path::{Path, PathBuf};

pub fn bundled_root_from_environment() -> anyhow::Result<PathBuf> {
    if let Some(value) = std::env::var_os("MEGLE_PLUGINS_DIR").filter(|v| !v.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    let executable = std::env::current_exe()?;
    Ok(executable
        .parent()
        .unwrap_or(Path::new("."))
        .join("bundled-plugins"))
}

#[derive(Debug, Clone)]
pub struct DiscoveryErrorEntry {
    pub manifest_path: PathBuf,
    pub message: String,
}

pub fn discover(plugins_dir: &Path) -> (Vec<ManifestRecord>, Vec<DiscoveryErrorEntry>) {
    let mut records = Vec::new();
    let mut errors = Vec::new();
    let entries = match fs::read_dir(plugins_dir) {
        Ok(v) => v,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return (records, errors),
        Err(e) => {
            errors.push(DiscoveryErrorEntry {
                manifest_path: plugins_dir.into(),
                message: e.to_string(),
            });
            return (records, errors);
        }
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|v| v.path()).collect();
    paths.sort();
    for dir in paths {
        if !dir.is_dir() {
            continue;
        }
        let path = dir.join("plugin.json");
        if !path.exists() {
            continue;
        }
        match parse_manifest(&path) {
            Ok(v) => records.push(v),
            Err(e) => errors.push(DiscoveryErrorEntry {
                manifest_path: path,
                message: e.to_string(),
            }),
        }
    }
    (records, errors)
}

pub fn reconcile_bundled(
    registry: &crate::host_registry::HostRegistry,
    bundled_root: &Path,
) -> (usize, Vec<DiscoveryErrorEntry>) {
    if !runtime::process::is_bundled_root(bundled_root) {
        return (
            0,
            vec![DiscoveryErrorEntry {
                manifest_path: bundled_root.to_path_buf(),
                message: "plugin discovery root is not a packaged bundled root".into(),
            }],
        );
    }
    let platform = if cfg!(windows) {
        manifest::Platform::WindowsX64
    } else {
        manifest::Platform::LinuxX64
    };
    let (records, mut errors) = discover(bundled_root);
    let mut registered = 0;
    for record in records {
        let Some(relative) = record.entry_for(platform) else {
            errors.push(DiscoveryErrorEntry {
                manifest_path: record.manifest_path.clone(),
                message: "bundled Provider has no entry for this platform".into(),
            });
            continue;
        };
        let plugin_root = record.manifest_path.parent().unwrap_or(bundled_root);
        match runtime::trust::verify_bundled_entry(
            &record,
            plugin_root,
            platform,
            runtime::trust::embedded_trust_registry(),
        ) {
            Ok(verified) => {
                let manifest_json = match serde_json::to_string(&record) {
                    Ok(value) => value,
                    Err(error) => {
                        errors.push(DiscoveryErrorEntry {
                            manifest_path: record.manifest_path.clone(),
                            message: error.to_string(),
                        });
                        continue;
                    }
                };
                let permissions = serde_json::to_string(&record.requested_permissions)
                    .unwrap_or_else(|_| "[]".into());
                let registration = crate::host_registry::RegisterBundledPlugin {
                    plugin_id: &record.id,
                    version: &record.version,
                    manifest_json: &manifest_json,
                    entry_relative_path: relative,
                    entry_sha256: &verified.expected_sha256(),
                    install_root: plugin_root,
                    trust_policy_id: "embedded-v1",
                    requested_permissions_json: &permissions,
                };
                if let Err(error) = registry.register_bundled_plugin(registration) {
                    errors.push(DiscoveryErrorEntry {
                        manifest_path: record.manifest_path.clone(),
                        message: error.to_string(),
                    });
                } else {
                    registered += 1;
                }
            }
            Err(error) => errors.push(DiscoveryErrorEntry {
                manifest_path: record.manifest_path.clone(),
                message: format!("bundled trust verification failed: {error:?}"),
            }),
        }
    }
    (registered, errors)
}
