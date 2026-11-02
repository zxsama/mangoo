use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use rand::{rngs::OsRng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, SystemTime},
};

pub const AUTH_FLOW_TTL: Duration = Duration::from_secs(10 * 60);
pub const MAX_CALLBACK_FIELD_BYTES: usize = 4096;
pub const MAX_CALLBACK_FIELDS: usize = 8;
pub const MAX_CALLBACK_QUERY_BYTES: usize = 128 * 1024;
pub const MAX_ACTIVE_AUTH_FLOWS: usize = 1024;
pub const MAX_MANUAL_RESPONSE_URI_BYTES: usize = 16 * 1024;
#[cfg(test)]
const MANUAL_REDIRECT_URI: &str = "https://login.microsoftonline.com/common/oauth2/nativeclient";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl(url::Url);

impl PublicUrl {
    pub fn parse(input: &str, allow_loopback_http: bool) -> anyhow::Result<Self> {
        let url = url::Url::parse(input)?;
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            anyhow::bail!("MANGOO_PUBLIC_URL must not contain userinfo, query or fragment");
        }
        let secure = url.scheme() == "https";
        let loopback_http = allow_loopback_http
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
        if !secure && !loopback_http {
            anyhow::bail!(
                "MANGOO_PUBLIC_URL must use HTTPS (HTTP is limited to loopback development)"
            );
        }
        if url.host_str().is_none() {
            anyhow::bail!("MANGOO_PUBLIC_URL must be an absolute URL");
        }
        Ok(Self(url))
    }
    pub fn callback_url(&self, flow_id: &str) -> anyhow::Result<url::Url> {
        if !flow_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
        {
            anyhow::bail!("invalid auth flow ID");
        }
        let mut callback = self.0.clone();
        let base = callback.path().trim_end_matches('/');
        callback.set_path(&format!("{base}/api/plugin-auth/callback/{flow_id}"));
        Ok(callback)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthFlowBinding {
    pub plugin_id: String,
    pub account_uuid: String,
    pub supervisor_session: String,
    pub provider_flow_id: Option<String>,
}

#[derive(Debug, Clone)]
struct StoredFlow {
    binding: AuthFlowBinding,
    expires_at: SystemTime,
}

#[derive(Clone, Debug)]
pub struct PendingAuthorization {
    pub authorization_url: String,
    pub expires_at: i64,
    session: String,
    epoch: i64,
    completion_mode: String,
    redirect_uri: Option<String>,
    ignored_fields: Vec<String>,
}

impl PendingAuthorization {
    pub fn is_host_callback(&self) -> bool {
        self.completion_mode == "host-callback"
    }
    pub fn parse_manual_response(&self, input: &str) -> anyhow::Result<Vec<(String, String)>> {
        anyhow::ensure!(
            self.completion_mode == "manual-response-uri",
            "invalid OAuth completion mode"
        );
        parse_manual_response_uri(
            input,
            self.redirect_uri
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("missing OAuth redirect"))?,
            &self.ignored_fields,
        )
    }
}

#[derive(Default)]
pub struct AuthFlowRegistry {
    flows: Mutex<HashMap<[u8; 32], StoredFlow>>,
    pending: Mutex<HashMap<(String, String), PendingAuthorization>>,
}

