use std::collections::{BTreeMap, HashSet};
use std::fmt;
#[cfg(test)]
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
    /// Process engine only. A Host-engine plugin has no executable, so the
    /// field is absent on the wire and empty here.
    #[serde(default, skip_serializing_if = "PlatformEntries::is_empty")]
    pub entry: PlatformEntries,
    /// Process engine only: the executable's framed RPC contract.
    #[serde(default, skip_serializing_if = "RpcDeclaration::is_empty")]
    pub rpc: RpcDeclaration,
    /// Host engine only: the Host-implemented capabilities the plugin turns on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostDeclaration>,
    pub activation_events: Vec<ActivationEvent>,
    pub requested_permissions: Vec<Permission>,
    /// Process engine only: the descriptive executable digest.
    #[serde(default, skip_serializing_if = "Integrity::is_empty")]
    pub integrity: Integrity,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub contributions: Option<super::contributions::PluginContributions>,
    #[serde(skip)]
    pub manifest_path: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    /// A trusted bundled native executable spoken to over framed RPC.
    Process,
    /// A trusted bundled feature implemented by the Host itself (Core, Desktop
    /// and Web). The artifact is only its manifest: Core pins that manifest in
    /// compiled trust, and the ordinary plugin lifecycle (enable, grants,
    /// disable) gates the Host services its declared capabilities name.
    Host,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PlatformEntries {
    pub windows_x64: Option<String>,
    pub linux_x64: Option<String>,
}

