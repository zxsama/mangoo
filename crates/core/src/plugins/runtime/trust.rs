use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::super::manifest::{Engine, ManifestRecord, Platform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    pub plugin_id: String,
    pub version: String,
    pub platform: Platform,
    pub binary_sha256: [u8; 32],
    pub manifest_sha256: [u8; 32],
}

/// Build-time trust root. Provider packaging adds exact entries here; an empty
/// registry deliberately makes a development build incapable of executing a
/// merely discovered or database-injected binary.
pub fn embedded_trust_registry() -> &'static [TrustEntry] {
    static REGISTRY: std::sync::OnceLock<Vec<TrustEntry>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        super::generated_trust::GENERATED_PLUGIN_TRUST
            .iter()
            // A zero binary digest is the Host-engine marker (no executable);
            // it can never match a real executable, and the exact manifest
            // digest still pins every Host plugin.
            .filter(|(_, _, _, _, manifest_hash)| *manifest_hash != [0; 32])
            .map(
                |(plugin_id, version, platform, binary_sha256, manifest_sha256)| TrustEntry {
                    plugin_id: (*plugin_id).into(),
                    version: (*version).into(),
                    platform: *platform,
                    binary_sha256: *binary_sha256,
                    manifest_sha256: *manifest_sha256,
                },
            )
            .collect()
    })
}

#[derive(Debug)]
pub enum TrustError {
    MissingEntry,
    EntryEscapesRoot,
    SymbolicLink,
    #[cfg(unix)]
    UnsafeOwnership,
    HashMismatch,
    IdentityMismatch,
    InvalidDocument,
    Io(std::io::Error),
}

impl std::fmt::Display for TrustError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingEntry => write!(formatter, "trusted executable entry is missing"),
            Self::EntryEscapesRoot => write!(formatter, "trusted executable escapes its root"),
            Self::SymbolicLink => write!(
                formatter,
                "trusted executable path contains a symbolic link"
            ),
            #[cfg(unix)]
            Self::UnsafeOwnership => write!(formatter, "trusted executable ownership is unsafe"),
            Self::HashMismatch => write!(formatter, "trusted executable hash does not match"),
            Self::IdentityMismatch => write!(formatter, "trusted executable identity changed"),
            Self::InvalidDocument => write!(formatter, "trusted plugin document is invalid"),
            Self::Io(error) => write!(formatter, "trusted executable I/O failed: {error}"),
        }
    }
}

impl std::error::Error for TrustError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io(error) => Some(error),
            _ => None,
        }
    }
}

impl From<std::io::Error> for TrustError {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}

/// The exact file object which passed trust validation. Process startup must
/// consume this object rather than reopen `path`, or it recreates a TOCTOU gap.
#[derive(Debug)]
pub struct VerifiedExecutable {
    path: PathBuf,
    expected_sha256: [u8; 32],
    _file: File,
    identity: StableIdentity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct StableIdentity {
    first: u64,
    second: u64,
}

impl VerifiedExecutable {
    pub fn path(&self) -> &Path {
        &self.path
    }

    pub(crate) fn expected_sha256(&self) -> [u8; 32] {
        self.expected_sha256
    }

    #[cfg(any(unix, test))]
    pub(crate) fn file(&self) -> &File {
        &self._file
    }

    #[cfg(test)]
    pub(crate) fn path_matches_verified_file(&self) -> Result<bool, TrustError> {
        self.matches_path(&self.path)
    }

    pub(crate) fn matches_path(&self, path: &Path) -> Result<bool, TrustError> {
        let reopened = open_nofollow(path)?;
        Ok(stable_identity(&reopened)? == self.identity
            && sha256_handle(&reopened)? == self.expected_sha256)
    }

