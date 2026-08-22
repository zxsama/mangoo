use std::env;

use url::Url;

use crate::error::ProviderError;

const DEFAULT_AUTHORITY: &str = "https://login.microsoftonline.com/common/";
const DEFAULT_GRAPH_ENDPOINT: &str = "https://graph.microsoft.com/v1.0/";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderConfig {
    pub client_id: String,
    pub authority: Url,
    pub graph_endpoint: Url,
    pub allow_http_emulator: bool,
}

impl ProviderConfig {
    pub fn from_env() -> Result<Self, ProviderError> {
        let client_id = env::var("MEGLE_ONEDRIVE_CLIENT_ID")
            .ok()
            .filter(|value| !value.trim().is_empty())
            .ok_or_else(|| {
                ProviderError::configuration("Microsoft public client ID is not configured")
            })?;
        let emulator = env::var("MEGLE_ONEDRIVE_EMULATOR_URL")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let allow_http_emulator =
            (cfg!(debug_assertions) || cfg!(feature = "graph-emulator")) && emulator.is_some();
        let authority = parse_endpoint(
            &env::var("MEGLE_ONEDRIVE_AUTHORITY").unwrap_or_else(|_| DEFAULT_AUTHORITY.into()),
            allow_http_emulator,
            "authority",
        )?;
        let graph_endpoint = parse_endpoint(
            emulator.as_deref().unwrap_or(DEFAULT_GRAPH_ENDPOINT),
            allow_http_emulator,
            "Graph endpoint",
        )?;
        Ok(Self {
            client_id,
            authority,
            graph_endpoint,
            allow_http_emulator,
        })
    }

    pub fn authorization_endpoint(&self) -> Result<Url, ProviderError> {
        self.authority
            .join("oauth2/v2.0/authorize")
            .map_err(|_| ProviderError::configuration("invalid Microsoft authority"))
    }

    pub fn token_endpoint(&self) -> Result<Url, ProviderError> {
        self.authority
            .join("oauth2/v2.0/token")
            .map_err(|_| ProviderError::configuration("invalid Microsoft authority"))
    }
}

fn parse_endpoint(value: &str, allow_http: bool, label: &str) -> Result<Url, ProviderError> {
    let mut url =
        Url::parse(value).map_err(|_| ProviderError::configuration(format!("invalid {label}")))?;
    if url.scheme() != "https" && !(allow_http && url.scheme() == "http") {
        return Err(ProviderError::configuration(format!(
            "{label} must use HTTPS"
        )));
    }
    if url.username() != ""
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(ProviderError::configuration(format!("invalid {label}")));
    }
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path());
        url.set_path(&path);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn authority_endpoints_are_bounded_and_secret_free() {
        let config = ProviderConfig {
            client_id: "public-client".into(),
            authority: Url::parse(DEFAULT_AUTHORITY).unwrap(),
            graph_endpoint: Url::parse(DEFAULT_GRAPH_ENDPOINT).unwrap(),
            allow_http_emulator: false,
        };
        assert_eq!(
            config.authorization_endpoint().unwrap().as_str(),
            "https://login.microsoftonline.com/common/oauth2/v2.0/authorize"
        );
        assert!(parse_endpoint("http://graph.invalid/v1.0", false, "Graph").is_err());
    }
}