impl PlatformEntries {
    pub fn is_empty(&self) -> bool {
        self.windows_x64.is_none() && self.linux_x64.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostDeclaration {
    pub capabilities: Vec<HostCapability>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum HostCapability {
    /// Standalone image viewer: path-scoped image services in Core, a separate
    /// Desktop viewer window and optional OS file-association registration.
    #[serde(rename = "image-viewer.v1")]
    ImageViewerV1,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RpcDeclaration {
    pub wire_versions: Vec<u16>,
    pub capabilities: Vec<Capability>,
    pub method_versions: BTreeMap<String, u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decoder: Option<DecoderDeclaration>,
}

impl RpcDeclaration {
    pub fn is_empty(&self) -> bool {
        self.wire_versions.is_empty()
            && self.capabilities.is_empty()
            && self.method_versions.is_empty()
            && self.decoder.is_none()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DecoderDeclaration {
    pub implementation_version: String,
    pub extensions: Vec<String>,
    pub purposes: Vec<DecoderPurpose>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DecoderPurpose {
    Thumbnail,
    Preview,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability {
    #[serde(rename = "sync-provider.v1")]
    SyncProviderV1,
    #[serde(rename = "file-decoder.v1")]
    FileDecoderV1,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ActivationEvent {
    SyncEnabled,
    AuthRequested,
    DiagnosticsRequested,
    FileDecodeRequested,
    /// Host engine: the user opened a file that the plugin's capability handles.
    FileOpenRequested,
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
    HostUi {
        operations: Vec<HostUiOperation>,
    },
    /// Host engine: read the image files the user explicitly opens, and the
    /// sibling images of their folder, by absolute path.
    LocalFiles {
        access: LocalFileAccess,
    },
    /// Host engine: register the application with the operating system as a
    /// handler for the capability's file types (current user only).
    FileAssociation {
        scope: FileAssociationScope,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LocalFileAccess {
    OpenedFolders,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum FileAssociationScope {
    CurrentUser,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HostUiOperation {
    CoreApi,
    PickFolder,
    RevealPath,
    ClipboardWrite,
    ExternalLink,
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Integrity {
    pub sha256: String,
}

impl Integrity {
    pub fn is_empty(&self) -> bool {
        self.sha256.is_empty()
    }
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
    use std::io::Read;
    let parent = path
        .parent()
        .ok_or(ManifestError::Invalid("manifest has no parent"))?;
    let directory = crate::library_path::MarkerDirectoryGuard::open_readonly(parent)?;
    let leaf = path
        .file_name()
        .ok_or(ManifestError::Invalid("manifest has no name"))?;
    let file = directory.open_marker(leaf, false)?;
    let mut bytes = Vec::new();
    file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    directory.verify_unchanged()?;
    parse_manifest_bytes(&bytes, path)
}

pub(crate) fn parse_manifest_bytes(
    bytes: &[u8],
    path: &Path,
) -> Result<ManifestRecord, ManifestError> {
    if bytes.len() > 256 * 1024 {
        return Err(ManifestError::Invalid("plugin manifest is too large"));
    }
    let raw: serde_json::Value = serde_json::from_slice(bytes)?;
    if raw.get("engine").and_then(|value| value.as_str()) == Some("host") {
        // Serde defaults must never stand in for a present-but-empty process
        // declaration: a Host plugin omits every process-only field.
        if ["entry", "rpc", "integrity"]
            .iter()
            .any(|field| raw.get(*field).is_some())
        {
            return Err(ManifestError::Invalid(
                "host engine manifests cannot declare process fields",
            ));
        }
    } else {
        let entries = raw
            .get("entry")
            .and_then(|value| value.as_object())
            .ok_or(ManifestError::Invalid("entry must be an object"))?;
        if entries.is_empty() || entries.values().any(serde_json::Value::is_null) {
            return Err(ManifestError::Invalid("platform entries cannot be null"));
        }
        if raw.get("rpc").is_none() || raw.get("integrity").is_none() {
            return Err(ManifestError::Invalid(
                "process manifests require rpc and integrity",
            ));
        }
    }
    let mut manifest: ManifestRecord = serde_json::from_value(raw)?;
    validate(&manifest)?;
    manifest.manifest_path = path.to_path_buf();
    Ok(manifest)
}

fn is_host_engine_permission(permission: &Permission) -> bool {
    matches!(
        permission,
        Permission::LocalFiles { .. } | Permission::FileAssociation { .. }
    )
}

/// A Host-engine plugin owns no executable, RPC contract or contributions: its
/// feature is compiled into the Host, which the trusted manifest turns on.
fn validate_host_engine(m: &ManifestRecord) -> Result<(), ManifestError> {
    if !m.entry.is_empty() || !m.rpc.is_empty() || !m.integrity.is_empty() {
        return Err(ManifestError::Invalid(
            "host engine manifests cannot declare process fields",
        ));
    }
    if m.contributions.is_some() {
        return Err(ManifestError::Invalid(
            "host engine manifests cannot declare contributions",
        ));
    }
    let Some(host) = &m.host else {
        return Err(ManifestError::Invalid(
            "host engine manifests require host capabilities",
        ));
    };
    if host.capabilities.is_empty() || !unique(&host.capabilities) {
        return Err(ManifestError::Invalid(
            "host capabilities must be unique and non-empty",
        ));
    }
    if m.activation_events.is_empty()
        || !unique(&m.activation_events)
        || m.activation_events.iter().any(|event| {
            !matches!(
                event,
                ActivationEvent::FileOpenRequested | ActivationEvent::DiagnosticsRequested
            )
        })
    {
        return Err(ManifestError::Invalid(
            "invalid host engine activation events",
        ));
    }
    if m.requested_permissions
        .iter()
        .any(|permission| !is_host_engine_permission(permission))
        || m.requested_permissions
            .iter()
            .enumerate()
            .any(|(index, permission)| m.requested_permissions[..index].contains(permission))
    {
        return Err(ManifestError::Invalid("invalid host engine permission"));
    }
    Ok(())
}

fn validate(m: &ManifestRecord) -> Result<(), ManifestError> {
    if let Some(contributions) = &m.contributions {
        contributions
            .validate(&m.requested_permissions)
            .map_err(|_| ManifestError::Invalid("invalid plugin contributions"))?;
    }
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
    if m.engine == Engine::Host {
        return validate_host_engine(m);
    }
    if m.host.is_some()
        || m.requested_permissions
            .iter()
            .any(is_host_engine_permission)
        || m.activation_events
            .contains(&ActivationEvent::FileOpenRequested)
    {
        return Err(ManifestError::Invalid(
            "host capabilities, permissions and activation require the host engine",
        ));
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
    let decoder_only = m.rpc.capabilities == [Capability::FileDecoderV1];
    let provider_only = m.rpc.capabilities == [Capability::SyncProviderV1];
    if !decoder_only && !provider_only {
        return Err(ManifestError::Invalid("unsupported capability combination"));
    }
    let required_methods = if decoder_only {
        required_file_decoder_method_versions()
    } else {
        required_sync_provider_method_versions()
    };
    if m.rpc.method_versions != required_methods {
        return Err(ManifestError::Invalid(
            "method versions must match the capability v1 contract",
        ));
    }
    match (&m.rpc.decoder, decoder_only) {
        (Some(decoder), true) => {
            if !valid_semver(&decoder.implementation_version)
                || decoder.extensions.is_empty()
                || decoder.extensions.len() > 64
                || !unique(&decoder.extensions)
                || decoder.extensions.iter().any(|extension| {
                    extension.is_empty()
                        || extension.len() > 16
                        || !extension
                            .bytes()
                            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit())
                })
                || decoder.purposes.is_empty()
                || !unique(&decoder.purposes)
                || m.activation_events.iter().any(|event| {
                    !matches!(
                        event,
                        ActivationEvent::FileDecodeRequested
                            | ActivationEvent::DiagnosticsRequested
                    )
                })
                || m.requested_permissions.iter().any(|permission| {
                    !matches!(permission,
                    Permission::TempStorage { max_bytes } if *max_bytes <= 256 * 1024 * 1024)
                })
                || m.contributions.is_some()
            {
                return Err(ManifestError::Invalid("invalid file decoder declaration"));
            }
        }
        (None, false) => {
            if m.activation_events
                .contains(&ActivationEvent::FileDecodeRequested)
            {
                return Err(ManifestError::Invalid(
                    "decoder activation requires decoder capability",
                ));
            }
        }
        _ => {
            return Err(ManifestError::Invalid(
                "decoder declaration and capability must agree",
            ))
        }
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
    if m.requested_permissions
        .iter()
        .enumerate()
        .any(|(index, permission)| m.requested_permissions[..index].contains(permission))
    {
        return Err(ManifestError::Invalid("duplicate requested permission"));
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

pub fn required_file_decoder_method_versions() -> BTreeMap<String, u16> {
    ["health", "diagnostics", "shutdown", "decoder.decode"]
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
        Permission::HostUi { operations }
            if operations.is_empty() || operations.len() > 5 || !unique(operations) =>
        {
            Err(ManifestError::Invalid("invalid host UI permission"))
        }
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
        r#"{"manifestVersion":2,"id":"com.mangoo.test","name":"Test","version":"1.2.3","engine":"process","entry":{"windowsX64":"bin/provider.exe","linuxX64":"bin/provider"},"rpc":{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.poll":1,"auth.cancel":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}},"activationEvents":["sync-enabled"],"requestedPermissions":[{"kind":"network","schemes":["https"],"hosts":["graph.microsoft.com"]}],"integrity":{"sha256":"0000000000000000000000000000000000000000000000000000000000000000"}}"#
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
            .join("../../contracts/plugins/fixtures/valid-sync-provider-v2.json")
            .canonicalize()
            .unwrap();
        let manifest = parse_manifest(&fixture).expect("shared v2 fixture");
        assert_eq!(manifest.id, "com.mangoo.fake-sync");
    }

    #[test]
    fn host_engine_manifest_is_strict_and_process_free() {
        let fixture =
            include_str!("../../../../contracts/plugins/fixtures/valid-host-image-viewer-v2.json");
        let parse = |text: &str| parse_manifest_bytes(text.as_bytes(), Path::new("plugin.json"));
        let manifest = parse(fixture).expect("host engine fixture");
        assert_eq!(manifest.engine, Engine::Host);
        assert_eq!(
            manifest.host.as_ref().unwrap().capabilities,
            [HostCapability::ImageViewerV1]
        );
        assert!(manifest.entry.is_empty() && manifest.rpc.is_empty());
        // Round-trips without growing process-only fields.
        let serialized = serde_json::to_value(&manifest).unwrap();
        for field in ["entry", "rpc", "integrity"] {
            assert!(serialized.get(field).is_none(), "{field} must stay absent");
        }
        let value: serde_json::Value = serde_json::from_str(fixture).unwrap();
        let with = |key: &str, extra: serde_json::Value| {
            let mut copy = value.clone();
            copy[key] = extra;
            serde_json::to_string(&copy).unwrap()
        };
        // Process-only declarations are rejected even when empty.
        assert!(parse(&with("entry", serde_json::json!({}))).is_err());
        assert!(parse(&with("integrity", serde_json::json!({"sha256": ""}))).is_err());
        // Host plugins cannot request process permissions or activation.
        assert!(parse(&with(
            "requestedPermissions",
            serde_json::json!([{"kind": "temp-storage", "maxBytes": 1}])
        ))
        .is_err());
        assert!(parse(&with(
            "activationEvents",
            serde_json::json!(["sync-enabled"])
        ))
        .is_err());
        assert!(parse(&with("host", serde_json::json!({"capabilities": []}))).is_err());
        assert!(parse(&with(
            "host",
            serde_json::json!({"capabilities": ["unknown.v1"]})
        ))
        .is_err());
        assert!(parse(&with(
            "contributions",
            serde_json::json!({"configuration": [], "authentication": null, "presentation": null, "primaryCredential": null})
        ))
        .is_err());
        // A process manifest cannot borrow Host capabilities or permissions.
        let mut process: serde_json::Value = serde_json::from_str(valid()).unwrap();
        process["host"] = serde_json::json!({"capabilities": ["image-viewer.v1"]});
        assert!(parse(&serde_json::to_string(&process).unwrap()).is_err());
        let mut process: serde_json::Value = serde_json::from_str(valid()).unwrap();
        process["requestedPermissions"] =
            serde_json::json!([{"kind": "local-files", "access": "opened-folders"}]);
        assert!(parse(&serde_json::to_string(&process).unwrap()).is_err());
        let mut process: serde_json::Value = serde_json::from_str(valid()).unwrap();
        process.as_object_mut().unwrap().remove("rpc");
        assert!(parse(&serde_json::to_string(&process).unwrap()).is_err());
    }

    #[test]
    fn ui_manifest_is_strict_bounded_and_requires_host_permission() {
        let valid: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../contracts/plugins/fixtures/valid-ui-provider-v2.json"
        ))
        .unwrap();
        let parse = |value: &serde_json::Value| {
            parse_manifest_bytes(
                &serde_json::to_vec(value).unwrap(),
                Path::new("plugin.json"),
            )
        };
        let parsed = parse(&valid).unwrap();
        let ui = parsed.contributions.as_ref().unwrap().ui.as_ref().unwrap();
        let status = ui.views.iter().find(|view| view.id == "status").unwrap();
        assert_eq!(
            status.slot,
            super::super::contributions::PluginUiSlot::NotificationCenter
        );
        assert_eq!(
            serde_json::to_value(status).unwrap()["slot"],
            "notification-center"
        );
        for (pointer, value) in [
            ("/contributions/ui/apiVersion", serde_json::json!(2)),
            (
                "/contributions/ui/entry",
                serde_json::json!("../index.html"),
            ),
            (
                "/contributions/ui/entry",
                serde_json::json!("ui\\index.html"),
            ),
            (
                "/contributions/ui/entry",
                serde_json::json!("ui./index.html"),
            ),
            (
                "/contributions/ui/views/0/slot",
                serde_json::json!("unknown"),
            ),
            ("/contributions/ui/views/0/title", serde_json::json!("\n")),
            (
                "/contributions/ui/views/0/title",
                serde_json::json!("界".repeat(129)),
            ),
            (
                "/contributions/ui/sha256",
                serde_json::json!("G".repeat(64)),
            ),
            ("/contributions/ui/views", serde_json::json!([])),
            ("/contributions/ui", serde_json::Value::Null),
            ("/requestedPermissions", serde_json::json!([])),
            (
                "/requestedPermissions/0/operations",
                serde_json::json!(["unknown"]),
            ),
            (
                "/requestedPermissions/0/operations",
                serde_json::json!(["core-api", "core-api"]),
            ),
        ] {
            let mut invalid = valid.clone();
            *invalid.pointer_mut(pointer).unwrap() = value;
            assert!(parse(&invalid).is_err(), "{pointer}");
        }
        let mut duplicate = valid.clone();
        duplicate["contributions"]["ui"]["views"][1]["id"] = serde_json::json!("setup");
        assert!(parse(&duplicate).is_err());
        let mut unknown = valid.clone();
        unknown["contributions"]["ui"]["url"] = serde_json::json!("https://example.test/");
        assert!(parse(&unknown).is_err());
        let mut no_ui = valid;
        no_ui["contributions"].as_object_mut().unwrap().remove("ui");
        no_ui["requestedPermissions"] = serde_json::json!([]);
        assert!(parse(&no_ui).is_ok());
    }
}
#[test]
fn file_decoder_manifest_is_exact_and_provider_stays_strict() {
    let bytes = include_bytes!("../../../../contracts/plugins/fixtures/valid-file-decoder-v2.json");
    let manifest = parse_manifest_bytes(bytes, Path::new("plugin.json")).unwrap();
    assert_eq!(manifest.rpc.capabilities, [Capability::FileDecoderV1]);
    assert_eq!(manifest.rpc.decoder.unwrap().extensions, ["ppm"]);
    let base: serde_json::Value = serde_json::from_slice(bytes).unwrap();
    for mutate in [
        |value: &mut serde_json::Value| {
            value["rpc"]["capabilities"] =
                serde_json::json!(["file-decoder.v1", "sync-provider.v1"]);
        },
        |value: &mut serde_json::Value| {
            value["rpc"]["methodVersions"]["auth.begin"] = 1.into();
        },
        |value: &mut serde_json::Value| {
            value["rpc"]["decoder"]["extensions"] = serde_json::json!(["*"]);
        },
        |value: &mut serde_json::Value| {
            value["rpc"]["decoder"]["purposes"] = serde_json::json!(["preview", "preview"]);
        },
        |value: &mut serde_json::Value| {
            value["activationEvents"] = serde_json::json!(["auth-requested"]);
        },
        |value: &mut serde_json::Value| {
            value["requestedPermissions"] =
                serde_json::json!([{"kind":"network","schemes":["https"],"hosts":["example.com"]}]);
        },
    ] {
        let mut value = base.clone();
        mutate(&mut value);
        assert!(parse_manifest_bytes(
            &serde_json::to_vec(&value).unwrap(),
            Path::new("plugin.json")
        )
        .is_err());
    }
}
