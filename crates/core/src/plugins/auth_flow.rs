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
            anyhow::bail!("MEGLE_PUBLIC_URL must not contain userinfo, query or fragment");
        }
        let secure = url.scheme() == "https";
        let loopback_http = allow_loopback_http
            && url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "::1"));
        if !secure && !loopback_http {
            anyhow::bail!(
                "MEGLE_PUBLIC_URL must use HTTPS (HTTP is limited to loopback development)"
            );
        }
        if url.host_str().is_none() {
            anyhow::bail!("MEGLE_PUBLIC_URL must be an absolute URL");
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

#[derive(Default)]
pub struct AuthFlowRegistry {
    flows: Mutex<HashMap<[u8; 32], StoredFlow>>,
}

impl AuthFlowRegistry {
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
    pub fn purge_expired(&self) -> usize {
        let Ok(mut flows) = self.flows.lock() else {
            return 0;
        };
        let before = flows.len();
        let now = SystemTime::now();
        flows.retain(|_, v| v.expires_at >= now);
        before - flows.len()
    }
}

pub fn parse_callback_fields(raw_query: Option<&str>) -> anyhow::Result<Vec<(String, String)>> {
    let raw_query = raw_query.unwrap_or_default();
    if raw_query.len() > MAX_CALLBACK_QUERY_BYTES {
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
            key.is_empty() || key.len() > 64 || value.len() > MAX_CALLBACK_FIELD_BYTES
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

pub fn parse_manual_response_uri(input: &str) -> anyhow::Result<Vec<(String, String)>> {
    let input = input.trim_matches(|character: char| character.is_ascii_whitespace());
    if input.is_empty()
        || input.len() > MAX_MANUAL_RESPONSE_URI_BYTES
        || input.chars().any(char::is_control)
        || has_malformed_percent_encoding(input)
    {
        anyhow::bail!("invalid OAuth response URI")
    }
    let url = url::Url::parse(input).map_err(|_| anyhow::anyhow!("invalid OAuth response URI"))?;
    if url.scheme() != "https"
        || url.host_str() != Some("login.microsoftonline.com")
        || url.port_or_known_default() != Some(443)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
        || url.path() != "/common/oauth2/nativeclient"
    {
        anyhow::bail!("invalid OAuth response URI")
    }
    let allowed = [
        "code",
        "state",
        "error",
        "error_description",
        "error_uri",
        "session_state",
    ];
    let mut fields = url.query_pairs().into_owned().collect::<Vec<_>>();
    validate_callback_fields(&fields)?;
    if fields
        .iter()
        .any(|(key, _)| !allowed.contains(&key.as_str()))
    {
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
    fields.retain(|(key, _)| key != "session_state");
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
    fn binding() -> AuthFlowBinding {
        AuthFlowBinding {
            plugin_id: "com.megle.fake".into(),
            account_uuid: "a".into(),
            supervisor_session: "s".into(),
            provider_flow_id: Some("provider-flow".into()),
        }
    }
    #[test]
    fn public_url_is_strict() {
        assert!(PublicUrl::parse("https://example.com/megle/", false).is_ok());
        assert!(PublicUrl::parse("https://u@example.com/", false).is_err());
        assert!(PublicUrl::parse("http://example.com/", true).is_err());
        assert!(PublicUrl::parse("http://localhost:8080/", true).is_ok());
        assert_eq!(
            PublicUrl::parse("https://example.com/megle/", false)
                .unwrap()
                .callback_url("flow_123")
                .unwrap()
                .as_str(),
            "https://example.com/megle/api/plugin-auth/callback/flow_123"
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
            parse_manual_response_uri(&format!(
                "{MANUAL_REDIRECT_URI}?code=opaque&state=bound&session_state=ignored"
            ))
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
            assert!(parse_manual_response_uri(invalid).is_err(), "{invalid}");
        }
    }
}