impl AuthFlowRegistry {
    pub fn register_pending(
        &self,
        plugin_id: &str,
        flow_id: &str,
        session: &str,
        epoch: i64,
        auth: &crate::sync::provider::AuthBegin,
        declaration: &super::contributions::PluginAuthenticationContribution,
        expires_at: i64,
    ) -> anyhow::Result<()> {
        validate_component(flow_id)?;
        anyhow::ensure!(
            declaration.modes.contains(&auth.completion_mode),
            "invalid OAuth completion mode"
        );
        let authorization =
            super::contributions::safe_external_url(&auth.authorization_url, false)?;
        anyhow::ensure!(
            declaration
                .authorization_hosts
                .iter()
                .any(|host| Some(host.as_str()) == authorization.host_str()),
            "invalid OAuth authorization host"
        );
        let query = authorization.query_pairs().into_owned().collect::<Vec<_>>();
        validate_callback_fields(&query)?;
        if let Some(redirect) = auth.redirect_uri.as_ref() {
            match auth.completion_mode.as_str() {
                "manual-response-uri" => anyhow::ensure!(
                    Some(redirect) == declaration.manual_redirect_uri.as_ref(),
                    "invalid OAuth redirect"
                ),
                "loopback" => {
                    let url = super::contributions::safe_external_url(redirect, true)?;
                    anyhow::ensure!(
                        url.scheme() == "http"
                            && url.host_str() == Some("localhost")
                            && url.port().is_some_and(|p| p > 0)
                            && url.query().is_none()
                            && Some(url.path()) == declaration.loopback_redirect_path.as_deref(),
                        "invalid OAuth loopback redirect"
                    );
                }
                "host-callback" => {}
                _ => anyhow::bail!("invalid OAuth completion mode"),
            }
            // If the provider exposes its OAuth redirect in the link, it must be the negotiated one.
            anyhow::ensure!(
                authorization
                    .query_pairs()
                    .filter(|(key, _)| key == "redirect_uri")
                    .all(|(_, value)| value == redirect.as_str()),
                "invalid OAuth redirect"
            );
        } else {
            anyhow::ensure!(
                auth.completion_mode == "host-callback",
                "missing OAuth redirect"
            );
        }
        let mut pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("auth registry unavailable"))?;
        let now = crate::host_registry::unix_timestamp_millis();
        pending.retain(|_, entry| entry.expires_at >= now);
        anyhow::ensure!(
            pending.len() < MAX_ACTIVE_AUTH_FLOWS,
            "too many active OAuth flows"
        );
        pending.retain(|(plugin, _), _| plugin != plugin_id);
        pending.insert(
            (plugin_id.into(), flow_id.into()),
            PendingAuthorization {
                authorization_url: auth.authorization_url.clone(),
                expires_at,
                session: session.into(),
                epoch,
                completion_mode: auth.completion_mode.clone(),
                redirect_uri: auth.redirect_uri.clone(),
                ignored_fields: declaration.ignored_callback_fields.clone(),
            },
        );
        Ok(())
    }
    pub fn pending(
        &self,
        plugin_id: &str,
        flow_id: &str,
        session: &str,
        epoch: i64,
    ) -> anyhow::Result<PendingAuthorization> {
        let pending = self
            .pending
            .lock()
            .map_err(|_| anyhow::anyhow!("auth registry unavailable"))?;
        let value = pending
            .get(&(plugin_id.into(), flow_id.into()))
            .ok_or_else(|| anyhow::anyhow!("OAuth flow unavailable"))?;
        anyhow::ensure!(
            value.session == session
                && value.epoch == epoch
                && value.expires_at >= crate::host_registry::unix_timestamp_millis(),
            "OAuth flow unavailable"
        );
        Ok(value.clone())
    }
    pub fn cancel_pending(&self, plugin_id: &str, flow_id: Option<&str>) {
        self.pending
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .retain(|(plugin, flow), _| {
                plugin != plugin_id || flow_id.is_some_and(|id| id != flow)
            });
    }
    pub fn cancel_host_flow(&self, flow_id: &str) -> Option<String> {
        let hash: [u8; 32] = Sha256::digest(flow_id.as_bytes()).into();
        self.flows
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&hash)
            .and_then(|flow| flow.binding.provider_flow_id)
    }
    pub fn create(&self, binding: AuthFlowBinding) -> anyhow::Result<String> {
        validate_component(&binding.plugin_id)?;
        validate_component(&binding.account_uuid)?;
        validate_component(&binding.supervisor_session)?;
        let mut id = [0u8; 32];
        OsRng.fill_bytes(&mut id);
        let external = URL_SAFE_NO_PAD.encode(id);
        let hash = Sha256::digest(external.as_bytes()).into();
        let mut flows = self
            .flows
            .lock()
            .map_err(|e| anyhow::anyhow!("auth flow lock poisoned: {e}"))?;
        let now = SystemTime::now();
        flows.retain(|_, stored| stored.expires_at >= now);
        if flows.len() >= MAX_ACTIVE_AUTH_FLOWS {
            anyhow::bail!("too many active OAuth flows")
        }
        flows.insert(
            hash,
            StoredFlow {
                binding,
                expires_at: now + AUTH_FLOW_TTL,
            },
        );
        Ok(external)
    }
    pub fn attach_provider_flow(
        &self,
        flow_id: &str,
        provider_flow_id: String,
    ) -> anyhow::Result<()> {
        validate_component(&provider_flow_id)?;
        let hash: [u8; 32] = Sha256::digest(flow_id.as_bytes()).into();
        let mut flows = self
            .flows
            .lock()
            .map_err(|e| anyhow::anyhow!("auth flow lock poisoned: {e}"))?;
        let stored = flows
            .get_mut(&hash)
            .ok_or_else(|| anyhow::anyhow!("OAuth flow is missing or already consumed"))?;
        stored.binding.provider_flow_id = Some(provider_flow_id);
        Ok(())
    }
    pub fn consume_bound(
        &self,
        flow_id: &str,
        fields: &[(String, String)],
    ) -> anyhow::Result<AuthFlowBinding> {
        if flow_id.len() != 43 || validate_callback_fields(fields).is_err() {
            anyhow::bail!("invalid OAuth callback")
        }
        let hash: [u8; 32] = Sha256::digest(flow_id.as_bytes()).into();
        let stored = self
            .flows
            .lock()
            .map_err(|e| anyhow::anyhow!("auth flow lock poisoned: {e}"))?
            .remove(&hash)
            .ok_or_else(|| anyhow::anyhow!("OAuth flow is missing or already consumed"))?;
        if SystemTime::now() > stored.expires_at {
            anyhow::bail!("OAuth flow expired")
        }
        if stored.binding.provider_flow_id.is_none() {
            anyhow::bail!("OAuth provider flow is not attached")
        }
        Ok(stored.binding)
    }
    #[cfg(test)]
    pub fn consume(
        &self,
        flow_id: &str,
        expected: &AuthFlowBinding,
        fields: &[(String, String)],
    ) -> anyhow::Result<()> {
        if flow_id.len() != 43 || validate_callback_fields(fields).is_err() {
            anyhow::bail!("invalid OAuth callback")
        }
        let hash: [u8; 32] = Sha256::digest(flow_id.as_bytes()).into();
        let stored = self
            .flows
            .lock()
            .map_err(|e| anyhow::anyhow!("auth flow lock poisoned: {e}"))?
            .remove(&hash)
            .ok_or_else(|| anyhow::anyhow!("OAuth flow is missing or already consumed"))?;
        if SystemTime::now() > stored.expires_at {
            anyhow::bail!("OAuth flow expired")
        }
        if &stored.binding != expected {
            anyhow::bail!("OAuth flow binding mismatch")
        }
        Ok(())
    }
}

