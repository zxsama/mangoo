use std::fmt;

use serde_json::{json, Value};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderError {
    pub code: &'static str,
    pub retryable: bool,
    pub retry_after_ms: Option<u64>,
    pub detail: String,
}

impl ProviderError {
    pub fn new(code: &'static str, retryable: bool, detail: impl Into<String>) -> Self {
        Self {
            code,
            retryable,
            retry_after_ms: None,
            detail: sanitize_detail(&detail.into()),
        }
    }

    pub fn configuration(detail: impl Into<String>) -> Self {
        Self::new("precondition_failed", false, detail)
    }

    pub fn protocol(detail: impl Into<String>) -> Self {
        Self::new("protocol_error", false, detail)
    }

    pub fn to_payload(&self) -> Value {
        json!({
            "code": self.code,
            "retryable": self.retryable,
            "retryAfterMs": self.retry_after_ms,
            "detail": self.detail
        })
    }
}

impl fmt::Display for ProviderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.detail)
    }
}

impl std::error::Error for ProviderError {}

pub fn sanitize_detail(input: &str) -> String {
    let mut safe = input.to_owned();
    for marker in [
        "access_token",
        "refresh_token",
        "code_verifier",
        "client_secret",
        "Authorization",
    ] {
        safe = safe.replace(marker, "[redacted]");
    }
    if let Some((prefix, _)) = safe.split_once('?') {
        safe = format!("{prefix}?[redacted]");
    }
    safe.chars().take(512).collect()
}

pub fn map_http_error(
    status: u16,
    graph_code: Option<&str>,
    retry_after_ms: Option<u64>,
) -> ProviderError {
    if graph_code.is_some_and(|code| {
        matches!(
            code.to_ascii_lowercase().as_str(),
            "invalid_grant" | "invalidauthenticationtoken" | "tokenexpired" | "tokenrevoked"
        )
    }) {
        return ProviderError::new(
            "unauthenticated",
            false,
            "Microsoft authorization is required",
        );
    }
    let (code, retryable, detail) = match status {
        401 => (
            "unauthenticated",
            false,
            "Microsoft authorization is required",
        ),
        403 => (
            "permission_denied",
            false,
            "Microsoft Graph permission was denied",
        ),
        404 => ("not_found", false, "OneDrive object was not found"),
        409 | 412 | 416 => ("precondition_failed", false, "OneDrive precondition failed"),
        410 => ("cursor_reset", true, "OneDrive delta cursor must be reset"),
        429 => ("throttled", true, "Microsoft Graph throttled the request"),
        507 => ("precondition_failed", false, "OneDrive quota is exhausted"),
        500 | 502 | 503 | 504 | 509 => ("unavailable", true, "Microsoft Graph is unavailable"),
        _ => ("internal", false, "Microsoft Graph request failed"),
    };
    let mut error = ProviderError::new(
        code,
        retryable,
        match graph_code {
            Some(value) => format!("{detail} ({})", sanitize_detail(value)),
            None => detail.into(),
        },
    );
    error.retry_after_ms = retry_after_ms;
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_graph_status_and_redacts_query_capabilities() {
        let error = map_http_error(429, Some("tooManyRequests"), Some(3_000));
        assert_eq!(error.code, "throttled");
        assert_eq!(error.retry_after_ms, Some(3_000));
        assert_eq!(
            sanitize_detail("https://upload.invalid/a?token=secret"),
            "https://upload.invalid/a?[redacted]"
        );
    }

    #[test]
    fn maps_the_complete_provider_http_error_matrix() {
        for (status, code, retryable) in [
            (401, "unauthenticated", false),
            (403, "permission_denied", false),
            (404, "not_found", false),
            (409, "precondition_failed", false),
            (412, "precondition_failed", false),
            (416, "precondition_failed", false),
            (429, "throttled", true),
            (500, "unavailable", true),
            (502, "unavailable", true),
            (503, "unavailable", true),
            (504, "unavailable", true),
            (507, "precondition_failed", false),
            (509, "unavailable", true),
            (410, "cursor_reset", true),
        ] {
            let error = map_http_error(status, None, Some(1_000));
            assert_eq!(error.code, code, "status {status}");
            assert_eq!(error.retryable, retryable, "status {status}");
            assert_eq!(error.retry_after_ms, Some(1_000), "status {status}");
        }
        assert_eq!(
            map_http_error(400, Some("invalid_grant"), None).code,
            "unauthenticated"
        );
    }
}
