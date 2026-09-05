use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
#[cfg(test)]
use sha2::{Digest, Sha256};

pub const MAX_PLUGIN_ID_LEN: usize = 255;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ManifestRecord {
    pub manifest_version: u8,
    pub id: String,
    pub name: String,
    pub version: String,
    pub engine: Engine,
    pub entry: PlatformEntries,
    pub rpc: RpcDeclaration,
    pub activation_events: Vec<ActivationEvent>,
    pub requested_permissions: Vec<Permission>,
    pub integrity: Integrity,
    #[serde(skip)]
    pub manifest_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    Process,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformEntries {
    pub windows_x64: Option<String>,
    pub linux_x64: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RpcDeclaration {
    pub wire_versions: Vec<u16>,
    pub capabilities: Vec<Capability>,
    pub method_versions: BTreeMap<String, u16>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "sync-provider.v1")]
    SyncProviderV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationEvent {
    SyncEnabled,
    AuthRequested,
    DiagnosticsRequested,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Permission {
    Network {
        schemes: Vec<NetworkScheme>,
        hosts: Vec<String>,
    },
    OauthLoopback {
        mode: LoopbackMode,
    },
    Secret {
        namespace: String,
        operations: Vec<SecretOperation>,
    },
    TempStorage {
        #[serde(rename = "maxBytes")]
        max_bytes: u64,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NetworkScheme {
    Https,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LoopbackMode {
    EphemeralOnly,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretOperation {
    Read,
    Write,
    Delete,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub sha256: String,
}

#[derive(Debug)]
pub enum ManifestError {
    Io(std::io::Error),
    Json(serde_json::Error),
    Invalid(&'static str),
}
impl fmt::Display for ManifestError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(e) => write!(f, "failed to read plugin manifest: {e}"),
            Self::Json(e) => write!(f, "plugin manifest is not valid v2 JSON: {e}"),
            Self::Invalid(e) => write!(f, "invalid plugin manifest: {e}"),
        }
    }
}
impl std::error::Error for ManifestError {}
impl From<std::io::Error> for ManifestError {
    fn from(v: std::io::Error) -> Self {
        Self::Io(v)
    }
}
impl From<serde_json::Error> for ManifestError {
    fn from(v: serde_json::Error) -> Self {
        Self::Json(v)
    }
}

pub fn parse_manifest(path: &Path) -> Result<ManifestRecord, ManifestError> {
    let bytes = fs::read(path)?;
    let raw: serde_json::Value = serde_json::from_slice(&bytes)?;
    let entries = raw
        .get("entry")
        .and_then(|value| value.as_object())
        .ok_or(ManifestError::Invalid("entry must be an object"))?;
    if entries.is_empty() || entries.values().any(serde_json::Value::is_null) {
        return Err(ManifestError::Invalid("platform entries cannot be null"));
    }
    let mut manifest: ManifestRecord = serde_json::from_value(raw)?;
    validate(&manifest)?;
    manifest.manifest_path = path.to_path_buf();
    Ok(manifest)
}

fn validate(m: &ManifestRecord) -> Result<(), ManifestError> {
    if m.manifest_version != 2 {
        return Err(ManifestError::Invalid("manifestVersion must be 2"));
    }
    if !valid_id(&m.id) {
        return Err(ManifestError::Invalid("invalid reverse-domain plugin id"));
    }
    if m.name.trim().is_empty() || m.name.len() > 128 {
        return Err(ManifestError::Invalid("invalid name"));
    }
    if !valid_semver(&m.version) {
        return Err(ManifestError::Invalid("version must be semantic version"));
    }
    if m.entry.windows_x64.is_none() && m.entry.linux_x64.is_none() {
        return Err(ManifestError::Invalid(
            "at least one platform entry is required",
        ));
    }
    for entry in [&m.entry.windows_x64, &m.entry.linux_x64]
        .into_iter()
        .flatten()
    {
        validate_relative_entry(Path::new(entry))?;
    }
    if m.rpc.wire_versions.is_empty()
        || m.rpc.wire_versions.contains(&0)
        || !unique(&m.rpc.wire_versions)
    {
        return Err(ManifestError::Invalid(
            "wire versions must be unique and nonzero",
        ));
    }
    if m.rpc.capabilities.is_empty() || !unique(&m.rpc.capabilities) {
        return Err(ManifestError::Invalid(
            "capabilities must be unique and non-empty",
        ));
    }
    if m.rpc.method_versions != required_sync_provider_method_versions() {
        return Err(ManifestError::Invalid(
            "sync-provider method versions must match the v1 contract",
        ));
    }
    if m.activation_events.is_empty() || !unique(&m.activation_events) {
        return Err(ManifestError::Invalid("activation events must be unique"));
    }
    if m.integrity.sha256.len() != 64
        || !m
            .integrity
            .sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(ManifestError::Invalid(
            "integrity sha256 must be 64 hex characters",
        ));
    }
    for permission in &m.requested_permissions {
        validate_permission(permission)?;
    }
    Ok(())
}

pub fn required_sync_provider_method_versions() -> BTreeMap<String, u16> {
    [
        "describe",
        "health",
        "diagnostics",
        "shutdown",
        "auth.begin",
        "auth.complete",
        "auth.poll",
        "auth.cancel",
        "auth.status",
        "auth.sign_out",
        "delta.list",
        "object.head",
        "object.get",
        "object.get_range",
        "object.put_immutable",
        "upload.begin",
        "upload.write",
        "upload.commit",
        "upload.abort",
    ]
    .into_iter()
    .map(|method| (method.to_owned(), 1))
    .collect()
}
fn unique<T: Eq + std::hash::Hash>(values: &[T]) -> bool {
    let mut s = HashSet::new();
    values.iter().all(|v| s.insert(v))
}
fn valid_id(v: &str) -> bool {
    if v.is_empty() || v.len() > MAX_PLUGIN_ID_LEN || !v.contains('.') {
        return false;
    }
    v.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .next()
                .is_some_and(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
            && part
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    })
}
fn valid_semver(v: &str) -> bool {
    v.len() <= 128 && semver::Version::parse(v).is_ok()
}
pub fn validate_relative_entry(path: &Path) -> Result<(), ManifestError> {
    let value = path
        .to_str()
        .ok_or(ManifestError::Invalid("entry must be valid UTF-8"))?;
    let has_drive_prefix =
        value.as_bytes().get(1) == Some(&b':') && value.as_bytes()[0].is_ascii_alphabetic();
    if value.is_empty()
        || value.len() > 512
        || value.starts_with(['/', '\\'])
        || has_drive_prefix
        || value.contains('\0')
        || value.split(['/', '\\']).any(|component| component == "..")
    {
        return Err(ManifestError::Invalid(
            "entry must be a normalized relative path",
        ));
    }
    Ok(())
}
fn validate_permission(p: &Permission) -> Result<(), ManifestError> {
    match p {
        Permission::Network { schemes, hosts }
            if schemes.is_empty() || hosts.is_empty() || !unique(schemes) || !unique(hosts) =>
        {
            Err(ManifestError::Invalid("invalid network permission"))
        }
        Permission::Network { hosts, .. } if hosts.iter().any(|host| !valid_dns_name(host)) => {
            Err(ManifestError::Invalid("network hosts must be DNS names"))
        }
        Permission::Secret {
            namespace,
            operations,
        } if !valid_secret_namespace(namespace) || operations.is_empty() || !unique(operations) => {
            Err(ManifestError::Invalid("invalid secret permission"))
        }
        Permission::TempStorage { max_bytes }
            if *max_bytes == 0 || *max_bytes > 1024 * 1024 * 1024 =>
        {
            Err(ManifestError::Invalid("invalid temp storage limit"))
        }
        _ => Ok(()),
    }
}

fn valid_dns_name(host: &str) -> bool {
    host.len() <= 253
        && host.split('.').count() >= 2
        && host.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && label
                    .bytes()
                    .next()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
                && label
                    .bytes()
                    .last()
                    .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

fn valid_secret_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && namespace.len() <= 64
        && namespace
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        && namespace
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Platform {
    WindowsX64,
    LinuxX64,
}
impl ManifestRecord {
    pub fn entry_for(&self, platform: Platform) -> Option<&str> {
        match platform {
            Platform::WindowsX64 => self.entry.windows_x64.as_deref(),
            Platform::LinuxX64 => self.entry.linux_x64.as_deref(),
        }
    }
}
#[cfg(test)]
pub fn sha256_file(path: &Path) -> Result<[u8; 32], std::io::Error> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash)?;
    Ok(hash.finalize().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn valid() -> &'static str {
        r#"{"manifestVersion":2,"id":"com.megle.test","name":"Test","version":"1.2.3","engine":"process","entry":{"windowsX64":"bin/provider.exe","linuxX64":"bin/provider"},"rpc":{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.poll":1,"auth.cancel":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}},"activationEvents":["sync-enabled"],"requestedPermissions":[{"kind":"network","schemes":["https"],"hosts":["graph.microsoft.com"]}],"integrity":{"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}}"#
    }
    #[test]
    fn strict_v2_manifest_decodes() {
        let m: ManifestRecord = serde_json::from_str(valid()).unwrap();
        validate(&m).unwrap();
    }
    #[test]
    fn rejects_unknown_and_v1() {
        assert!(serde_json::from_str::<ManifestRecord>(
            &valid().replace("\"name\":\"Test\"", "\"name\":\"Test\",\"extra\":1")
        )
        .is_err());
        assert!(validate(
            &serde_json::from_str::<ManifestRecord>(
                &valid().replace("\"manifestVersion\":2", "\"manifestVersion\":1")
            )
            .unwrap()
        )
        .is_err());
    }
    #[test]
    fn rejects_entry_escape_and_bad_semver() {
        assert!(validate_relative_entry(Path::new("../evil")).is_err());
        assert!(validate_relative_entry(Path::new("..\\evil")).is_err());
        assert!(validate_relative_entry(Path::new("C:\\evil")).is_err());
        assert!(!valid_semver("01.2.3"));
        assert!(!valid_semver("1.2.3-"));
        assert!(!valid_semver("1.2.3-alpha..1"));
    }

    #[test]
    fn permission_validation_matches_schema_constraints() {
        assert!(!valid_dns_name("LOCALHOST"));
        assert!(!valid_dns_name("localhost"));
        assert!(!valid_dns_name("-bad.example"));
        assert!(valid_dns_name("graph.microsoft.com"));
        assert!(!valid_secret_namespace("Bad_Namespace"));
        assert!(valid_secret_namespace("fake-sync"));
    }

    #[test]
    fn contract_fixture_matches_decoder() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../contracts/plugins/fixtures/valid-sync-provider-v2.json");
        let manifest = parse_manifest(&fixture).expect("shared v2 fixture");
        assert_eq!(manifest.id, "com.megle.fake-sync");
    }
}
