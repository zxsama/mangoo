use aes_gcm::{
    aead::{rand_core::RngCore, Aead, KeyInit, OsRng},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretScope {
    pub plugin_id: String,
    pub account_uuid: String,
    pub namespace: String,
    pub purpose: String,
}

impl SecretScope {
    fn validate(&self) -> anyhow::Result<()> {
        for (name, value) in [
            ("plugin_id", &self.plugin_id),
            ("account_uuid", &self.account_uuid),
            ("namespace", &self.namespace),
            ("purpose", &self.purpose),
        ] {
            if value.is_empty()
                || value.len() > 160
                || !value
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            {
                anyhow::bail!("invalid secret {name}");
            }
        }
        Ok(())
    }

    fn aad(&self, opaque_ref: &str, generation: u64) -> anyhow::Result<Vec<u8>> {
        self.validate()?;
        Ok(serde_json::to_vec(&(self, opaque_ref, generation))?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretValue {
    pub bytes: Vec<u8>,
    pub generation: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SecretStoreError {
    NotFound,
    ScopeDenied,
    GenerationConflict,
    ConfigurationRequired(String),
    Corrupt(String),
    Storage(String),
}

impl std::fmt::Display for SecretStoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotFound => write!(f, "secret not found"),
            Self::ScopeDenied => write!(f, "secret scope denied"),
            Self::GenerationConflict => write!(f, "secret generation conflict"),
            Self::ConfigurationRequired(v) => write!(f, "secret store configuration required: {v}"),
            Self::Corrupt(v) => write!(f, "secret record is corrupt: {v}"),
            Self::Storage(v) => write!(f, "secret store failed: {v}"),
        }
    }
}
impl std::error::Error for SecretStoreError {}

pub trait SecretStore: Send + Sync {
    fn create(&self, scope: &SecretScope, value: &[u8]) -> Result<String, SecretStoreError>;
    fn read(&self, scope: &SecretScope, opaque_ref: &str) -> Result<SecretValue, SecretStoreError>;
    fn compare_and_swap(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
        expected_generation: u64,
        value: &[u8],
    ) -> Result<u64, SecretStoreError>;
    fn delete(&self, scope: &SecretScope, opaque_ref: &str) -> Result<(), SecretStoreError>;
}

pub fn platform_secret_store() -> Result<std::sync::Arc<dyn SecretStore>, SecretStoreError> {
    #[cfg(windows)]
    {
        return Ok(std::sync::Arc::new(WindowsCredentialSecretStore::default()));
    }
    #[cfg(not(windows))]
    {
        let key = key_file_from_environment()?;
        let directory = std::env::var_os("MEGLE_SECRETS_DIR")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/data/secrets"));
        Ok(std::sync::Arc::new(
            EncryptedFileSecretStore::from_key_file(directory, &key)?,
        ))
    }
}

#[derive(Default)]
pub struct MemorySecretStore {
    records: Mutex<HashMap<String, (SecretScope, SecretValue)>>,
}

impl SecretStore for MemorySecretStore {
    fn create(&self, scope: &SecretScope, value: &[u8]) -> Result<String, SecretStoreError> {
        scope
            .validate()
            .map_err(|_| SecretStoreError::ScopeDenied)?;
        let opaque_ref = format!("mem_{}", uuid::Uuid::now_v7().simple());
        self.records
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?
            .insert(
                opaque_ref.clone(),
                (
                    scope.clone(),
                    SecretValue {
                        bytes: value.to_vec(),
                        generation: 1,
                    },
                ),
            );
        Ok(opaque_ref)
    }

    fn read(&self, scope: &SecretScope, opaque_ref: &str) -> Result<SecretValue, SecretStoreError> {
        let records = self
            .records
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let (stored_scope, value) = records.get(opaque_ref).ok_or(SecretStoreError::NotFound)?;
        if stored_scope != scope {
            return Err(SecretStoreError::ScopeDenied);
        }
        Ok(value.clone())
    }

    fn compare_and_swap(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
        expected_generation: u64,
        value: &[u8],
    ) -> Result<u64, SecretStoreError> {
        let mut records = self
            .records
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let (stored_scope, stored) = records
            .get_mut(opaque_ref)
            .ok_or(SecretStoreError::NotFound)?;
        if stored_scope != scope {
            return Err(SecretStoreError::ScopeDenied);
        }
        if stored.generation != expected_generation {
            return Err(SecretStoreError::GenerationConflict);
        }
        stored.generation += 1;
        stored.bytes.clear();
        stored.bytes.extend_from_slice(value);
        Ok(stored.generation)
    }

    fn delete(&self, scope: &SecretScope, opaque_ref: &str) -> Result<(), SecretStoreError> {
        let mut records = self
            .records
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let (stored_scope, _) = records.get(opaque_ref).ok_or(SecretStoreError::NotFound)?;
        if stored_scope != scope {
            return Err(SecretStoreError::ScopeDenied);
        }
        records.remove(opaque_ref);
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
struct EncryptedRecord {
    version: u8,
    scope: SecretScope,
    generation: u64,
    nonce: String,
    ciphertext: String,
}

pub struct EncryptedFileSecretStore {
    directory: PathBuf,
    key: [u8; 32],
    operation_lock: Mutex<()>,
}

impl EncryptedFileSecretStore {
    pub fn from_key_file(
        directory: impl Into<PathBuf>,
        key_file: &Path,
    ) -> Result<Self, SecretStoreError> {
        let metadata = std::fs::metadata(key_file).map_err(|e| {
            SecretStoreError::ConfigurationRequired(format!("cannot read key file: {e}"))
        })?;
        if !metadata.is_file() {
            return Err(SecretStoreError::ConfigurationRequired(
                "key path is not a file".into(),
            ));
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if metadata.mode() & 0o077 != 0 {
                return Err(SecretStoreError::ConfigurationRequired(
                    "key file must not be group/other accessible".into(),
                ));
            }
        }
        let raw = std::fs::read(key_file)
            .map_err(|e| SecretStoreError::ConfigurationRequired(e.to_string()))?;
        let key: [u8; 32] = if raw.len() == 32 {
            raw.try_into().map_err(|_| {
                SecretStoreError::ConfigurationRequired("master key must be 32 bytes".into())
            })?
        } else {
            let decoded = URL_SAFE_NO_PAD
                .decode(
                    raw.iter()
                        .copied()
                        .filter(|b| !b.is_ascii_whitespace())
                        .collect::<Vec<_>>(),
                )
                .map_err(|_| {
                    SecretStoreError::ConfigurationRequired(
                        "master key must be 32 raw bytes or base64url".into(),
                    )
                })?;
            decoded.try_into().map_err(|_| {
                SecretStoreError::ConfigurationRequired("master key must decode to 32 bytes".into())
            })?
        };
        let directory = directory.into();
        std::fs::create_dir_all(&directory)
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
                .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        }
        Ok(Self {
            directory,
            key,
            operation_lock: Mutex::new(()),
        })
    }

    fn path(&self, opaque_ref: &str) -> Result<PathBuf, SecretStoreError> {
        if !opaque_ref.starts_with("sec_")
            || opaque_ref.len() != 36
            || !opaque_ref[4..].bytes().all(|b| b.is_ascii_hexdigit())
        {
            return Err(SecretStoreError::NotFound);
        }
        Ok(self.directory.join(format!("{opaque_ref}.json")))
    }

    fn load(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
    ) -> Result<(EncryptedRecord, Vec<u8>), SecretStoreError> {
        let bytes = std::fs::read(self.path(opaque_ref)?).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                SecretStoreError::NotFound
            } else {
                SecretStoreError::Storage(e.to_string())
            }
        })?;
        let record: EncryptedRecord =
            serde_json::from_slice(&bytes).map_err(|e| SecretStoreError::Corrupt(e.to_string()))?;
        if record.version != 1 {
            return Err(SecretStoreError::Corrupt(
                "unsupported record version".into(),
            ));
        }
        if &record.scope != scope {
            return Err(SecretStoreError::ScopeDenied);
        }
        let nonce = URL_SAFE_NO_PAD
            .decode(&record.nonce)
            .map_err(|e| SecretStoreError::Corrupt(e.to_string()))?;
        let ciphertext = URL_SAFE_NO_PAD
            .decode(&record.ciphertext)
            .map_err(|e| SecretStoreError::Corrupt(e.to_string()))?;
        let aad = scope
            .aad(opaque_ref, record.generation)
            .map_err(|_| SecretStoreError::ScopeDenied)?;
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let plaintext = cipher
            .decrypt(
                Nonce::from_slice(&nonce),
                aes_gcm::aead::Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| SecretStoreError::Corrupt("authentication failed".into()))?;
        Ok((record, plaintext))
    }

    fn save(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
        generation: u64,
        value: &[u8],
    ) -> Result<(), SecretStoreError> {
        let mut nonce = [0u8; 12];
        OsRng.fill_bytes(&mut nonce);
        let aad = scope
            .aad(opaque_ref, generation)
            .map_err(|_| SecretStoreError::ScopeDenied)?;
        let cipher = Aes256Gcm::new_from_slice(&self.key)
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let ciphertext = cipher
            .encrypt(
                Nonce::from_slice(&nonce),
                aes_gcm::aead::Payload {
                    msg: value,
                    aad: &aad,
                },
            )
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let record = EncryptedRecord {
            version: 1,
            scope: scope.clone(),
            generation,
            nonce: URL_SAFE_NO_PAD.encode(nonce),
            ciphertext: URL_SAFE_NO_PAD.encode(ciphertext),
        };
        let path = self.path(opaque_ref)?;
        let tmp = self.directory.join(format!(
            ".{opaque_ref}.{}.tmp",
            uuid::Uuid::now_v7().simple()
        ));
        let bytes =
            serde_json::to_vec(&record).map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let mut options = std::fs::OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&tmp)
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        use std::io::Write;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        std::fs::rename(&tmp, &path).map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        if let Ok(dir) = std::fs::File::open(&self.directory) {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

impl SecretStore for EncryptedFileSecretStore {
    fn create(&self, scope: &SecretScope, value: &[u8]) -> Result<String, SecretStoreError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        scope
            .validate()
            .map_err(|_| SecretStoreError::ScopeDenied)?;
        let mut random = [0u8; 16];
        OsRng.fill_bytes(&mut random);
        let opaque_ref = format!("sec_{}", hex(&random));
        self.save(scope, &opaque_ref, 1, value)?;
        Ok(opaque_ref)
    }
    fn read(&self, scope: &SecretScope, opaque_ref: &str) -> Result<SecretValue, SecretStoreError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let (record, bytes) = self.load(scope, opaque_ref)?;
        Ok(SecretValue {
            bytes,
            generation: record.generation,
        })
    }
    fn compare_and_swap(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
        expected_generation: u64,
        value: &[u8],
    ) -> Result<u64, SecretStoreError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let (record, _) = self.load(scope, opaque_ref)?;
        if record.generation != expected_generation {
            return Err(SecretStoreError::GenerationConflict);
        }
        let next = expected_generation
            .checked_add(1)
            .ok_or_else(|| SecretStoreError::Storage("generation overflow".into()))?;
        self.save(scope, opaque_ref, next, value)?;
        Ok(next)
    }
    fn delete(&self, scope: &SecretScope, opaque_ref: &str) -> Result<(), SecretStoreError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        self.load(scope, opaque_ref)?;
        std::fs::remove_file(self.path(opaque_ref)?)
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        if let Ok(directory) = std::fs::File::open(&self.directory) {
            directory
                .sync_all()
                .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        }
        Ok(())
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn key_file_from_environment() -> Result<PathBuf, SecretStoreError> {
    std::env::var_os("MEGLE_SECRET_KEY_FILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| {
            SecretStoreError::ConfigurationRequired("MEGLE_SECRET_KEY_FILE is not set".into())
        })
}