    #[cfg(test)]
    pub(crate) fn for_test(path: &Path) -> Result<Self, TrustError> {
        verified_from_path(path, false, None)
    }
}

pub fn verify_bundled_entry(
    manifest: &ManifestRecord,
    bundled_root: &Path,
    platform: Platform,
    allowlist: &[TrustEntry],
) -> Result<VerifiedExecutable, TrustError> {
    if manifest.engine != Engine::Process {
        return Err(TrustError::MissingEntry);
    }
    verify_manifest(manifest, bundled_root, platform, allowlist)?;
    if let Some(ui) = manifest.contributions.as_ref().and_then(|c| c.ui.as_ref()) {
        read_ui_document(bundled_root, ui)?;
    }
    let relative = manifest
        .entry_for(platform)
        .ok_or(TrustError::MissingEntry)?;
    let expected = allowlist
        .iter()
        .find(|entry| {
            entry.plugin_id == manifest.id
                && entry.version == manifest.version
                && entry.platform == platform
        })
        .ok_or(TrustError::MissingEntry)?;

    let canonical_root = bundled_root.canonicalize()?;
    let joined = bundled_root.join(relative);
    let mut cursor = bundled_root.to_path_buf();
    for component in Path::new(relative).components() {
        cursor.push(component.as_os_str());
        if std::fs::symlink_metadata(&cursor)?.file_type().is_symlink() {
            return Err(TrustError::SymbolicLink);
        }
    }
    let canonical = joined.canonicalize()?;
    if !canonical.starts_with(&canonical_root) {
        return Err(TrustError::EntryEscapesRoot);
    }

    verified_from_path(&canonical, cfg!(unix), Some(expected.binary_sha256))
}

pub const MAX_UI_DOCUMENT_BYTES: usize = 8 * 1024 * 1024;

/// Trust for a Host-engine plugin: the artifact is exactly its manifest, which
/// must match the compiled digest, and the compiled row must carry the
/// no-executable marker so a process row can never vouch for it (or vice versa).
pub fn verify_bundled_host_plugin(
    manifest: &ManifestRecord,
    bundled_root: &Path,
    platform: Platform,
    allowlist: &[TrustEntry],
) -> Result<(), TrustError> {
    if manifest.engine != Engine::Host {
        return Err(TrustError::IdentityMismatch);
    }
    let expected = allowlist
        .iter()
        .find(|entry| {
            entry.plugin_id == manifest.id
                && entry.version == manifest.version
                && entry.platform == platform
        })
        .ok_or(TrustError::MissingEntry)?;
    if expected.binary_sha256 != [0; 32] {
        return Err(TrustError::IdentityMismatch);
    }
    verify_manifest(manifest, bundled_root, platform, allowlist)
}

/// Read from a pinned no-follow handle. On Windows the complete parent chain
/// also stays pinned against replacement; Unix packages require safe ownership.
fn read_bounded_document(root: &Path, relative: &str, limit: usize) -> Result<Vec<u8>, TrustError> {
    let path = root.join(relative);
    #[cfg(unix)]
    validate_unix_components(&path)?;
    let parent = path.parent().ok_or(TrustError::EntryEscapesRoot)?;
    let leaf = path.file_name().ok_or(TrustError::EntryEscapesRoot)?;
    let directory = crate::library_path::MarkerDirectoryGuard::open_readonly(parent)?;
    let file = directory.open_marker(leaf, false)?;
    if file.metadata()?.len() > limit as u64 {
        return Err(TrustError::InvalidDocument);
    }
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > limit {
        return Err(TrustError::InvalidDocument);
    }
    directory.verify_unchanged()?;
    Ok(bytes)
}

pub fn verify_manifest(
    manifest: &ManifestRecord,
    root: &Path,
    platform: Platform,
    allowlist: &[TrustEntry],
) -> Result<(), TrustError> {
    let expected = allowlist
        .iter()
        .find(|entry| {
            entry.plugin_id == manifest.id
                && entry.version == manifest.version
                && entry.platform == platform
        })
        .ok_or(TrustError::MissingEntry)?;
    let path = root.join("plugin.json");
    let bytes = read_bounded_document(root, "plugin.json", 256 * 1024)?;
    let hash: [u8; 32] = Sha256::digest(&bytes).into();
    if hash != expected.manifest_sha256 {
        return Err(TrustError::HashMismatch);
    }
    let mut actual = super::super::manifest::parse_manifest_bytes(&bytes, &path)
        .map_err(|_| TrustError::InvalidDocument)?;
    let mut selected = manifest.clone();
    actual.manifest_path = PathBuf::new();
    selected.manifest_path = PathBuf::new();
    if actual != selected {
        return Err(TrustError::IdentityMismatch);
    }
    Ok(())
}

pub fn read_ui_document(
    root: &Path,
    ui: &super::super::contributions::PluginUiContribution,
) -> Result<String, TrustError> {
    if !super::super::contributions::valid_ui_entry(&ui.entry) {
        return Err(TrustError::EntryEscapesRoot);
    }
    let bytes = read_bounded_document(root, &ui.entry, MAX_UI_DOCUMENT_BYTES)?;
    if format!("{:x}", Sha256::digest(&bytes)) != ui.sha256 {
        return Err(TrustError::HashMismatch);
    }
    String::from_utf8(bytes).map_err(|_| TrustError::InvalidDocument)
}

fn verified_from_path(
    path: &Path,
    _require_secure_components: bool,
    expected_sha256: Option<[u8; 32]>,
) -> Result<VerifiedExecutable, TrustError> {
    #[cfg(unix)]
    if _require_secure_components {
        validate_unix_components(path)?;
    }

    let file = open_nofollow(path)?;
    let before = stable_identity(&file)?;
    let actual = sha256_handle(&file)?;
    let after = stable_identity(&file)?;
    if before != after {
        return Err(TrustError::IdentityMismatch);
    }
    if expected_sha256.is_some_and(|expected| expected != actual) {
        return Err(TrustError::HashMismatch);
    }
    Ok(VerifiedExecutable {
        path: path.to_path_buf(),
        expected_sha256: actual,
        _file: file,
        identity: after,
    })
}

fn open_nofollow(path: &Path) -> Result<File, TrustError> {
    let mut options = File::options();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

fn sha256_handle(file: &File) -> Result<[u8; 32], TrustError> {
    let mut cloned = file.try_clone()?;
    cloned.seek(std::io::SeekFrom::Start(0))?;
    let mut hash = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = cloned.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    Ok(hash.finalize().into())
}

#[cfg(unix)]
fn stable_identity(file: &File) -> Result<StableIdentity, TrustError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = file.metadata()?;
    Ok(StableIdentity {
        first: metadata.dev(),
        second: metadata.ino(),
    })
}

#[cfg(windows)]
fn stable_identity(file: &File) -> Result<StableIdentity, TrustError> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION,
    };
    let mut information: BY_HANDLE_FILE_INFORMATION = unsafe { std::mem::zeroed() };
    if unsafe { GetFileInformationByHandle(file.as_raw_handle() as _, &mut information) } == 0 {
        return Err(TrustError::Io(std::io::Error::last_os_error()));
    }
    Ok(StableIdentity {
        first: u64::from(information.dwVolumeSerialNumber),
        second: (u64::from(information.nFileIndexHigh) << 32)
            | u64::from(information.nFileIndexLow),
    })
}