pub fn parse_callback_fields(raw_query: Option<&str>) -> anyhow::Result<Vec<(String, String)>> {
    let raw_query = raw_query.unwrap_or_default();
    if raw_query.len() > MAX_CALLBACK_QUERY_BYTES || has_malformed_percent_encoding(raw_query) {
        anyhow::bail!("invalid OAuth callback")
    }
    let fields = url::form_urlencoded::parse(raw_query.as_bytes())
        .into_owned()
        .collect::<Vec<_>>();
    validate_callback_fields(&fields)?;
    Ok(fields)
}

pub fn validate_callback_fields(fields: &[(String, String)]) -> anyhow::Result<()> {
    if fields.len() > MAX_CALLBACK_FIELDS
        || fields.iter().any(|(key, value)| {
            key.is_empty()
                || key.len() > 64
                || value.len() > MAX_CALLBACK_FIELD_BYTES
                || key.chars().any(char::is_control)
                || value.chars().any(char::is_control)
        })
        || fields
            .iter()
            .map(|(key, _)| key)
            .collect::<HashSet<_>>()
            .len()
            != fields.len()
    {
        anyhow::bail!("invalid OAuth callback")
    }
    Ok(())
}

pub fn parse_manual_response_uri(
    input: &str,
    expected_redirect: &str,
    ignored_fields: &[String],
) -> anyhow::Result<Vec<(String, String)>> {
    let input = input.trim_matches(|character: char| character.is_ascii_whitespace());
    if input.is_empty()
        || input.len() > MAX_MANUAL_RESPONSE_URI_BYTES
        || input.chars().any(char::is_control)
        || input.contains('\\')
        || has_malformed_percent_encoding(input)
    {
        anyhow::bail!("invalid OAuth response URI")
    }
    let url = url::Url::parse(input).map_err(|_| anyhow::anyhow!("invalid OAuth response URI"))?;
    let expected = super::contributions::safe_external_url(expected_redirect, false)?;
    if url.scheme() != "https"
        || url.origin() != expected.origin()
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != expected.path()
    {
        anyhow::bail!("invalid OAuth response URI")
    }
    let allowed = ["code", "state", "error", "error_description", "error_uri"];
    let mut fields = url.query_pairs().into_owned().collect::<Vec<_>>();
    validate_callback_fields(&fields)?;
    if fields.iter().any(|(key, value)| {
        (!allowed.contains(&key.as_str()) && !ignored_fields.contains(key))
            || key.chars().any(char::is_control)
            || value.chars().any(char::is_control)
    }) {
        anyhow::bail!("invalid OAuth response URI")
    }
    let has_code = fields
        .iter()
        .any(|(key, value)| key == "code" && !value.is_empty());
    let has_error = fields
        .iter()
        .any(|(key, value)| key == "error" && !value.is_empty());
    let has_state = fields
        .iter()
        .any(|(key, value)| key == "state" && !value.is_empty());
    if !has_state || has_code == has_error {
        anyhow::bail!("invalid OAuth response URI")
    }
    fields.retain(|(key, _)| !ignored_fields.contains(key));
    Ok(fields)
}