pub fn scope_fingerprint(scope: &SecretScope) -> Result<String, SecretStoreError> {
    scope
        .validate()
        .map_err(|_| SecretStoreError::ScopeDenied)?;
    let digest = Sha256::digest(
        serde_json::to_vec(scope).map_err(|e| SecretStoreError::Storage(e.to_string()))?,
    );
    Ok(hex(&digest[..16]))
}

#[cfg(windows)]
pub struct WindowsCredentialSecretStore {
    operation_lock: Mutex<()>,
}

#[cfg(windows)]
impl Default for WindowsCredentialSecretStore {
    fn default() -> Self {
        Self {
            operation_lock: Mutex::new(()),
        }
    }
}

#[cfg(windows)]
impl WindowsCredentialSecretStore {
    fn target(scope: &SecretScope, opaque_ref: &str) -> Result<Vec<u16>, SecretStoreError> {
        let fingerprint = scope_fingerprint(scope)?;
        Ok(format!("Megle/{fingerprint}/{opaque_ref}\0")
            .encode_utf16()
            .collect())
    }

    fn write_record(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
        value: &SecretValue,
    ) -> Result<(), SecretStoreError> {
        use windows_sys::Win32::Security::Credentials::{
            CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
        };
        let target = Self::target(scope, opaque_ref)?;
        let mut blob = value.generation.to_le_bytes().to_vec();
        blob.extend_from_slice(&value.bytes);
        let mut credential: CREDENTIALW = unsafe { std::mem::zeroed() };
        credential.Type = CRED_TYPE_GENERIC;
        credential.TargetName = target.as_ptr() as *mut u16;
        credential.CredentialBlobSize = u32::try_from(blob.len())
            .map_err(|_| SecretStoreError::Storage("credential is too large".into()))?;
        credential.CredentialBlob = blob.as_mut_ptr();
        credential.Persist = CRED_PERSIST_LOCAL_MACHINE;
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(SecretStoreError::Storage(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        blob.fill(0);
        Ok(())
    }

    fn read_record(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
    ) -> Result<SecretValue, SecretStoreError> {
        use windows_sys::Win32::{
            Foundation::ERROR_NOT_FOUND,
            Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC},
        };
        let target = Self::target(scope, opaque_ref)?;
        let mut pointer: *mut CREDENTIALW = std::ptr::null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pointer) } == 0 {
            let error = std::io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Err(SecretStoreError::NotFound)
            } else {
                Err(SecretStoreError::Storage(error.to_string()))
            };
        }
        let result = (|| {
            let credential = unsafe { &*pointer };
            if credential.CredentialBlobSize < 8 {
                return Err(SecretStoreError::Corrupt(
                    "credential record is truncated".into(),
                ));
            }
            let blob = unsafe {
                std::slice::from_raw_parts(
                    credential.CredentialBlob,
                    credential.CredentialBlobSize as usize,
                )
            };
            let generation = u64::from_le_bytes(
                blob[..8]
                    .try_into()
                    .map_err(|_| SecretStoreError::Corrupt("generation missing".into()))?,
            );
            Ok(SecretValue {
                bytes: blob[8..].to_vec(),
                generation,
            })
        })();
        unsafe { CredFree(pointer.cast()) };
        result
    }
}