#[cfg(not(any(unix, windows)))]
fn stable_identity(_file: &File) -> Result<StableIdentity, TrustError> {
    Err(TrustError::IdentityMismatch)
}

#[cfg(unix)]
fn validate_unix_components(path: &Path) -> Result<(), TrustError> {
    use std::os::unix::fs::MetadataExt;

    let mut current = PathBuf::from("/");
    for component in path.components() {
        use std::path::Component;
        match component {
            Component::RootDir => continue,
            Component::Normal(part) => current.push(part),
            _ => return Err(TrustError::EntryEscapesRoot),
        }
        let metadata = std::fs::symlink_metadata(&current)?;
        if metadata.file_type().is_symlink() {
            return Err(TrustError::SymbolicLink);
        }
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(TrustError::UnsafeOwnership);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_engine_trust_pins_the_manifest_and_requires_the_no_executable_marker() {
        let root = std::env::temp_dir().join(format!(
            "mangoo_host_trust_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let manifest_path = root.join("plugin.json");
        std::fs::write(
            &manifest_path,
            include_str!(
                "../../../../../contracts/plugins/fixtures/valid-host-image-viewer-v2.json"
            ),
        )
        .unwrap();
        let manifest = crate::plugins::manifest::parse_manifest(&manifest_path).unwrap();
        let manifest_sha256: [u8; 32] =
            Sha256::digest(std::fs::read(&manifest_path).unwrap()).into();
        let row = |binary_sha256: [u8; 32], manifest_sha256: [u8; 32]| TrustEntry {
            plugin_id: manifest.id.clone(),
            version: manifest.version.clone(),
            platform: Platform::WindowsX64,
            binary_sha256,
            manifest_sha256,
        };
        verify_bundled_host_plugin(
            &manifest,
            &root,
            Platform::WindowsX64,
            &[row([0; 32], manifest_sha256)],
        )
        .expect("exact manifest digest with the host marker");
        assert!(verify_bundled_host_plugin(&manifest, &root, Platform::WindowsX64, &[]).is_err());
        // A process row (non-zero executable digest) never vouches for a host plugin.
        assert!(verify_bundled_host_plugin(
            &manifest,
            &root,
            Platform::WindowsX64,
            &[row([7; 32], manifest_sha256)]
        )
        .is_err());
        assert!(verify_bundled_host_plugin(
            &manifest,
            &root,
            Platform::WindowsX64,
            &[row([0; 32], [9; 32])]
        )
        .is_err());
        // The process verifier never accepts a host manifest.
        assert!(matches!(
            verify_bundled_entry(
                &manifest,
                &root,
                Platform::WindowsX64,
                &[row([0; 32], manifest_sha256)]
            ),
            Err(TrustError::MissingEntry)
        ));
        // Tampering with the manifest bytes breaks the pin.
        std::fs::write(
            &manifest_path,
            std::fs::read_to_string(&manifest_path)
                .unwrap()
                .replace("Mangoo", "Other"),
        )
        .unwrap();
        assert!(verify_bundled_host_plugin(
            &manifest,
            &root,
            Platform::WindowsX64,
            &[row([0; 32], manifest_sha256)]
        )
        .is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn exact_allowlist_hash_is_required() {
        let root = std::env::temp_dir().join(format!(
            "mangoo_trust_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("provider"), b"trusted").unwrap();
        let json = format!(
            r#"{{"manifestVersion":2,"id":"com.mangoo.test","name":"Test","version":"1.0.0","engine":"process","entry":{{"linuxX64":"bin/provider"}},"rpc":{{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.poll":1,"auth.cancel":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.get_range":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}}}},"activationEvents":["sync-enabled"],"requestedPermissions":[],"integrity":{{"sha256":"{}"}}}}"#,
            "0".repeat(64)
        );
        let manifest_path = root.join("plugin.json");
        std::fs::write(&manifest_path, json).unwrap();
        let manifest = crate::plugins::manifest::parse_manifest(&manifest_path).unwrap();
        let hash: [u8; 32] = Sha256::digest(b"trusted").into();
        let allowlist = [TrustEntry {
            plugin_id: manifest.id.clone(),
            version: manifest.version.clone(),
            platform: Platform::LinuxX64,
            binary_sha256: hash,
            manifest_sha256: Sha256::digest(std::fs::read(&manifest_path).unwrap()).into(),
        }];

        // The manifest's self-declared digest is never an executable trust root.
        assert!(matches!(
            verify_bundled_entry(&manifest, &root, Platform::LinuxX64, &[]),
            Err(TrustError::MissingEntry)
        ));
        for (plugin_id, version, platform) in [
            ("com.mangoo.other", "1.0.0", Platform::LinuxX64),
            ("com.mangoo.test", "2.0.0", Platform::LinuxX64),
            ("com.mangoo.test", "1.0.0", Platform::WindowsX64),
        ] {
            let unrelated = [TrustEntry {
                plugin_id: plugin_id.into(),
                version: version.into(),
                platform,
                binary_sha256: hash,
                manifest_sha256: allowlist[0].manifest_sha256,
            }];
            assert!(matches!(
                verify_bundled_entry(&manifest, &root, Platform::LinuxX64, &unrelated),
                Err(TrustError::MissingEntry)
            ));
        }

        #[cfg(windows)]
        assert!(verify_bundled_entry(&manifest, &root, Platform::LinuxX64, &allowlist).is_ok());
        std::fs::write(bin.join("provider"), b"swapped").unwrap();
        let result = verify_bundled_entry(&manifest, &root, Platform::LinuxX64, &allowlist);
        #[cfg(unix)]
        assert!(matches!(
            result,
            Err(TrustError::HashMismatch) | Err(TrustError::UnsafeOwnership)
        ));
        #[cfg(not(unix))]
        assert!(matches!(result, Err(TrustError::HashMismatch)));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_replacement_does_not_change_verified_handle() {
        let root = std::env::temp_dir().join(format!(
            "mangoo_trust_swap_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("provider");
        std::fs::write(&path, b"trusted").unwrap();
        let verified = VerifiedExecutable::for_test(&path).unwrap();
        let original = root.join("provider.original");
        std::fs::rename(&path, &original).unwrap();
        std::fs::write(&path, b"swapped").unwrap();

        assert!(!verified.path_matches_verified_file().unwrap());
        assert_eq!(
            sha256_handle(verified.file()).unwrap(),
            verified.expected_sha256()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn ui_document_reads_are_bounded_utf8_and_pin_the_selected_manifest() {
        let root = std::env::temp_dir().join(format!("mangoo-ui-trust-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(root.join("ui")).unwrap();
        let mut raw: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../contracts/plugins/fixtures/valid-ui-provider-v2.json"
        ))
        .unwrap();
        let document = b"<html>fixture</html>";
        raw["contributions"]["ui"]["sha256"] =
            serde_json::json!(format!("{:x}", Sha256::digest(document)));
        let bytes = serde_json::to_vec(&raw).unwrap();
        let manifest_path = root.join("plugin.json");
        std::fs::write(&manifest_path, &bytes).unwrap();
        std::fs::write(root.join("ui/index.html"), document).unwrap();
        let manifest = crate::plugins::manifest::parse_manifest(&manifest_path).unwrap();
        let trust = [TrustEntry {
            plugin_id: manifest.id.clone(),
            version: manifest.version.clone(),
            platform: Platform::WindowsX64,
            binary_sha256: [1; 32],
            manifest_sha256: Sha256::digest(&bytes).into(),
        }];
        verify_manifest(&manifest, &root, Platform::WindowsX64, &trust).unwrap();
        let mut altered = manifest.clone();
        altered
            .contributions
            .as_mut()
            .unwrap()
            .ui
            .as_mut()
            .unwrap()
            .views[0]
            .title = "Changed".into();
        assert!(matches!(
            verify_manifest(&altered, &root, Platform::WindowsX64, &trust),
            Err(TrustError::IdentityMismatch)
        ));
        let mut ui = manifest.contributions.unwrap().ui.unwrap();
        assert_eq!(read_ui_document(&root, &ui).unwrap().as_bytes(), document);
        let invalid = [0xff];
        std::fs::write(root.join("ui/index.html"), invalid).unwrap();
        ui.sha256 = format!("{:x}", Sha256::digest(invalid));
        assert!(matches!(
            read_ui_document(&root, &ui),
            Err(TrustError::InvalidDocument)
        ));
        let oversized = std::fs::File::create(root.join("ui/index.html")).unwrap();
        oversized.set_len(MAX_UI_DOCUMENT_BYTES as u64 + 1).unwrap();
        drop(oversized);
        assert!(matches!(
            read_ui_document(&root, &ui),
            Err(TrustError::InvalidDocument)
        ));
        ui.entry = "../outside.html".into();
        assert!(matches!(
            read_ui_document(&root, &ui),
            Err(TrustError::EntryEscapesRoot)
        ));
        std::fs::remove_dir_all(root).unwrap();
    }
}
