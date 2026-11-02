//! Bounded Provider metadata and integrity-pinned sandbox UI declarations.
use super::manifest::{HostUiOperation, Permission};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginContributions {
    pub configuration: Vec<PluginConfigurationField>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_ui"
    )]
    pub ui: Option<PluginUiContribution>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub authentication: Option<PluginAuthenticationContribution>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub presentation: Option<PluginPresentationContribution>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub primary_credential: Option<PrimaryCredential>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiContribution {
    pub api_version: u8,
    pub entry: String,
    pub sha256: String,
    pub views: Vec<PluginUiView>,
}

fn deserialize_ui<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<PluginUiContribution>, D::Error> {
    PluginUiContribution::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginUiView {
    pub id: String,
    pub title: String,
    pub slot: PluginUiSlot,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PluginUiSlot {
    LibraryMenu,
    LibrarySettings,
    PluginDetail,
    RootMenu,
    NotificationCenter,
}

pub fn valid_ui_entry(entry: &str) -> bool {
    entry.len() <= 512
        && entry.ends_with(".html")
        && entry.split('/').all(|part| {
            !part.is_empty()
                && !part.ends_with('.')
                && (part.as_bytes()[0].is_ascii_alphanumeric()
                    || matches!(part.as_bytes()[0], b'_' | b'-'))
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
        })
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginConfigurationField {
    pub id: String,
    pub label: String,
    pub description: String,
    pub format: ConfigurationFormat,
    pub required: bool,
    #[serde(deserialize_with = "Option::deserialize")]
    pub environment: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub default_value: Option<String>,
    pub choices: Vec<String>,
    pub hidden: bool,
    pub debug_only: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ConfigurationFormat {
    Uuid,
    Text,
    Choice,
    HttpsUrl,
    LoopbackUrl,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginAuthenticationContribution {
    pub modes: Vec<String>,
    pub authorization_hosts: Vec<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub manual_redirect_uri: Option<String>,
    #[serde(deserialize_with = "Option::deserialize")]
    pub loopback_redirect_path: Option<String>,
    pub ignored_callback_fields: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginPresentationContribution {
    pub setup_label: String,
    pub account_label: String,
    pub icon: String,
    pub help_links: Vec<PluginHelpLink>,
    pub setup_steps: Vec<PluginSetupStep>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginHelpLink {
    pub id: String,
    pub label: String,
    pub url: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PluginSetupStep {
    pub title: String,
    pub body: String,
    #[serde(deserialize_with = "Option::deserialize")]
    pub help_link_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PrimaryCredential {
    pub namespace: String,
    pub purpose: String,
}

fn text(value: &str, max: usize) -> bool {
    value.len() <= max && !value.chars().any(char::is_control)
}
fn id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 64
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.bytes().all(|b| b.is_ascii_alphanumeric())
}
fn unique<T: std::hash::Hash + Eq>(values: &[T]) -> bool {
    values.iter().collect::<HashSet<_>>().len() == values.len()
}

pub fn safe_external_url(value: &str, allow_loopback: bool) -> anyhow::Result<url::Url> {
    anyhow::ensure!(
        !value.is_empty() && text(value, 8192) && !value.contains('\\'),
        "invalid-external-link"
    );
    let parsed = url::Url::parse(value).map_err(|_| anyhow::anyhow!("invalid-external-link"))?;
    let loopback = matches!(
        parsed.host_str(),
        Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
    );
    anyhow::ensure!(
        parsed.host_str().is_some()
            && parsed.username().is_empty()
            && parsed.password().is_none()
            && parsed.fragment().is_none()
            && ((parsed.scheme() == "https" && parsed.port_or_known_default() == Some(443))
                || (allow_loopback && loopback && parsed.scheme() == "http")),
        "invalid-external-link"
    );
    Ok(parsed)
}

impl PluginConfigurationField {
    pub fn normalize(&self, value: Option<&str>) -> anyhow::Result<Option<String>> {
        let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) else {
            return Ok(None);
        };
        anyhow::ensure!(text(value, 2048), "invalid-plugin-configuration-value");
        Ok(Some(match self.format {
            ConfigurationFormat::Uuid => {
                let id = uuid::Uuid::parse_str(value)
                    .map_err(|_| anyhow::anyhow!("invalid-plugin-configuration-value"))?;
                anyhow::ensure!(!id.is_nil(), "invalid-plugin-configuration-value");
                id.to_string()
            }
            ConfigurationFormat::Choice => {
                anyhow::ensure!(
                    self.choices.iter().any(|v| v == value),
                    "invalid-plugin-configuration-value"
                );
                value.to_owned()
            }
            ConfigurationFormat::HttpsUrl => {
                safe_external_url(value, false)?;
                value.to_owned()
            }
            ConfigurationFormat::LoopbackUrl => {
                let url = safe_external_url(value, true)?;
                anyhow::ensure!(
                    self.debug_only
                        && cfg!(debug_assertions)
                        && matches!(
                            url.host_str(),
                            Some("localhost" | "127.0.0.1" | "[::1]" | "::1")
                        ),
                    "invalid-plugin-configuration-value"
                );
                value.to_owned()
            }
            ConfigurationFormat::Text => value.to_owned(),
        }))
    }
}

impl PluginContributions {
    pub fn validate(&self, permissions: &[Permission]) -> anyhow::Result<()> {
        if let Some(ui) = &self.ui {
            anyhow::ensure!(
                ui.api_version == 1
                    && valid_ui_entry(&ui.entry)
                    && ui.sha256.len() == 64
                    && ui
                        .sha256
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                    && !ui.views.is_empty()
                    && ui.views.len() <= 16,
                "invalid-plugin-ui"
            );
            let mut views = HashSet::new();
            for view in &ui.views {
                anyhow::ensure!(
                    !view.id.is_empty()
                        && view.id.len() <= 64
                        && view.id.as_bytes()[0].is_ascii_alphabetic()
                        && view
                            .id
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
                        && views.insert(&view.id)
                        && !view.title.trim().is_empty()
                        && view.title.chars().count() <= 128
                        && !view.title.chars().any(char::is_control),
                    "invalid-plugin-ui-view"
                );
            }
            anyhow::ensure!(permissions.iter().any(|p| matches!(p,
                Permission::HostUi { operations } if operations.contains(&HostUiOperation::CoreApi))),
                "plugin-ui-permission-required");
        }
        anyhow::ensure!(
            self.configuration.len() <= 8,
            "too-many-configuration-fields"
        );
        let mut ids = HashSet::new();
        let mut environments = HashSet::new();
        for field in &self.configuration {
            anyhow::ensure!(
                id(&field.id)
                    && ids.insert(&field.id)
                    && !field.label.trim().is_empty()
                    && text(&field.label, 128)
                    && text(&field.description, 2048)
                    && field.choices.len() <= 16
                    && unique(&field.choices),
                "invalid-configuration-field"
            );
            anyhow::ensure!(
                field.choices.iter().all(|v| !v.is_empty() && text(v, 128)),
                "invalid-configuration-choices"
            );
            if let Some(env) = &field.environment {
                anyhow::ensure!(
                    env.starts_with("MANGOO_")
                        && env.len() > 6
                        && env.len() <= 106
                        && env
                            .bytes()
                            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_')
                        && environments.insert(env),
                    "invalid-configuration-environment"
                );
                anyhow::ensure!(
                    ![
                        "MANGOO_SESSION_TOKEN",
                        "MANGOO_SECRET_KEY_FILE",
                        "MANGOO_SECRETS_DIR",
                        "MANGOO_PLUGIN_CONFIGURATION"
                    ]
                    .contains(&env.as_str()),
                    "reserved-configuration-environment"
                );
                anyhow::ensure!(
                    !env.split('_')
                        .any(|part| ["SECRET", "SECRETS", "TOKEN", "PASSWORD", "KEY"]
                            .contains(&part)),
                    "secret-environment-not-configuration"
                );
            }
            anyhow::ensure!(
                field.format != ConfigurationFormat::LoopbackUrl || field.debug_only,
                "loopback-must-be-debug-only"
            );
            if !field.debug_only || cfg!(debug_assertions) {
                field.normalize(field.default_value.as_deref())?;
            }
        }
        if let Some(auth) = &self.authentication {
            anyhow::ensure!(
                !auth.modes.is_empty()
                    && auth.modes.len() <= 3
                    && unique(&auth.modes)
                    && auth.modes.iter().all(|m| [
                        "loopback",
                        "manual-response-uri",
                        "host-callback"
                    ]
                    .contains(&m.as_str())),
                "invalid-auth-modes"
            );
            anyhow::ensure!(
                !auth.authorization_hosts.is_empty()
                    && auth.authorization_hosts.len() <= 8
                    && unique(&auth.authorization_hosts),
                "invalid-auth-hosts"
            );
            for host in &auth.authorization_hosts {
                let url = safe_external_url(&format!("https://{host}/"), false)?;
                anyhow::ensure!(
                    url.host_str() == Some(host.as_str())
                        && host.contains('.')
                        && url.path() == "/",
                    "invalid-auth-host"
                );
                anyhow::ensure!(
                    permissions.iter().any(
                        |p| matches!(p, Permission::Network { hosts, .. } if hosts.contains(host))
                    ),
                    "auth-host-not-declared"
                );
            }
            if let Some(uri) = &auth.manual_redirect_uri {
                anyhow::ensure!(uri.len() <= 2048, "invalid-manual-redirect");
                let url = safe_external_url(uri, false)?;
                anyhow::ensure!(
                    url.query().is_none()
                        && auth
                            .authorization_hosts
                            .iter()
                            .any(|h| Some(h.as_str()) == url.host_str()),
                    "invalid-manual-redirect"
                );
            }
            anyhow::ensure!(
                !auth.modes.iter().any(|m| m == "manual-response-uri")
                    || auth.manual_redirect_uri.is_some(),
                "manual-redirect-required"
            );
            if let Some(path) = &auth.loopback_redirect_path {
                anyhow::ensure!(
                    path.starts_with('/')
                        && !path.starts_with("//")
                        && text(path, 256)
                        && !path.contains(['?', '#', '\\']),
                    "invalid-loopback-path"
                );
            }
            anyhow::ensure!(
                !auth.modes.iter().any(|mode| mode == "loopback")
                    || (auth.loopback_redirect_path.is_some()
                        && permissions
                            .iter()
                            .any(|p| matches!(p, Permission::OauthLoopback { .. }))),
                "loopback-permission-required"
            );
            anyhow::ensure!(
                auth.ignored_callback_fields.len() <= 4
                    && unique(&auth.ignored_callback_fields)
                    && auth.ignored_callback_fields.iter().all(|v| text(v, 64)
                        && !v.is_empty()
                        && !["code", "state", "error", "error_description", "error_uri"]
                            .contains(&v.as_str())),
                "invalid-ignored-callback-fields"
            );
        }
        if let Some(p) = &self.presentation {
            anyhow::ensure!(
                !p.setup_label.trim().is_empty()
                    && text(&p.setup_label, 128)
                    && !p.account_label.trim().is_empty()
                    && text(&p.account_label, 128)
                    && ["cloud", "storage"].contains(&p.icon.as_str())
                    && p.help_links.len() <= 4
                    && p.setup_steps.len() <= 8,
                "invalid-presentation"
            );
            let mut links = HashSet::new();
            for link in &p.help_links {
                anyhow::ensure!(
                    id(&link.id)
                        && links.insert(&link.id)
                        && !link.label.trim().is_empty()
                        && text(&link.label, 128)
                        && link.url.len() <= 2048,
                    "invalid-help-link"
                );
                safe_external_url(&link.url, false)?;
            }
            for step in &p.setup_steps {
                anyhow::ensure!(
                    !step.title.trim().is_empty()
                        && text(&step.title, 128)
                        && text(&step.body, 2048)
                        && step
                            .help_link_id
                            .as_ref()
                            .is_none_or(|id| links.contains(id)),
                    "invalid-setup-step"
                );
            }
        }
        if let Some(scope) = &self.primary_credential {
            anyhow::ensure!(!scope.purpose.is_empty() && scope.purpose.len() <= 64 && scope.purpose.as_bytes()[0].is_ascii_alphanumeric() && scope.purpose.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && permissions.iter().any(|p| matches!(p,Permission::Secret { namespace, .. } if namespace == &scope.namespace)), "invalid-primary-credential");
        }
        Ok(())
    }

    pub fn normalize_values(
        &self,
        values: &BTreeMap<String, Option<String>>,
    ) -> anyhow::Result<BTreeMap<String, String>> {
        anyhow::ensure!(values.len() <= 8, "invalid-plugin-configuration");
        let mut normalized = BTreeMap::new();
        for (key, value) in values {
            let field = self
                .configuration
                .iter()
                .find(|f| &f.id == key && (!f.debug_only || cfg!(debug_assertions)))
                .ok_or_else(|| anyhow::anyhow!("unknown-plugin-configuration-field"))?;
            if let Some(value) = field.normalize(value.as_deref())? {
                normalized.insert(key.clone(), value);
            }
        }
        Ok(normalized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn manifest(template: &str) -> super::super::manifest::ManifestRecord {
        serde_json::from_str(&template.replace("__SHA256__", &"0".repeat(64))).unwrap()
    }
    #[test]
    fn generic_contributions_two_providers_are_strict_and_non_executable() {
        for template in [
            include_str!(
                "../../../../contracts/plugins/fixtures/valid-configurable-provider-v2.json"
            ),
            include_str!("../../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"),
        ] {
            let record = manifest(template);
            let declaration = record.contributions.unwrap();
            declaration.validate(&record.requested_permissions).unwrap();
            let mut invalid = declaration.clone();
            invalid.configuration.push(invalid.configuration[0].clone());
            assert!(invalid.validate(&record.requested_permissions).is_err());
            let mut invalid = declaration.clone();
            invalid.configuration[0].environment = Some("MANGOO_SESSION_TOKEN".into());
            assert!(invalid.validate(&record.requested_permissions).is_err());
            let mut invalid = declaration.clone();
            invalid.presentation.as_mut().unwrap().help_links[0].url =
                "javascript:untrusted".into();
            assert!(invalid.validate(&record.requested_permissions).is_err());
            let mut invalid = declaration.clone();
            invalid.authentication.as_mut().unwrap().authorization_hosts =
                vec!["ungranted.example.test".into()];
            assert!(invalid.validate(&record.requested_permissions).is_err());
            let mut json = serde_json::to_value(&declaration).unwrap();
            json["executeJavascript"] = serde_json::json!("code");
            assert!(serde_json::from_value::<PluginContributions>(json).is_err());
            let mut json = serde_json::to_value(&declaration).unwrap();
            json.as_object_mut().unwrap().remove("authentication");
            assert!(serde_json::from_value::<PluginContributions>(json).is_err());
        }
        for url in [
            "https://user@example.test/",
            "https://example.test/#fragment",
            "https://example.test:444/",
            "http://example.test/",
            "https://example.test/\n",
        ] {
            assert!(safe_external_url(url, false).is_err());
        }
        assert!(safe_external_url("https://auth.example.test/authorize?opaque=one", false).is_ok());
    }
}
