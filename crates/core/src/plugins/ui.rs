//! Verified plugin documents. This path never starts or calls a Provider.
use std::path::Path;

use super::manifest::{ManifestRecord, Platform};
use super::runtime::trust::{self, TrustEntry};
use crate::host_registry::HostRegistry;

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostPluginViewDocument {
    pub plugin_id: String,
    pub lifecycle_epoch: i64,
    pub version: String,
    pub view_id: String,
    pub title: String,
    pub document: String,
}

pub fn load_view_document(
    registry: &HostRegistry,
    bundled_root: &Path,
    plugin_id: &str,
    view_id: &str,
    platform: Platform,
    allowlist: &[TrustEntry],
) -> anyhow::Result<HostPluginViewDocument> {
    let source = registry
        .plugin_ui_source(plugin_id)?
        .ok_or_else(|| anyhow::anyhow!("plugin-ui-unavailable"))?;
    // Discovery is one directory per plugin. Persisted paths cannot broaden it.
    anyhow::ensure!(
        source.install_root == bundled_root.join(plugin_id),
        "plugin-ui-untrusted"
    );
    let manifest: ManifestRecord = serde_json::from_str(&source.manifest_json)
        .map_err(|_| anyhow::anyhow!("plugin-ui-untrusted"))?;
    anyhow::ensure!(
        manifest.id == plugin_id
            && manifest.version == source.version
            && manifest.requested_permissions == source.requested_permissions,
        "plugin-ui-untrusted"
    );
    trust::verify_manifest(&manifest, &source.install_root, platform, allowlist)
        .map_err(|_| anyhow::anyhow!("plugin-ui-untrusted"))?;
    let ui = manifest
        .contributions
        .as_ref()
        .and_then(|c| c.ui.as_ref())
        .ok_or_else(|| anyhow::anyhow!("plugin-view-not-found"))?;
    let view = ui
        .views
        .iter()
        .find(|v| v.id == view_id)
        .ok_or_else(|| anyhow::anyhow!("plugin-view-not-found"))?;
    let document = trust::read_ui_document(&source.install_root, ui)
        .map_err(|_| anyhow::anyhow!("plugin-ui-untrusted"))?;
    anyhow::ensure!(
        registry.plugin_ui_source(plugin_id)?.as_ref() == Some(&source),
        "plugin-ui-changed"
    );
    Ok(HostPluginViewDocument {
        plugin_id: plugin_id.to_owned(),
        lifecycle_epoch: source.lifecycle_epoch,
        version: source.version,
        view_id: view.id.clone(),
        title: view.title.clone(),
        document,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_registry::RegisterBundledPlugin;
    use sha2::{Digest, Sha256};

    struct Fixture {
        registry: HostRegistry,
        root: std::path::PathBuf,
        manifest: ManifestRecord,
        trust: Vec<TrustEntry>,
        _cleanup: Cleanup,
    }

    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    impl Fixture {
        fn new() -> Self {
            let root =
                std::env::temp_dir().join(format!("mangoo-plugin-ui-{}", uuid::Uuid::now_v7()));
            let plugin_root = root.join("com.example.ui");
            std::fs::create_dir_all(plugin_root.join("ui")).unwrap();
            let document = b"<!doctype html><script>window.fixture = true;</script>";
            std::fs::write(plugin_root.join("ui/index.html"), document).unwrap();
            let mut manifest: serde_json::Value = serde_json::from_str(include_str!(
                "../../../../contracts/plugins/fixtures/valid-ui-provider-v2.json"
            ))
            .unwrap();
            manifest["contributions"]["ui"]["sha256"] =
                serde_json::json!(format!("{:x}", Sha256::digest(document)));
            // Missing Secret Store produces configuration-required while its UI
            // must remain available so the user can complete configuration.
            manifest["requestedPermissions"].as_array_mut().unwrap().push(serde_json::json!({"kind":"secret","namespace":"fixture","operations":["read"]}));
            let bytes = serde_json::to_vec(&manifest).unwrap();
            let manifest_path = plugin_root.join("plugin.json");
            std::fs::write(&manifest_path, &bytes).unwrap();
            let manifest = super::super::manifest::parse_manifest(&manifest_path).unwrap();
            let registry = HostRegistry::open(root.join("host.sqlite")).unwrap();
            registry
                .register_bundled_plugin(RegisterBundledPlugin {
                    plugin_id: &manifest.id,
                    version: &manifest.version,
                    manifest_json: &serde_json::to_string(&manifest).unwrap(),
                    entry_relative_path: "bin/provider.exe",
                    entry_sha256: &[1; 32],
                    install_root: &plugin_root,
                    trust_policy_id: "embedded-v1",
                    requested_permissions_json: &serde_json::to_string(
                        &manifest.requested_permissions,
                    )
                    .unwrap(),
                })
                .unwrap();
            let trust = vec![TrustEntry {
                plugin_id: manifest.id.clone(),
                version: manifest.version.clone(),
                platform: Platform::WindowsX64,
                binary_sha256: [1; 32],
                manifest_sha256: Sha256::digest(&bytes).into(),
            }];
            let _cleanup = Cleanup(root.clone());
            Self {
                registry,
                root,
                manifest,
                trust,
                _cleanup,
            }
        }

        fn enable(&self, secret_store: bool) {
            self.registry
                .set_plugin_grants(
                    &self.manifest.id,
                    &serde_json::to_string(&self.manifest.requested_permissions).unwrap(),
                    secret_store,
                )
                .unwrap();
            self.registry
                .finish_plugin_runtime_restart(&self.manifest.id)
                .unwrap();
            self.registry
                .set_plugin_intent(&self.manifest.id, true, secret_store)
                .unwrap();
        }

        fn load(&self, view: &str) -> anyhow::Result<HostPluginViewDocument> {
            load_view_document(
                &self.registry,
                &self.root,
                &self.manifest.id,
                view,
                Platform::WindowsX64,
                &self.trust,
            )
        }
    }

    #[test]
    fn ui_source_requires_current_grants_and_non_stopping_enabled_intent() {
        let fixture = Fixture::new();
        let id = &fixture.manifest.id;
        assert!(fixture.registry.plugin_ui_source(id).unwrap().is_none());
        fixture.registry.set_plugin_intent(id, true, true).unwrap();
        assert!(fixture.registry.plugin_ui_source(id).unwrap().is_none());
        fixture.enable(true);
        let before = fixture.registry.plugin_ui_source(id).unwrap().unwrap();
        fixture.registry.mark_plugin_stop_pending(id).unwrap();
        assert!(fixture.registry.plugin_ui_source(id).unwrap().is_none());
        fixture.registry.finish_plugin_runtime_restart(id).unwrap();
        assert_ne!(
            fixture.registry.plugin_ui_source(id).unwrap().unwrap(),
            before
        );
        let launch = fixture
            .registry
            .plugin_runtime_launches()
            .unwrap()
            .pop()
            .unwrap();
        assert!(fixture
            .registry
            .record_plugin_runtime_failure(
                id,
                &launch.fingerprint(),
                "incompatible",
                "fixture",
                "fixture",
                None
            )
            .unwrap());
        assert!(fixture.registry.plugin_ui_source(id).unwrap().is_none());
    }

    #[cfg(windows)]
    #[test]
    fn verified_ui_loads_without_provider_or_authenticated_account_and_rejects_changes() {
        let mut fixture = Fixture::new();
        fixture.enable(false);
        let projection = fixture
            .registry
            .host_plugin(&fixture.manifest.id)
            .unwrap()
            .unwrap();
        assert_eq!(projection.runtime_state, "configuration-required");
        assert_eq!(
            projection.auth_state.as_deref(),
            Some("authorization-required")
        );
        assert!(fixture
            .registry
            .plugin_runtime_launches()
            .unwrap()
            .is_empty());
        let document = fixture.load("setup").unwrap();
        assert_eq!(document.plugin_id, fixture.manifest.id);
        assert_eq!(document.lifecycle_epoch, projection.lifecycle_epoch);
        assert!(document.lifecycle_epoch > 0);
        assert_eq!(document.version, "1.0.0");
        assert!(document.document.contains("window.fixture"));
        assert_eq!(
            fixture.load("missing").unwrap_err().to_string(),
            "plugin-view-not-found"
        );
        fixture.trust[0].manifest_sha256 = [0; 32];
        assert_eq!(
            fixture.load("setup").unwrap_err().to_string(),
            "plugin-ui-untrusted"
        );
        fixture.trust[0].manifest_sha256 =
            Sha256::digest(std::fs::read(fixture.root.join("com.example.ui/plugin.json")).unwrap())
                .into();
        std::fs::write(fixture.root.join("com.example.ui/ui/index.html"), "changed").unwrap();
        assert_eq!(
            fixture.load("setup").unwrap_err().to_string(),
            "plugin-ui-untrusted"
        );
        let path = fixture.root.join("com.example.ui/plugin.json");
        let mut replacement: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        replacement["contributions"]["ui"]["sha256"] =
            serde_json::json!(format!("{:x}", Sha256::digest(b"changed")));
        std::fs::write(path, serde_json::to_vec(&replacement).unwrap()).unwrap();
        assert_eq!(
            fixture.load("setup").unwrap_err().to_string(),
            "plugin-ui-untrusted"
        );
        fixture
            .registry
            .set_plugin_intent(&fixture.manifest.id, false, false)
            .unwrap();
        assert_eq!(
            fixture.load("setup").unwrap_err().to_string(),
            "plugin-ui-unavailable"
        );
    }
}