fn has_malformed_percent_encoding(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            if index + 2 >= bytes.len()
                || !bytes[index + 1].is_ascii_hexdigit()
                || !bytes[index + 2].is_ascii_hexdigit()
            {
                return true;
            }
            index += 3;
        } else {
            index += 1;
        }
    }
    false
}

fn validate_component(value: &str) -> anyhow::Result<()> {
    if value.is_empty() || value.len() > 160 || value.bytes().any(|b| b.is_ascii_control()) {
        anyhow::bail!("invalid OAuth flow binding")
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generic_authorization_pins_second_provider_url_redirect_session_epoch_and_expiry() {
        let manifest: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../bundled-plugins/com.mangoo.fake-sync/plugin.template.json"
        ))
        .unwrap();
        let declaration: super::super::contributions::PluginAuthenticationContribution =
            serde_json::from_value(manifest["contributions"]["authentication"].clone()).unwrap();
        let auth=crate::sync::provider::AuthBegin{authorization_url:"https://auth.example.test/authorize?workspace=fixture&state=s&redirect_uri=https%3A%2F%2Fauth.example.test%2Freturn".into(),flow_id:"provider-flow".into(),completion_mode:"manual-response-uri".into(),redirect_uri:Some("https://auth.example.test/return".into())};
        let registry = AuthFlowRegistry::default();
        let expires = crate::host_registry::unix_timestamp_millis() + 60_000;
        registry
            .register_pending(
                "com.mangoo.fake-sync",
                "flow",
                "session",
                3,
                &auth,
                &declaration,
                expires,
            )
            .unwrap();
        let pending = registry
            .pending("com.mangoo.fake-sync", "flow", "session", 3)
            .unwrap();
        assert_eq!(pending.authorization_url, auth.authorization_url);
        assert_eq!(
            pending
                .parse_manual_response(
                    "https://auth.example.test/return?code=opaque&state=s&session_hint=ignored"
                )
                .unwrap(),
            vec![
                ("code".into(), "opaque".into()),
                ("state".into(), "s".into())
            ]
        );
        for uri in [
            "https://auth.example.test/wrong?code=x&state=s",
            "https://other.example.test/return?code=x&state=s",
            "https://auth.example.test/return?code=x&code=y&state=s",
            "https://auth.example.test/return?code=x&state=s&extra=v",
            "https://auth.example.test/return?code=%0A&state=s",
            "https://auth.example.test/return?code=%GG&state=s",
        ] {
            assert!(pending.parse_manual_response(uri).is_err());
        }
        assert!(registry.pending("other", "flow", "session", 3).is_err());
        assert!(registry
            .pending("com.mangoo.fake-sync", "wrong", "session", 3)
            .is_err());
        assert!(registry
            .pending("com.mangoo.fake-sync", "flow", "stale", 3)
            .is_err());
        assert!(registry
            .pending("com.mangoo.fake-sync", "flow", "session", 4)
            .is_err());
        let mut invalid = auth.clone();
        invalid.redirect_uri = Some("https://auth.example.test/wrong".into());
        assert!(registry
            .register_pending("p", "f", "s", 1, &invalid, &declaration, expires)
            .is_err());
        invalid = auth.clone();
        invalid.authorization_url = "https://wrong.example.test/authorize".into();
        assert!(registry
            .register_pending("p", "f", "s", 1, &invalid, &declaration, expires)
            .is_err());
        registry.cancel_pending("com.mangoo.fake-sync", Some("flow"));
        assert!(registry
            .pending("com.mangoo.fake-sync", "flow", "session", 3)
            .is_err());
        registry
            .register_pending(
                "com.mangoo.fake-sync",
                "flow",
                "session",
                3,
                &auth,
                &declaration,
                0,
            )
            .unwrap();
        assert!(registry
            .pending("com.mangoo.fake-sync", "flow", "session", 3)
            .is_err());
    }
    fn binding() -> AuthFlowBinding {
        AuthFlowBinding {
            plugin_id: "com.mangoo.fake".into(),
            account_uuid: "a".into(),
            supervisor_session: "s".into(),
            provider_flow_id: Some("provider-flow".into()),
        }
    }
    #[test]
    fn public_url_is_strict() {
        assert!(PublicUrl::parse("https://example.com/mangoo/", false).is_ok());
        assert!(PublicUrl::parse("https://u@example.com/", false).is_err());
        assert!(PublicUrl::parse("http://example.com/", true).is_err());
        assert!(PublicUrl::parse("http://localhost:8080/", true).is_ok());
        assert_eq!(
            PublicUrl::parse("https://example.com/mangoo/", false)
                .unwrap()
                .callback_url("flow_123")
                .unwrap()
                .as_str(),
            "https://example.com/mangoo/api/plugin-auth/callback/flow_123"
        );
    }
    #[test]
    fn flow_is_one_time_and_binding_scoped() {
        let registry = AuthFlowRegistry::default();
        let expected = binding();
        let id = registry.create(expected.clone()).unwrap();
        assert!(registry
            .consume(
                &id,
                &AuthFlowBinding {
                    account_uuid: "b".into(),
                    ..expected.clone()
                },
                &[]
            )
            .is_err());
        assert!(
            registry.consume(&id, &expected, &[]).is_err(),
            "binding failure consumes flow to prevent probing/replay"
        );
        let id = registry.create(expected.clone()).unwrap();
        registry
            .consume(&id, &expected, &[("code".into(), "opaque".into())])
            .unwrap();
        assert!(registry.consume(&id, &expected, &[]).is_err());
    }

    #[test]
    fn callback_query_rejects_duplicate_and_oversized_fields_before_collection() {
        assert!(parse_callback_fields(Some("code=one&code=two")).is_err());
        assert!(parse_callback_fields(Some(&format!(
            "code={}",
            "x".repeat(MAX_CALLBACK_FIELD_BYTES + 1)
        )))
        .is_err());
        assert_eq!(
            parse_callback_fields(Some("code=opaque&state=bound")).unwrap(),
            vec![
                ("code".to_owned(), "opaque".to_owned()),
                ("state".to_owned(), "bound".to_owned())
            ]
        );
    }

    #[test]
    fn manual_response_uri_is_exact_bounded_and_sanitized() {
        assert_eq!(
            parse_manual_response_uri(
                &format!("{MANUAL_REDIRECT_URI}?code=opaque&state=bound&session_state=ignored"),
                MANUAL_REDIRECT_URI,
                &["session_state".into()]
            )
            .unwrap(),
            vec![
                ("code".into(), "opaque".into()),
                ("state".into(), "bound".into())
            ]
        );
        for invalid in [
            "http://login.microsoftonline.com/common/oauth2/nativeclient?code=x&state=s",
            "https://login.microsoftonline.com.evil/common/oauth2/nativeclient?code=x&state=s",
            "https://login.microsoftonline.com/common/oauth2/nativeclient?code=x&code=y&state=s",
            "https://login.microsoftonline.com/common/oauth2/nativeclient?code=x&state=s&token=secret",
            "https://login.microsoftonline.com/common/oauth2/nativeclient?code=%GG&state=s",
        ] {
            assert!(parse_manual_response_uri(invalid,MANUAL_REDIRECT_URI,&["session_state".into()]).is_err(), "{invalid}");
        }
    }
}
