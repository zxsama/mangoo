use std::collections::HashMap;
use std::time::{Duration, Instant};

use base64::Engine;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;
use uuid::Uuid;

use crate::config::ProviderConfig;
use crate::error::ProviderError;

pub const OAUTH_SCOPES: &str = "Files.ReadWrite.AppFolder offline_access openid profile";
pub const FLOW_TTL: Duration = Duration::from_secs(10 * 60);
pub const MAX_FLOWS: usize = 32;

#[derive(Debug, Clone)]
pub struct OAuthFlow {
    pub id: String,
    pub state: String,
    pub verifier: String,
    pub redirect_uri: String,
    created_at: Instant,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CredentialRecordV1 {
    pub format_version: u8,
    pub refresh_token: String,
    pub access_token: Option<String>,
    pub access_expires_at_ms: Option<u64>,
    pub drive_id: String,
    pub account_kind: String,
    pub tenant_hint: Option<String>,
    pub object_hint: Option<String>,
    pub display_hint: Option<String>,
    pub scopes: Vec<String>,
    #[serde(default)]
    pub upload_sessions: Vec<UploadSessionSecretV1>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UploadSessionSecretV1 {
    pub secret_ref: String,
    pub purpose: String,
}

#[derive(Debug, Default)]
pub struct OAuthFlows {
    flows: HashMap<String, OAuthFlow>,
}

impl OAuthFlows {
    pub fn begin(
        &mut self,
        config: &ProviderConfig,
        callback_url: Option<&str>,
    ) -> Result<(String, String), ProviderError> {
        self.flows
            .retain(|_, flow| flow.created_at.elapsed() <= FLOW_TTL);
        if self.flows.len() >= MAX_FLOWS {
            return Err(ProviderError::new(
                "unavailable",
                true,
                "too many pending OAuth flows",
            ));
        }
        let redirect_uri = match callback_url {
            Some(value) => validate_callback(value)?,
            None => "http://localhost:53682/plugin-auth/callback".to_owned(),
        };
        let id = Uuid::now_v7().to_string();
        let state = random_urlsafe(32);
        let verifier = random_urlsafe(64);
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        let mut url = config.authorization_endpoint()?;
        url.query_pairs_mut()
            .append_pair("client_id", &config.client_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("response_mode", "query")
            .append_pair("scope", OAUTH_SCOPES)
            .append_pair("state", &state)
            .append_pair("code_challenge", &challenge)
            .append_pair("code_challenge_method", "S256");
        self.flows.insert(
            id.clone(),
            OAuthFlow {
                id: id.clone(),
                state,
                verifier,
                redirect_uri,
                created_at: Instant::now(),
            },
        );
        Ok((id, url.into()))
    }

    pub fn consume(
        &mut self,
        id: &str,
        fields: &HashMap<String, String>,
    ) -> Result<(String, String, String), ProviderError> {
        let flow = self.flows.remove(id).ok_or_else(|| {
            ProviderError::new(
                "invalid_argument",
                false,
                "OAuth flow is missing or already used",
            )
        })?;
        if flow.created_at.elapsed() > FLOW_TTL {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "OAuth flow expired",
            ));
        }
        if fields.get("state") != Some(&flow.state) {
            return Err(ProviderError::new(
                "invalid_argument",
                false,
                "OAuth state mismatch",
            ));
        }
        if let Some(error) = fields.get("error") {
            return Err(ProviderError::new(
                "unauthenticated",
                false,
                format!("Microsoft authorization failed: {error}"),
            ));
        }
        let code = fields
            .get("code")
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                ProviderError::new("invalid_argument", false, "OAuth code is missing")
            })?;
        Ok((code.clone(), flow.verifier, flow.redirect_uri))
    }

    pub fn cancel_all(&mut self) {
        self.flows.clear();
    }

    pub fn cancel(&mut self, id: &str) -> bool {
        self.flows.remove(id).is_some()
    }
}

fn validate_callback(value: &str) -> Result<String, ProviderError> {
    let url = Url::parse(value)
        .map_err(|_| ProviderError::new("invalid_argument", false, "invalid OAuth callback URL"))?;
    let loopback = url.scheme() == "http" && url.host_str() == Some("localhost");
    if url.scheme() != "https" && !loopback {
        return Err(ProviderError::new(
            "invalid_argument",
            false,
            "OAuth callback must be HTTPS or localhost loopback",
        ));
    }
    if url.username() != ""
        || url.password().is_some()
        || url.fragment().is_some()
        || url.query().is_some()
        || (loopback && (url.port().is_none() || url.path() != "/plugin-auth/callback"))
        || (!loopback && url.port().is_some_and(|port| port != 443))
    {
        return Err(ProviderError::new(
            "invalid_argument",
            false,
            "invalid OAuth callback URL",
        ));
    }
    Ok(url.into())
}

fn random_urlsafe(bytes: usize) -> String {
    let mut value = vec![0_u8; bytes];
    rand::rngs::OsRng.fill_bytes(&mut value);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> ProviderConfig {
        ProviderConfig {
            client_id: "public-client".into(),
            authority: Url::parse("https://login.microsoftonline.com/common/").unwrap(),
            graph_endpoint: Url::parse("https://graph.microsoft.com/v1.0/").unwrap(),
            allow_http_emulator: false,
            auth_mode: crate::config::AuthMode::Loopback,
        }
    }

    #[test]
    fn pkce_flow_is_single_use_and_state_bound() {
        let mut flows = OAuthFlows::default();
        let (id, authorization) = flows
            .begin(
                &config(),
                Some("https://megle.invalid/api/plugin-auth/callback/x"),
            )
            .unwrap();
        let url = Url::parse(&authorization).unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(k, _)| k == "code_challenge_method")
                .unwrap()
                .1,
            "S256"
        );
        let state = url
            .query_pairs()
            .find(|(k, _)| k == "state")
            .unwrap()
            .1
            .into_owned();
        let fields = HashMap::from([("state".into(), state), ("code".into(), "one-time".into())]);
        assert_eq!(flows.consume(&id, &fields).unwrap().0, "one-time");
        assert!(flows.consume(&id, &fields).is_err());
    }

    #[test]
    fn callback_rejects_query_fragment_credentials_and_unsafe_loopback_forms() {
        for value in [
            "http://127.0.0.1/plugin-auth/callback",
            "http://127.0.0.1:53682/plugin-auth/callback",
            "http://localhost/plugin-auth/callback",
            "http://localhost:53682/other",
            "http://user@localhost:53682/plugin-auth/callback",
            "http://localhost:53682/plugin-auth/callback?code=unexpected",
            "http://localhost:53682/plugin-auth/callback#fragment",
            "https://user@example.invalid/plugin-auth/callback",
            "https://example.invalid/plugin-auth/callback?code=secret",
            "https://example.invalid/plugin-auth/callback#fragment",
            "https://example.invalid:444/plugin-auth/callback",
        ] {
            assert!(validate_callback(value).is_err(), "accepted {value}");
        }
        assert!(validate_callback("http://localhost:53682/plugin-auth/callback").is_ok());
        assert!(validate_callback("https://example.invalid/plugin-auth/callback").is_ok());
    }

    #[test]
    fn pending_oauth_flows_are_bounded() {
        let mut flows = OAuthFlows::default();
        for _ in 0..MAX_FLOWS {
            flows
                .begin(
                    &config(),
                    Some("https://megle.invalid/api/plugin-auth/callback/x"),
                )
                .unwrap();
        }
        assert!(flows
            .begin(
                &config(),
                Some("https://megle.invalid/api/plugin-auth/callback/x")
            )
            .is_err());
    }
}
