use std::fs::File;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::super::manifest::{ManifestRecord, Platform};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustEntry {
    pub plugin_id: String,
    pub version: String,
    pub platform: Platform,
    pub binary_sha256: [u8; 32],
}

/// Build-time trust root. Provider packaging adds exact entries here; an empty
/// registry deliberately makes a development build incapable of executing a
/// merely discovered or database-injected binary.
pub fn embedded_trust_registry() -> &'static [TrustEntry] {
    static REGISTRY: std::sync::OnceLock<Vec<TrustEntry>> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| {
        let hash = super::generated_trust::GENERATED_FAKE_PROVIDER_SHA256;
        if hash == [0; 32] {
            return Vec::new();
        }
        vec![
            TrustEntry {
                plugin_id: "com.megle.fake-sync".into(),
                version: "1.0.0".into(),
                platform: Platform::WindowsX64,
                binary_sha256: hash,
            },
            TrustEntry {
                plugin_id: "com.megle.fake-sync".into(),
                version: "1.0.0".into(),
                platform: Platform::LinuxX64,
                binary_sha256: hash,
            },
        ]
    })
}

#[derive(Debug)]
pub enum TrustError {
    MissingEntry,
    EntryEscapesRoot,
    SymbolicLink,
    UnsafeOwnership,
    HashMismatch,
    IdentityMismatch,
    Io(std::io::Error),
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
    file: File,
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

    pub(crate) fn file(&self) -> &File {
        &self.file
    }

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
        file,
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
    fn exact_allowlist_hash_is_required() {
        let root = std::env::temp_dir().join(format!(
            "megle_trust_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(bin.join("provider"), b"trusted").unwrap();
        let json = format!(
            r#"{{"manifestVersion":2,"id":"com.megle.test","name":"Test","version":"1.0.0","engine":"process","entry":{{"linuxX64":"bin/provider"}},"rpc":{{"wireVersions":[1],"capabilities":["sync-provider.v1"],"methodVersions":{{"describe":1,"health":1,"diagnostics":1,"shutdown":1,"auth.begin":1,"auth.complete":1,"auth.status":1,"auth.sign_out":1,"delta.list":1,"object.head":1,"object.get":1,"object.put_immutable":1,"upload.begin":1,"upload.write":1,"upload.commit":1,"upload.abort":1}}}},"activationEvents":["sync-enabled"],"requestedPermissions":[],"integrity":{{"sha256":"{}"}}}}"#,
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
        }];

        #[cfg(windows)]
        assert!(verify_bundled_entry(&manifest, &root, Platform::LinuxX64, &allowlist).is_ok());
        std::fs::write(bin.join("provider"), b"swapped").unwrap();
        assert!(matches!(
            verify_bundled_entry(&manifest, &root, Platform::LinuxX64, &allowlist),
            Err(TrustError::HashMismatch) | Err(TrustError::UnsafeOwnership)
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn path_replacement_does_not_change_verified_handle() {
        let root = std::env::temp_dir().join(format!(
            "megle_trust_swap_{}_{}",
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
}