#[cfg(windows)]
impl SecretStore for WindowsCredentialSecretStore {
    fn create(&self, scope: &SecretScope, value: &[u8]) -> Result<String, SecretStoreError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        scope
            .validate()
            .map_err(|_| SecretStoreError::ScopeDenied)?;
        let opaque_ref = format!("sec_{}", uuid::Uuid::now_v7().simple());
        self.write_record(
            scope,
            &opaque_ref,
            &SecretValue {
                bytes: value.to_vec(),
                generation: 1,
            },
        )?;
        Ok(opaque_ref)
    }
    fn read(&self, scope: &SecretScope, opaque_ref: &str) -> Result<SecretValue, SecretStoreError> {
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        self.read_record(scope, opaque_ref)
    }
    fn compare_and_swap(
        &self,
        scope: &SecretScope,
        opaque_ref: &str,
        expected_generation: u64,
        value: &[u8],
    ) -> Result<u64, SecretStoreError> {
        // Hold the process-local mutex across read + replacement. Credential
        // Manager atomically replaces the record; Host is the only writer.
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let current = self.read_record(scope, opaque_ref)?;
        if current.generation != expected_generation {
            return Err(SecretStoreError::GenerationConflict);
        }
        let next = expected_generation
            .checked_add(1)
            .ok_or_else(|| SecretStoreError::Storage("generation overflow".into()))?;
        self.write_record(
            scope,
            opaque_ref,
            &SecretValue {
                bytes: value.to_vec(),
                generation: next,
            },
        )?;
        Ok(next)
    }
    fn delete(&self, scope: &SecretScope, opaque_ref: &str) -> Result<(), SecretStoreError> {
        use windows_sys::Win32::Security::Credentials::{CredDeleteW, CRED_TYPE_GENERIC};
        let _guard = self
            .operation_lock
            .lock()
            .map_err(|e| SecretStoreError::Storage(e.to_string()))?;
        let target = Self::target(scope, opaque_ref)?;
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            return Err(SecretStoreError::Storage(
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scope(account: &str) -> SecretScope {
        SecretScope {
            plugin_id: "com.megle.fake".into(),
            account_uuid: account.into(),
            namespace: "fake-sync".into(),
            purpose: "oauth".into(),
        }
    }
    fn conformance(store: &dyn SecretStore) {
        let a = scope("account-a");
        let b = scope("account-b");
        let reference = store.create(&a, b"first").unwrap();
        assert_eq!(
            store.read(&a, &reference).unwrap(),
            SecretValue {
                bytes: b"first".to_vec(),
                generation: 1
            }
        );
        assert_eq!(
            store.read(&b, &reference),
            Err(SecretStoreError::ScopeDenied)
        );
        assert_eq!(
            store.compare_and_swap(&a, &reference, 0, b"bad"),
            Err(SecretStoreError::GenerationConflict)
        );
        assert_eq!(
            store
                .compare_and_swap(&a, &reference, 1, b"second")
                .unwrap(),
            2
        );
        assert_eq!(store.read(&a, &reference).unwrap().bytes, b"second");
        store.delete(&a, &reference).unwrap();
        assert_eq!(store.read(&a, &reference), Err(SecretStoreError::NotFound));
    }
    #[test]
    fn memory_backend_conforms() {
        conformance(&MemorySecretStore::default())
    }
    #[test]
    fn encrypted_backend_persists_and_authenticates() {
        let root = std::env::temp_dir().join(format!("megle-secret-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&root).unwrap();
        let key = root.join("key");
        std::fs::write(&key, [7u8; 32]).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        let store = EncryptedFileSecretStore::from_key_file(root.join("records"), &key).unwrap();
        conformance(&store);
        let reference = store
            .create(&scope("account-a"), b"token-not-plaintext")
            .unwrap();
        let stored = std::fs::read(store.path(&reference).unwrap()).unwrap();
        assert!(!stored.windows(19).any(|w| w == b"token-not-plaintext"));
        let _ = std::fs::remove_dir_all(root);
    }
}
