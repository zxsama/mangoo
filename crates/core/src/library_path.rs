use std::fs;
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MIN_LIBRARY_FREE_BYTES: u64 = 256 * 1024 * 1024;

pub const MARKER_FILE_NAME: &str = ".megle-library";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryMarker {
    pub format_version: u32,
    pub library_uuid: String,
    pub root_uuid: String,
    pub state: MarkerState,
    pub creation_txn_uuid: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum MarkerState {
    Pending,
    Committed,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum PreflightReason {
    Ok,
    InvalidPath,
    InvalidName,
    CloudSyncRoot,
    ReparsePoint,
    NestedLibraryRoot,
    NotEmpty,
    #[allow(dead_code)]
    MarkerMismatch,
    NotWritable,
    InsufficientSpace,
    PathChanged,
}

impl PreflightReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::InvalidPath => "invalid-path",
            Self::InvalidName => "invalid-name",
            Self::CloudSyncRoot => "cloud-sync-root",
            Self::ReparsePoint => "reparse-point",
            Self::NestedLibraryRoot => "nested-library-root",
            Self::NotEmpty => "not-empty",
            Self::MarkerMismatch => "marker-mismatch",
            Self::NotWritable => "not-writable",
            Self::InsufficientSpace => "insufficient-space",
            Self::PathChanged => "path-changed",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PreflightResult {
    pub ok: bool,
    pub reason: PreflightReason,
    pub normalized_root: PathBuf,
    pub token: Option<String>,
    pub available_bytes: Option<u64>,
}

pub fn safe_library_name(name: &str) -> Result<String, PreflightReason> {
    let trimmed = name.trim();
    if trimmed.is_empty()
        || trimmed.ends_with(['.', ' '])
        || trimmed
            .chars()
            .any(|c| c < ' ' || r#"<>:"/\|?*"#.contains(c))
    {
        return Err(PreflightReason::InvalidName);
    }
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED
        .iter()
        .any(|value| trimmed.eq_ignore_ascii_case(value))
    {
        return Err(PreflightReason::InvalidName);
    }
    Ok(trimmed.to_string())
}

pub fn normalize_absolute(path: &Path) -> Result<PathBuf, PreflightReason> {
    if !path.is_absolute()
        || path
            .components()
            .any(|part| matches!(part, Component::ParentDir))
    {
        return Err(PreflightReason::InvalidPath);
    }
    Ok(path.components().collect())
}

pub fn preflight(
    name: &str,
    candidate: &Path,
    existing_roots: &[PathBuf],
    cloud_roots: &[PathBuf],
) -> PreflightResult {
    if safe_library_name(name).is_err() {
        return rejected(candidate, PreflightReason::InvalidName);
    }
    let root = match normalize_absolute(candidate) {
        Ok(root) => root,
        Err(reason) => return rejected(candidate, reason),
    };
    if has_existing_reparse_component(&root) {
        return rejected(&root, PreflightReason::ReparsePoint);
    }
    if is_platform_sync_root(&root) {
        return rejected(&root, PreflightReason::CloudSyncRoot);
    }
    if cloud_roots.iter().any(|cloud| paths_overlap(&root, cloud)) {
        return rejected(&root, PreflightReason::CloudSyncRoot);
    }
    if existing_roots
        .iter()
        .any(|existing| paths_overlap(&root, existing))
    {
        return rejected(&root, PreflightReason::NestedLibraryRoot);
    }
    if root.exists() {
        match fs::read_dir(&root) {
            Ok(entries) => {
                if entries.into_iter().next().is_some() {
                    return rejected(&root, PreflightReason::NotEmpty);
                }
            }
            Err(_) => return rejected(&root, PreflightReason::NotWritable),
        }
    } else if root.parent().is_none_or(|parent| !parent.exists()) {
        return rejected(&root, PreflightReason::InvalidPath);
    }
    if !probe_writable(&root) {
        return rejected(&root, PreflightReason::NotWritable);
    }
    let available_bytes = free_space_bytes(&root).ok();
    if available_bytes.is_some_and(|bytes| bytes < MIN_LIBRARY_FREE_BYTES) {
        return rejected(&root, PreflightReason::InsufficientSpace);
    }
    let token = identity_token(&root).ok();
    PreflightResult {
        ok: token.is_some(),
        reason: if token.is_some() {
            PreflightReason::Ok
        } else {
            PreflightReason::NotWritable
        },
        normalized_root: root,
        token,
        available_bytes,
    }
}

pub fn identity_token(path: &Path) -> io::Result<String> {
    let existing = if path.exists() {
        path
    } else {
        path.parent()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no parent"))?
    };
    let identity = platform_identity(existing)?;
    let mut digest = Sha256::new();
    digest.update(existing.to_string_lossy().to_lowercase().as_bytes());
    digest.update(identity.as_bytes());
    Ok(format!("v1:{}", hex(&digest.finalize())))
}

/// Stable device-local volume/file identity used by watcher and full rescan
/// reconciliation. It is never serialized into synchronization events.
pub fn local_file_identity(path: &Path) -> io::Result<String> {
    platform_identity(path)
}

pub fn verify_preflight_token(path: &Path, token: &str) -> Result<(), PreflightReason> {
    if identity_token(path).ok().as_deref() == Some(token) {
        Ok(())
    } else {
        Err(PreflightReason::PathChanged)
    }
}

pub fn atomic_write_marker_guarded(
    guard: &SafeDirectory,
    marker: &LibraryMarker,
) -> anyhow::Result<String> {
    let temporary = PathBuf::from(format!(
        "{MARKER_FILE_NAME}.{}.tmp",
        marker.creation_txn_uuid
    ));
    let bytes = serde_json::to_vec_pretty(marker)?;
    let temporary_name = temporary
        .file_name()
        .ok_or_else(|| anyhow::anyhow!("marker temporary has no leaf"))?;
    let mut file = guard.create_file(temporary_name)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    guard.rename_child(temporary_name, std::ffi::OsStr::new(MARKER_FILE_NAME), true)?;
    guard
        .verify_unchanged()
        .map_err(|_| anyhow::anyhow!("path-changed"))?;
    guard.sync_all()?;
    Ok(guard.child_identity(std::ffi::OsStr::new(MARKER_FILE_NAME), false)?)
}

pub struct SafeDirectory {
    path: PathBuf,
    handle: fs::File,
    identity: String,
}

impl SafeDirectory {
    pub fn open(path: &Path) -> io::Result<Self> {
        reject_reparse_components(path)?;
        let handle = open_directory_nofollow(path)?;
        let identity = identity_from_handle(&handle)?;
        Ok(Self {
            path: path.to_path_buf(),
            handle,
            identity,
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn verify_unchanged(&self) -> Result<(), PreflightReason> {
        if platform_identity(&self.path).ok().as_deref() == Some(self.identity.as_str()) {
            Ok(())
        } else {
            Err(PreflightReason::PathChanged)
        }
    }

    pub fn create_file(&self, leaf: &std::ffi::OsStr) -> io::Result<fs::File> {
        create_relative_file(&self.handle, &self.path, leaf)
    }

    pub fn create_directory(&self, leaf: &std::ffi::OsStr) -> io::Result<()> {
        create_relative_directory(&self.handle, &self.path, leaf)
    }

    pub fn child_identity(&self, leaf: &std::ffi::OsStr, directory: bool) -> io::Result<String> {
        child_identity_relative(&self.handle, &self.path, leaf, directory)
    }

    pub fn delete_file_child(&self, leaf: &std::ffi::OsStr) -> io::Result<()> {
        delete_relative_file(&self.handle, &self.path, leaf)
    }

    pub fn rename_child(
        &self,
        source: &std::ffi::OsStr,
        target: &std::ffi::OsStr,
        replace: bool,
    ) -> io::Result<()> {
        rename_relative_child(
            &self.handle,
            &self.path,
            source,
            target,
            replace,
            false,
            None,
        )
    }

    pub fn rename_directory_child(
        &self,
        source: &std::ffi::OsStr,
        target: &std::ffi::OsStr,
    ) -> io::Result<()> {
        rename_relative_child(&self.handle, &self.path, source, target, false, true, None)
    }

    pub fn rename_directory_child_expected(
        &self,
        source: &std::ffi::OsStr,
        target: &std::ffi::OsStr,
        expected_identity: &str,
    ) -> io::Result<()> {
        rename_relative_child(
            &self.handle,
            &self.path,
            source,
            target,
            false,
            true,
            Some(expected_identity),
        )
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.handle.sync_all()
    }
}

/// Holds the complete Windows directory chain against rename while a marker is
/// published. Generic SafeDirectory handles request DELETE and cannot be used
/// for this purpose: they conflict with these deliberately narrower handles.
pub struct MarkerDirectoryGuard {
    path: PathBuf,
    handle: fs::File,
    identity: String,
    #[cfg(windows)]
    _ancestors: Vec<fs::File>,
}

impl MarkerDirectoryGuard {
    pub fn open(path: &Path) -> io::Result<Self> {
        #[cfg(windows)]
        let (handle, ancestors) = open_marker_directory_chain(path)?;
        #[cfg(not(windows))]
        let handle = {
            reject_reparse_components(path)?;
            open_directory_nofollow(path)?
        };
        let identity = identity_from_handle(&handle)?;
        let guard = Self {
            path: path.to_path_buf(),
            handle,
            identity,
            #[cfg(windows)]
            _ancestors: ancestors,
        };
        guard.verify_unchanged()?;
        Ok(guard)
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }

    pub fn verify_unchanged(&self) -> io::Result<()> {
        if platform_identity(&self.path)? != self.identity {
            return Err(io::Error::other("path-changed"));
        }
        Ok(())
    }

    pub fn file_identity(&self, file: &fs::File) -> io::Result<String> {
        identity_from_handle(file)
    }

    pub fn open_temporary(&self, leaf: &std::ffi::OsStr, create: bool) -> io::Result<fs::File> {
        validate_marker_leaf(leaf)?;
        #[cfg(windows)]
        let file = {
            use windows_sys::Win32::Storage::FileSystem::{
                DELETE, FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_READ, FILE_WRITE_DATA,
                SYNCHRONIZE,
            };
            nt_open_relative_with_access(
                &self.handle,
                leaf,
                false,
                create,
                FILE_READ_DATA | FILE_WRITE_DATA | FILE_READ_ATTRIBUTES | DELETE | SYNCHRONIZE,
                FILE_SHARE_READ,
            )?
        };
        #[cfg(not(windows))]
        let file = open_marker_relative(&self.handle, leaf, true, create)?;
        identity_from_handle(&file)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("marker is not a regular file"));
        }
        Ok(file)
    }

    /// Existing final markers deny competing writers/deleters. A verification
    /// read of our still-pinned source must instead share its WRITE and DELETE.
    pub fn open_marker(&self, leaf: &std::ffi::OsStr, pinned: bool) -> io::Result<fs::File> {
        validate_marker_leaf(leaf)?;
        #[cfg(windows)]
        let file = {
            use windows_sys::Win32::Storage::FileSystem::{
                FILE_READ_ATTRIBUTES, FILE_READ_DATA, FILE_SHARE_DELETE, FILE_SHARE_READ,
                FILE_SHARE_WRITE, SYNCHRONIZE,
            };
            let sharing = if pinned {
                FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE
            } else {
                FILE_SHARE_READ
            };
            nt_open_relative_with_access(
                &self.handle,
                leaf,
                false,
                false,
                FILE_READ_DATA | FILE_READ_ATTRIBUTES | SYNCHRONIZE,
                sharing,
            )?
        };
        #[cfg(not(windows))]
        let file = {
            let _ = pinned;
            open_marker_relative(&self.handle, leaf, false, false)?
        };
        identity_from_handle(&file)?;
        if !file.metadata()?.is_file() {
            return Err(io::Error::other("marker is not a regular file"));
        }
        Ok(file)
    }

    pub fn publish(
        &self,
        source: &fs::File,
        temporary: &std::ffi::OsStr,
        target: &std::ffi::OsStr,
    ) -> io::Result<()> {
        validate_marker_leaf(temporary)?;
        validate_marker_leaf(target)?;
        self.verify_unchanged()?;
        let current = self.open_marker(temporary, true)?;
        if identity_from_handle(&current)? != identity_from_handle(source)? {
            return Err(io::Error::other("path-changed"));
        }
        drop(current);
        #[cfg(windows)]
        {
            // A simple leaf and NULL RootDirectory is the documented native
            // same-directory form, including when the redirector serves SMB.
            // There is intentionally no unsupported-error/path-based fallback.
            nt_rename_information(None, source, target, false)?;
        }
        #[cfg(not(windows))]
        {
            rename_marker_noreplace(&self.handle, temporary, target)?;
        }
        self.verify_unchanged()
    }

    pub fn sync_all(&self) -> io::Result<()> {
        self.handle.sync_all()
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_marker_noreplace(
    parent: &fs::File,
    source: &std::ffi::OsStr,
    target: &std::ffi::OsStr,
) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;

    validate_marker_leaf(source)?;
    validate_marker_leaf(target)?;
    let source = CString::new(source.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid marker source name"))?;
    let target = CString::new(target.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid marker target name"))?;
    // Use the captured directory for both names. Native no-replace rename
    // works on supported FAT/CIFS filesystems without requiring hard links.
    // ENOSYS/EINVAL/EOPNOTSUPP remain errors; neither an overwriting rename nor
    // a link/unlink fallback can preserve this operation's contract.
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let result = unsafe {
        libc::renameat2(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_NOREPLACE as _,
        )
    };
    #[cfg(target_vendor = "apple")]
    let result = unsafe {
        libc::renameatx_np(
            parent.as_raw_fd(),
            source.as_ptr(),
            parent.as_raw_fd(),
            target.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(not(any(
    windows,
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple"
)))]
fn rename_marker_noreplace(
    _parent: &fs::File,
    _source: &std::ffi::OsStr,
    _target: &std::ffi::OsStr,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "atomic no-replace marker rename is unsupported on this platform",
    ))
}

fn validate_marker_leaf(leaf: &std::ffi::OsStr) -> io::Result<()> {
    let value = leaf.to_string_lossy();
    if value.is_empty()
        || value == "."
        || value == ".."
        || value.ends_with([' ', '.'])
        || value
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        || !matches!(
            Path::new(leaf).components().next(),
            Some(Component::Normal(_))
        )
        || Path::new(leaf).components().count() != 1
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid marker child name",
        ));
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        if leaf.encode_wide().count() > 255 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "marker child name too long",
            ));
        }
    }
    Ok(())
}

#[cfg(windows)]
fn open_marker_directory_chain(path: &Path) -> io::Result<(fs::File, Vec<fs::File>)> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::path::Prefix;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_WRITE,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_TRAVERSE, SYNCHRONIZE,
    };

    let invalid_path = || io::Error::new(io::ErrorKind::InvalidInput, "invalid marker root path");
    let mut components = path.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix))
            if matches!(
                prefix.kind(),
                Prefix::Disk(_)
                    | Prefix::VerbatimDisk(_)
                    | Prefix::UNC(_, _)
                    | Prefix::VerbatimUNC(_, _)
            ) =>
        {
            prefix
        }
        _ => return Err(invalid_path()),
    };
    if components.next() != Some(Component::RootDir) {
        return Err(invalid_path());
    }
    // Path::components normalizes interior "."; reject it before that can hide
    // unsupported input from the component-by-component walk.
    if components
        .as_path()
        .as_os_str()
        .to_string_lossy()
        .split(['\\', '/'])
        .any(|part| part == "." || part == "..")
    {
        return Err(invalid_path());
    }
    let mut names = Vec::new();
    for component in components {
        let Component::Normal(name) = component else {
            return Err(invalid_path());
        };
        validate_marker_leaf(name)?;
        names.push(name);
    }
    if names.is_empty() {
        return Err(invalid_path());
    }
    let anchor = PathBuf::from(prefix.as_os_str()).join(std::path::MAIN_SEPARATOR_STR);
    // LIST_DIRECTORY is material: an attribute-only handle does not reliably
    // participate in Windows share-access checks.
    let access = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE | SYNCHRONIZE;
    let sharing = FILE_SHARE_READ | FILE_SHARE_WRITE;
    let mut current = fs::OpenOptions::new()
        .access_mode(access)
        .share_mode(sharing)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(anchor)?;
    identity_from_handle(&current)?;
    let mut ancestors = Vec::with_capacity(names.len());
    for (index, name) in names.iter().enumerate() {
        let child_access = if index + 1 == names.len() {
            access | FILE_GENERIC_WRITE
        } else {
            access
        };
        let child =
            nt_open_relative_with_access(&current, name, true, false, child_access, sharing)?;
        identity_from_handle(&child)?;
        ancestors.push(current);
        current = child;
    }
    Ok((current, ancestors))
}

#[cfg(windows)]
fn nt_open_relative(
    parent: &fs::File,
    leaf: &std::ffi::OsStr,
    directory: bool,
    create: bool,
) -> io::Result<fs::File> {
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE, SYNCHRONIZE,
    };
    nt_open_relative_with_access(
        parent,
        leaf,
        directory,
        create,
        FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE,
        FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
    )
}

#[cfg(windows)]
fn nt_open_relative_with_access(
    parent: &fs::File,
    leaf: &std::ffi::OsStr,
    directory: bool,
    create: bool,
    access: u32,
    sharing: u32,
) -> io::Result<fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{RtlNtStatusToDosError, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::FILE_ATTRIBUTE_NORMAL;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    let mut name: Vec<u16> = leaf.encode_wide().collect();
    let mut unicode = UNICODE_STRING {
        Length: (name.len() * 2) as u16,
        MaximumLength: (name.len() * 2) as u16,
        Buffer: name.as_mut_ptr(),
    };
    let attributes = OBJECT_ATTRIBUTES {
        Length: std::mem::size_of::<OBJECT_ATTRIBUTES>() as u32,
        RootDirectory: parent.as_raw_handle(),
        ObjectName: &mut unicode,
        Attributes: 0,
        SecurityDescriptor: std::ptr::null_mut(),
        SecurityQualityOfService: std::ptr::null_mut(),
    };
    let mut raw = std::ptr::null_mut();
    let mut status = IO_STATUS_BLOCK::default();
    let options = if directory {
        FILE_DIRECTORY_FILE
    } else {
        FILE_NON_DIRECTORY_FILE
    } | FILE_OPEN_REPARSE_POINT
        | FILE_SYNCHRONOUS_IO_NONALERT;
    let disposition = if create { FILE_CREATE } else { FILE_OPEN };
    let result = unsafe {
        NtCreateFile(
            &mut raw,
            access,
            &attributes,
            &mut status,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            sharing,
            disposition,
            options,
            std::ptr::null(),
            0,
        )
    };
    if result < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ))
    } else {
        Ok(unsafe { fs::File::from_raw_handle(raw) })
    }
}

#[cfg(windows)]
fn create_relative_file(
    parent: &fs::File,
    _path: &Path,
    leaf: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    nt_open_relative(parent, leaf, false, true)
}

#[cfg(windows)]
fn create_relative_directory(
    parent: &fs::File,
    _path: &Path,
    leaf: &std::ffi::OsStr,
) -> io::Result<()> {
    drop(nt_open_relative(parent, leaf, true, true)?);
    Ok(())
}

#[cfg(not(windows))]
fn create_relative_file(
    _parent: &fs::File,
    path: &Path,
    leaf: &std::ffi::OsStr,
) -> io::Result<fs::File> {
    fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path.join(leaf))
}

#[cfg(unix)]
fn open_marker_relative(
    parent: &fs::File,
    leaf: &std::ffi::OsStr,
    writable: bool,
    create: bool,
) -> io::Result<fs::File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    validate_marker_leaf(leaf)?;
    let leaf = std::ffi::CString::new(leaf.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "invalid marker child name"))?;
    let mut flags = libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK;
    flags |= if writable {
        libc::O_RDWR
    } else {
        libc::O_RDONLY
    };
    if create {
        flags |= libc::O_CREAT | libc::O_EXCL;
    }
    let fd = unsafe {
        libc::openat(
            parent.as_raw_fd(),
            leaf.as_ptr(),
            flags,
            0o666 as libc::mode_t,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { fs::File::from_raw_fd(fd) })
}

#[cfg(not(any(windows, unix)))]
fn open_marker_relative(
    _parent: &fs::File,
    _leaf: &std::ffi::OsStr,
    _writable: bool,
    _create: bool,
) -> io::Result<fs::File> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "directory-relative marker open is unsupported on this platform",
    ))
}

#[cfg(not(windows))]
fn create_relative_directory(
    _parent: &fs::File,
    path: &Path,
    leaf: &std::ffi::OsStr,
) -> io::Result<()> {
    fs::create_dir(path.join(leaf))
}

#[cfg(windows)]
fn child_identity_relative(
    parent: &fs::File,
    _path: &Path,
    leaf: &std::ffi::OsStr,
    directory: bool,
) -> io::Result<String> {
    identity_from_handle(&nt_open_relative(parent, leaf, directory, false)?)
}

#[cfg(windows)]
fn delete_relative_file(parent: &fs::File, _path: &Path, leaf: &std::ffi::OsStr) -> io::Result<()> {
    let child = nt_open_relative(parent, leaf, false, false)?;
    delete_handle(&child)
}

#[cfg(not(windows))]
fn delete_relative_file(_parent: &fs::File, path: &Path, leaf: &std::ffi::OsStr) -> io::Result<()> {
    fs::remove_file(path.join(leaf))
}

#[cfg(not(windows))]
fn child_identity_relative(
    _parent: &fs::File,
    path: &Path,
    leaf: &std::ffi::OsStr,
    _directory: bool,
) -> io::Result<String> {
    identity_from_handle(&fs::File::open(path.join(leaf))?)
}

#[cfg(windows)]
fn rename_relative_child(
    parent: &fs::File,
    _path: &Path,
    source: &std::ffi::OsStr,
    target: &std::ffi::OsStr,
    replace: bool,
    directory: bool,
    expected_identity: Option<&str>,
) -> io::Result<()> {
    let source_handle = nt_open_relative(parent, source, directory, false)?;
    if expected_identity.is_some_and(|expected| {
        identity_from_handle(&source_handle).ok().as_deref() != Some(expected)
    }) {
        return Err(io::Error::new(io::ErrorKind::Other, "path-changed"));
    }
    nt_rename_relative(parent, &source_handle, target, replace)
}

#[cfg(windows)]
fn nt_rename_relative(
    parent: &fs::File,
    source_handle: &fs::File,
    target: &std::ffi::OsStr,
    replace: bool,
) -> io::Result<()> {
    nt_rename_information(Some(parent), source_handle, target, replace)
}

#[cfg(windows)]
fn nt_rename_information(
    parent: Option<&fs::File>,
    source_handle: &fs::File,
    target: &std::ffi::OsStr,
    replace: bool,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    let target: Vec<u16> = target.encode_wide().collect();
    let header = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let byte_len = header + target.len() * 2 + 2;
    let mut storage = vec![0u64; byte_len.div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).RootDirectory =
            parent.map_or(std::ptr::null_mut(), |handle| handle.as_raw_handle());
        (*info).FileNameLength = (target.len() * 2) as u32;
        std::ptr::copy_nonoverlapping(
            target.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            target.len(),
        );
        let result = NtSetInformationFile(
            source_handle.as_raw_handle(),
            &mut status,
            info.cast(),
            byte_len as u32,
            FileRenameInformation,
        );
        if result < 0 {
            return Err(io::Error::from_raw_os_error(
                RtlNtStatusToDosError(result) as i32
            ));
        }
    }
    Ok(())
}

#[cfg(not(windows))]
fn rename_relative_child(
    _parent: &fs::File,
    path: &Path,
    source: &std::ffi::OsStr,
    target: &std::ffi::OsStr,
    _replace: bool,
    _directory: bool,
    expected_identity: Option<&str>,
) -> io::Result<()> {
    if let Some(expected) = expected_identity {
        let source_handle = fs::File::open(path.join(source))?;
        if identity_from_handle(&source_handle)? != expected {
            return Err(io::Error::new(io::ErrorKind::Other, "path-changed"));
        }
    }
    fs::rename(path.join(source), path.join(target))
}

pub fn safe_rename_directory(source: &Path, target: &Path) -> io::Result<()> {
    safe_rename_directory_inner(source, target, None)
}

pub fn safe_rename_directory_expected(
    source: &Path,
    target: &Path,
    expected_identity: &str,
) -> io::Result<()> {
    safe_rename_directory_inner(source, target, Some(expected_identity))
}

fn safe_rename_directory_inner(
    source: &Path,
    target: &Path,
    expected_identity: Option<&str>,
) -> io::Result<()> {
    let source_parent = source
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "source has no parent"))?;
    let target_parent = target
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "target has no parent"))?;
    if source_parent != target_parent {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "cross-parent quarantine rename refused",
        ));
    }
    let guard = SafeDirectory::open(source_parent)?;
    match expected_identity {
        Some(expected) => guard.rename_directory_child_expected(
            source.file_name().unwrap(),
            target.file_name().unwrap(),
            expected,
        )?,
        None => guard
            .rename_directory_child(source.file_name().unwrap(), target.file_name().unwrap())?,
    }
    guard
        .verify_unchanged()
        .map_err(|_| io::Error::new(io::ErrorKind::Other, "path-changed"))
}

pub fn safe_delete_tree_expected(path: &Path, expected_identity: &str) -> io::Result<()> {
    safe_delete_tree_inner(path, Some(expected_identity))
}

fn safe_delete_tree_inner(path: &Path, expected_identity: Option<&str>) -> io::Result<()> {
    reject_reparse_components(path)?;
    #[cfg(windows)]
    {
        let guard = SafeDirectory::open(path)?;
        if expected_identity.is_some_and(|expected| guard.identity() != expected) {
            return Err(io::Error::new(io::ErrorKind::Other, "path-changed"));
        }
        guard
            .verify_unchanged()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "path-changed"))?;
        delete_directory_handle(&guard.handle)
    }
    #[cfg(not(windows))]
    {
        if let Some(expected) = expected_identity {
            let guard = SafeDirectory::open(path)?;
            if guard.identity() != expected {
                return Err(io::Error::new(io::ErrorKind::Other, "path-changed"));
            }
        }
        fs::remove_dir_all(path)
    }
}

#[cfg(windows)]
fn enumerate_directory_handle(directory: &fs::File) -> io::Result<Vec<(std::ffi::OsString, bool)>> {
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileIdBothDirectoryInformation, NtQueryDirectoryFile, FILE_ID_BOTH_DIR_INFORMATION,
    };
    use windows_sys::Win32::Foundation::{RtlNtStatusToDosError, STATUS_NO_MORE_FILES};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_DIRECTORY, FILE_ATTRIBUTE_REPARSE_POINT,
    };
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let mut entries = Vec::new();
    let mut restart = true;
    loop {
        let mut buffer = vec![0u64; 64 * 1024 / 8];
        let mut status = IO_STATUS_BLOCK::default();
        let result = unsafe {
            NtQueryDirectoryFile(
                directory.as_raw_handle(),
                std::ptr::null_mut(),
                None,
                std::ptr::null(),
                &mut status,
                buffer.as_mut_ptr().cast(),
                (buffer.len() * std::mem::size_of::<u64>()) as u32,
                FileIdBothDirectoryInformation,
                false,
                std::ptr::null(),
                restart,
            )
        };
        restart = false;
        if result == STATUS_NO_MORE_FILES {
            break;
        }
        if result < 0 {
            return Err(io::Error::from_raw_os_error(
                unsafe { RtlNtStatusToDosError(result) } as i32,
            ));
        }
        let mut offset = 0usize;
        loop {
            let info = unsafe {
                &*buffer
                    .as_ptr()
                    .cast::<u8>()
                    .add(offset)
                    .cast::<FILE_ID_BOTH_DIR_INFORMATION>()
            };
            let name = unsafe {
                std::slice::from_raw_parts(
                    std::ptr::addr_of!(info.FileName).cast::<u16>(),
                    info.FileNameLength as usize / 2,
                )
            };
            let name = std::ffi::OsString::from_wide(name);
            if name != "." && name != ".." {
                if info.FileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "reparse point refused",
                    ));
                }
                entries.push((name, info.FileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0));
            }
            if info.NextEntryOffset == 0 {
                break;
            }
            offset += info.NextEntryOffset as usize;
        }
    }
    Ok(entries)
}

#[cfg(windows)]
fn delete_handle(handle: &fs::File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileDispositionInformationEx, NtSetInformationFile, FILE_DISPOSITION_DELETE,
        FILE_DISPOSITION_INFORMATION_EX, FILE_DISPOSITION_POSIX_SEMANTICS,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    let disposition = FILE_DISPOSITION_INFORMATION_EX {
        Flags: FILE_DISPOSITION_DELETE | FILE_DISPOSITION_POSIX_SEMANTICS,
    };
    let mut status = IO_STATUS_BLOCK::default();
    let result = unsafe {
        NtSetInformationFile(
            handle.as_raw_handle(),
            &mut status,
            (&disposition as *const FILE_DISPOSITION_INFORMATION_EX).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFORMATION_EX>() as u32,
            FileDispositionInformationEx,
        )
    };
    if result < 0 {
        Err(io::Error::from_raw_os_error(
            unsafe { RtlNtStatusToDosError(result) } as i32,
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn delete_directory_handle(directory: &fs::File) -> io::Result<()> {
    for (name, is_directory) in enumerate_directory_handle(directory)? {
        let child = nt_open_relative(directory, &name, is_directory, false)?;
        if is_directory {
            delete_directory_handle(&child)?;
        } else {
            delete_handle(&child)?;
        }
    }
    delete_handle(directory)
}

fn reject_reparse_components(path: &Path) -> io::Result<()> {
    if has_existing_reparse_component(path) {
        Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reparse point refused",
        ))
    } else {
        Ok(())
    }
}

#[cfg(windows)]
fn open_directory_nofollow(path: &Path) -> io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_GENERIC_READ,
        FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    fs::OpenOptions::new()
        .access_mode(FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

#[cfg(not(windows))]
fn open_directory_nofollow(path: &Path) -> io::Result<fs::File> {
    fs::File::open(path)
}

#[cfg(windows)]
fn identity_from_handle(handle: &fs::File) -> io::Result<String> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
    };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if info.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reparse point refused",
        ));
    }
    let index = (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow);
    Ok(format!("windows:{}:{index}", info.dwVolumeSerialNumber))
}

#[cfg(unix)]
fn identity_from_handle(handle: &fs::File) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = handle.metadata()?;
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn identity_from_handle(handle: &fs::File) -> io::Result<String> {
    Ok(format!("portable:{}", handle.metadata()?.len()))
}

pub fn read_marker(root: &Path) -> anyhow::Result<LibraryMarker> {
    Ok(serde_json::from_slice(&fs::read(
        root.join(MARKER_FILE_NAME),
    )?)?)
}

pub fn marker_identity(path: &Path) -> anyhow::Result<String> {
    Ok(platform_identity(path)?)
}

pub fn known_cloud_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for name in [
        "OneDrive",
        "OneDriveConsumer",
        "OneDriveCommercial",
        "Dropbox",
        "GoogleDrive",
    ] {
        if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
            roots.push(PathBuf::from(value));
        }
    }
    roots.extend(platform_known_cloud_roots());
    roots.sort();
    roots.dedup();
    roots
}

#[cfg(windows)]
fn platform_known_cloud_roots() -> Vec<PathBuf> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::System::Com::CoTaskMemFree;
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegEnumKeyExW, RegEnumValueW, RegOpenKeyExW, RegQueryValueExW, HKEY,
        HKEY_CURRENT_USER, KEY_READ, REG_EXPAND_SZ, REG_SZ,
    };
    use windows_sys::Win32::UI::Shell::{FOLDERID_SkyDrive, SHGetKnownFolderPath};

    fn wide(value: &str) -> Vec<u16> {
        value.encode_utf16().chain(Some(0)).collect()
    }
    unsafe fn query_string(key: HKEY, name: &str) -> Option<PathBuf> {
        let name = wide(name);
        let mut kind = 0;
        let mut bytes = 0u32;
        if unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                std::ptr::null_mut(),
                &mut bytes,
            )
        } != 0
            || kind != REG_SZ
            || bytes < 2
        {
            return None;
        }
        let mut data = vec![0u16; bytes as usize / 2];
        if unsafe {
            RegQueryValueExW(
                key,
                name.as_ptr(),
                std::ptr::null(),
                &mut kind,
                data.as_mut_ptr().cast(),
                &mut bytes,
            )
        } != 0
        {
            return None;
        }
        while data.last() == Some(&0) {
            data.pop();
        }
        Some(PathBuf::from(std::ffi::OsString::from_wide(&data)))
    }

    let mut roots = Vec::new();
    let mut known = std::ptr::null_mut();
    if unsafe { SHGetKnownFolderPath(&FOLDERID_SkyDrive, 0, std::ptr::null_mut(), &mut known) } >= 0
        && !known.is_null()
    {
        let mut length = 0usize;
        while unsafe { *known.add(length) } != 0 {
            length += 1;
        }
        roots.push(PathBuf::from(std::ffi::OsString::from_wide(unsafe {
            std::slice::from_raw_parts(known, length)
        })));
        unsafe { CoTaskMemFree(known.cast()) };
    }
    let sync_key = wide(r"Software\Microsoft\OneDrive\Accounts");
    let mut accounts: HKEY = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sync_key.as_ptr(),
            0,
            KEY_READ,
            &mut accounts,
        )
    } == 0
    {
        for index in 0..64u32 {
            let mut name = vec![0u16; 256];
            let mut length = name.len() as u32;
            if unsafe {
                RegEnumKeyExW(
                    accounts,
                    index,
                    name.as_mut_ptr(),
                    &mut length,
                    std::ptr::null(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            } != 0
            {
                break;
            }
            name.truncate(length as usize);
            name.push(0);
            let mut account: HKEY = std::ptr::null_mut();
            if unsafe { RegOpenKeyExW(accounts, name.as_ptr(), 0, KEY_READ, &mut account) } == 0 {
                if let Some(path) = unsafe { query_string(account, "UserFolder") } {
                    roots.push(path);
                }
                unsafe { RegCloseKey(account) };
            }
        }
        unsafe { RegCloseKey(accounts) };
    }
    // Cloud Files providers (OneDrive, Dropbox, Google Drive, etc.) register
    // each hydrated sync root here. Enumerating these values avoids relying on
    // provider-specific environment variables or account layouts.
    let sync_roots_key =
        wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\SyncRootManager");
    let mut sync_roots: HKEY = std::ptr::null_mut();
    if unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            sync_roots_key.as_ptr(),
            0,
            KEY_READ,
            &mut sync_roots,
        )
    } == 0
    {
        for index in 0..1024u32 {
            let mut name = vec![0u16; 1024];
            let mut name_length = name.len() as u32;
            let mut kind = 0u32;
            let mut data = vec![0u16; 32768];
            let mut data_bytes = (data.len() * 2) as u32;
            let result = unsafe {
                RegEnumValueW(
                    sync_roots,
                    index,
                    name.as_mut_ptr(),
                    &mut name_length,
                    std::ptr::null(),
                    &mut kind,
                    data.as_mut_ptr().cast(),
                    &mut data_bytes,
                )
            };
            if result != 0 {
                break;
            }
            if kind == REG_SZ || kind == REG_EXPAND_SZ {
                data.truncate(data_bytes as usize / 2);
                while data.last() == Some(&0) {
                    data.pop();
                }
                if !data.is_empty() {
                    roots.push(PathBuf::from(std::ffi::OsString::from_wide(&data)));
                }
            }
        }
        unsafe { RegCloseKey(sync_roots) };
    }
    roots
}

#[cfg(windows)]
pub(crate) fn is_platform_sync_root(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::CloudFilters::{
        CfGetSyncRootInfoByPath, CF_SYNC_ROOT_INFO_BASIC,
    };
    let existing = if path.exists() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let wide: Vec<u16> = existing.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut buffer = [0u8; 1024];
    let mut returned = 0u32;
    unsafe {
        CfGetSyncRootInfoByPath(
            wide.as_ptr(),
            CF_SYNC_ROOT_INFO_BASIC,
            buffer.as_mut_ptr().cast(),
            buffer.len() as u32,
            &mut returned,
        ) >= 0
    }
}

#[cfg(not(windows))]
pub(crate) fn is_platform_sync_root(_path: &Path) -> bool {
    false
}

#[cfg(not(windows))]
fn platform_known_cloud_roots() -> Vec<PathBuf> {
    Vec::new()
}

pub(crate) fn probe_writable(root: &Path) -> bool {
    let parent = if root.exists() {
        root
    } else {
        root.parent().unwrap_or(root)
    };
    let probe = parent.join(format!(".megle-write-probe-{}", uuid::Uuid::now_v7()));
    match fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(file) => {
            drop(file);
            fs::remove_file(&probe).is_ok()
        }
        Err(_) => false,
    }
}

#[cfg(windows)]
pub(crate) fn free_space_bytes(path: &Path) -> io::Result<u64> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    let existing = if path.exists() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let wide: Vec<u16> = existing.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut available = 0u64;
    if unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut available,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(available)
    }
}

#[cfg(unix)]
pub(crate) fn free_space_bytes(path: &Path) -> io::Result<u64> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    let existing = if path.exists() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    let name = CString::new(existing.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL"))?;
    let mut stats = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(name.as_ptr(), stats.as_mut_ptr()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let stats = unsafe { stats.assume_init() };
    Ok(stats.f_bavail.saturating_mul(stats.f_frsize))
}

#[cfg(not(any(windows, unix)))]
pub(crate) fn free_space_bytes(_path: &Path) -> io::Result<u64> {
    Ok(u64::MAX)
}

pub fn paths_overlap(left: &Path, right: &Path) -> bool {
    let left = left
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase();
    let right = right
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_lowercase();
    left == right
        || left
            .strip_prefix(&right)
            .is_some_and(|tail| tail.starts_with(['\\', '/']))
        || right
            .strip_prefix(&left)
            .is_some_and(|tail| tail.starts_with(['\\', '/']))
}

fn rejected(path: &Path, reason: PreflightReason) -> PreflightResult {
    PreflightResult {
        ok: false,
        reason,
        normalized_root: path.to_path_buf(),
        token: None,
        available_bytes: None,
    }
}

pub(crate) fn has_existing_reparse_component(path: &Path) -> bool {
    let mut current = PathBuf::new();
    for component in path.components() {
        current.push(component);
        if current.exists()
            && fs::symlink_metadata(&current)
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return true;
        }
        #[cfg(windows)]
        if current.exists()
            && fs::symlink_metadata(&current).is_ok_and(|metadata| {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            })
        {
            return true;
        }
    }
    false
}

#[cfg(windows)]
fn platform_identity(path: &Path) -> io::Result<String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_REPARSE_POINT,
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let handle = fs::OpenOptions::new()
        .access_mode(FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut information) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "reparse point refused",
        ));
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(format!(
        "windows:{}:{file_index}",
        information.dwVolumeSerialNumber
    ))
}

#[cfg(unix)]
fn platform_identity(path: &Path) -> io::Result<String> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "symbolic link refused",
        ));
    }
    Ok(format!("unix:{}:{}", metadata.dev(), metadata.ino()))
}

#[cfg(not(any(windows, unix)))]
fn platform_identity(path: &Path) -> io::Result<String> {
    Ok(format!("portable:{}", fs::symlink_metadata(path)?.len()))
}

fn hex(bytes: &[u8]) -> String {
    const TABLE: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(TABLE[(byte >> 4) as usize] as char);
        output.push(TABLE[(byte & 0xf) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlap_detects_same_parent_and_child_case_insensitively() {
        assert!(paths_overlap(
            Path::new("C:/Users/A/Library"),
            Path::new("c:/users/a/library/media")
        ));
        assert!(!paths_overlap(
            Path::new("C:/Users/A/Library"),
            Path::new("C:/Users/A/Library2")
        ));
    }

    #[test]
    fn windows_reserved_library_names_are_refused() {
        assert_eq!(safe_library_name("CON"), Err(PreflightReason::InvalidName));
        assert_eq!(safe_library_name("valid library").unwrap(), "valid library");
    }

    #[test]
    fn marker_identity_is_filesystem_identity_not_marker_contents() {
        let base =
            std::env::temp_dir().join(format!("megle-marker-identity-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let marker = base.join(MARKER_FILE_NAME);
        fs::write(&marker, b"first").unwrap();
        let first = marker_identity(&marker).unwrap();
        fs::write(&marker, b"second marker contents").unwrap();
        assert_eq!(marker_identity(&marker).unwrap(), first);
        fs::rename(&marker, base.join("old-marker")).unwrap();
        fs::write(&marker, b"replacement").unwrap();
        assert_ne!(marker_identity(&marker).unwrap(), first);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn safe_directory_detects_path_replacement_after_handle_capture() {
        let base = std::env::temp_dir().join(format!("megle-path-race-{}", uuid::Uuid::now_v7()));
        let original = base.join("root");
        let moved = base.join("moved");
        fs::create_dir_all(&original).unwrap();
        let guard = SafeDirectory::open(&original).unwrap();
        fs::rename(&original, &moved).unwrap();
        fs::create_dir(&original).unwrap();
        assert_eq!(guard.verify_unchanged(), Err(PreflightReason::PathChanged));
        drop(guard);
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn writable_probe_is_deleted_after_preflight() {
        let base =
            std::env::temp_dir().join(format!("megle-preflight-probe-{}", uuid::Uuid::now_v7()));
        fs::create_dir_all(&base).unwrap();
        let result = preflight("Library", &base, &[], &[]);
        assert!(result.ok);
        assert!(fs::read_dir(&base).unwrap().next().is_none());
        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn marker_guard_rejects_non_leaf_names() {
        for name in [
            "", ".", "..", "a/b", r"a\b", "a:stream", "name.", "name ", "a\0b", "a*b", "a?b",
        ] {
            assert!(
                validate_marker_leaf(std::ffi::OsStr::new(name)).is_err(),
                "accepted {name:?}"
            );
        }
        assert!(validate_marker_leaf(std::ffi::OsStr::new(".megle-sync-root")).is_ok());
    }

    #[cfg(windows)]
    #[test]
    fn marker_guard_refuses_traversal_and_reparse_components() {
        let base =
            std::env::temp_dir().join(format!("megle-marker-reparse-{}", uuid::Uuid::now_v7()));
        let ordinary = base.join("ordinary");
        let other = base.join("other");
        let link = base.join("link");
        fs::create_dir_all(ordinary.join("root")).unwrap();
        fs::create_dir(&other).unwrap();
        for path in [ordinary.join("..\\other"), ordinary.join(".\\root")] {
            assert_eq!(
                MarkerDirectoryGuard::open(&path).err().unwrap().kind(),
                io::ErrorKind::InvalidInput
            );
        }
        let result = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&link)
            .arg(&ordinary)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "create isolated directory junction fixture"
        );
        assert_eq!(
            MarkerDirectoryGuard::open(&link).err().unwrap().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            MarkerDirectoryGuard::open(&link.join("root"))
                .err()
                .unwrap()
                .kind(),
            io::ErrorKind::PermissionDenied
        );
        // Remove the junction entry explicitly before its owned fixture tree.
        fs::remove_dir(&link).unwrap();
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn marker_guard_refuses_a_different_source_handle() {
        let root =
            std::env::temp_dir().join(format!("megle-marker-source-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&root).unwrap();
        let directory = MarkerDirectoryGuard::open(&root).unwrap();
        let temporary = std::ffi::OsStr::new("operation.tmp");
        let mut correct = directory.open_temporary(temporary, true).unwrap();
        correct.write_all(b"verified").unwrap();
        let other = directory
            .open_temporary(std::ffi::OsStr::new("other.tmp"), true)
            .unwrap();
        let error = directory
            .publish(&other, temporary, std::ffi::OsStr::new(".megle-sync-root"))
            .unwrap_err();
        assert_eq!(error.to_string(), "path-changed");
        assert!(!root.join(".megle-sync-root").exists());
        assert_eq!(fs::read(root.join(temporary)).unwrap(), b"verified");
        drop(correct);
        drop(other);
        drop(directory);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn marker_child_opens_stay_on_the_captured_directory_after_path_replacement() {
        use std::io::Read;

        let base = std::env::temp_dir().join(format!("megle-marker-open-{}", uuid::Uuid::now_v7()));
        let root = base.join("root");
        let moved = base.join("moved");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("existing.tmp"), b"original").unwrap();
        let directory = MarkerDirectoryGuard::open(&root).unwrap();
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("existing.tmp"), b"replacement").unwrap();

        let temporary = std::ffi::OsStr::new("operation.tmp");
        let mut file = directory.open_temporary(temporary, true).unwrap();
        file.write_all(b"owned").unwrap();
        assert!(
            !root.join(temporary).exists(),
            "a captured marker guard must never create in a replaced path"
        );
        assert_eq!(fs::read(moved.join(temporary)).unwrap(), b"owned");
        let mut existing = directory
            .open_marker(std::ffi::OsStr::new("existing.tmp"), false)
            .unwrap();
        let mut bytes = Vec::new();
        existing.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"original");
        assert!(directory.verify_unchanged().is_err());

        let outside = base.join("outside");
        fs::write(&outside, b"untouched").unwrap();
        std::os::unix::fs::symlink(&outside, moved.join("symlink.tmp")).unwrap();
        let symlink = std::ffi::OsStr::new("symlink.tmp");
        assert!(directory.open_temporary(symlink, false).is_err());
        assert!(directory.open_marker(symlink, false).is_err());
        assert_eq!(fs::read(outside).unwrap(), b"untouched");
        drop(existing);
        drop(file);
        drop(directory);
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
    #[test]
    fn marker_native_rename_uses_captured_directory_and_never_replaces() {
        let base =
            std::env::temp_dir().join(format!("megle-marker-native-{}", uuid::Uuid::now_v7()));
        let root = base.join("root");
        let moved = base.join("moved");
        fs::create_dir_all(&root).unwrap();
        let parent = fs::File::open(&root).unwrap();
        fs::write(root.join("source"), b"original").unwrap();
        fs::write(root.join("occupied"), b"untouched").unwrap();
        fs::rename(&root, &moved).unwrap();
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source"), b"replacement").unwrap();

        let error = rename_marker_noreplace(
            &parent,
            std::ffi::OsStr::new("source"),
            std::ffi::OsStr::new("occupied"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(moved.join("source")).unwrap(), b"original");
        assert_eq!(fs::read(moved.join("occupied")).unwrap(), b"untouched");
        rename_marker_noreplace(
            &parent,
            std::ffi::OsStr::new("source"),
            std::ffi::OsStr::new("published"),
        )
        .unwrap();
        assert!(!moved.join("source").exists());
        assert_eq!(fs::read(moved.join("published")).unwrap(), b"original");
        assert_eq!(fs::read(root.join("source")).unwrap(), b"replacement");
        assert!(!root.join("published").exists());
        drop(parent);
        fs::remove_dir_all(base).unwrap();
    }

    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    #[test]
    fn marker_native_publication_succeeds_without_hard_link_support() {
        let root =
            std::env::temp_dir().join(format!("megle-marker-no-links-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&root).unwrap();
        let test_root = root.clone();
        // Isolate the filter to this child thread. Returning EOPNOTSUPP for
        // link/linkat simulates the capability missing on FAT and some shares;
        // this is not a substitute for a real filesystem compatibility run.
        std::thread::spawn(move || {
            let mut filter = [
                libc::sock_filter {
                    code: (libc::BPF_LD | libc::BPF_W | libc::BPF_ABS) as u16,
                    jt: 0,
                    jf: 0,
                    k: 0,
                },
                libc::sock_filter {
                    code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                    jt: 1,
                    jf: 0,
                    k: libc::SYS_link as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_JMP | libc::BPF_JEQ | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 1,
                    k: libc::SYS_linkat as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ERRNO | libc::EOPNOTSUPP as u32,
                },
                libc::sock_filter {
                    code: (libc::BPF_RET | libc::BPF_K) as u16,
                    jt: 0,
                    jf: 0,
                    k: libc::SECCOMP_RET_ALLOW,
                },
            ];
            let program = libc::sock_fprog {
                len: filter.len() as u16,
                filter: filter.as_mut_ptr(),
            };
            assert_eq!(
                unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) },
                0
            );
            assert_eq!(
                unsafe { libc::prctl(libc::PR_SET_SECCOMP, libc::SECCOMP_MODE_FILTER, &program) },
                0
            );

            let directory = MarkerDirectoryGuard::open(&test_root).unwrap();
            let temporary = std::ffi::OsStr::new("operation.tmp");
            let final_name = std::ffi::OsStr::new(".megle-sync-root");
            let mut source = directory.open_temporary(temporary, true).unwrap();
            source.write_all(b"marker").unwrap();
            source.sync_all().unwrap();
            let identity = directory.file_identity(&source).unwrap();
            assert_eq!(
                fs::hard_link(test_root.join(temporary), test_root.join("link-probe"))
                    .unwrap_err()
                    .raw_os_error(),
                Some(libc::EOPNOTSUPP)
            );
            directory.publish(&source, temporary, final_name).unwrap();
            directory.sync_all().unwrap();
            assert!(!test_root.join(temporary).exists());
            assert_eq!(fs::read(test_root.join(final_name)).unwrap(), b"marker");
            let published = directory.open_marker(final_name, true).unwrap();
            assert_eq!(directory.file_identity(&published).unwrap(), identity);
        })
        .join()
        .unwrap();
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(not(any(
        windows,
        target_os = "linux",
        target_os = "android",
        target_vendor = "apple"
    )))]
    #[test]
    fn marker_native_rename_is_explicitly_unsupported_without_an_atomic_primitive() {
        let root =
            std::env::temp_dir().join(format!("megle-marker-unsupported-{}", uuid::Uuid::now_v7()));
        fs::create_dir(&root).unwrap();
        fs::write(root.join("source"), b"original").unwrap();
        let parent = fs::File::open(&root).unwrap();
        let error = rename_marker_noreplace(
            &parent,
            std::ffi::OsStr::new("source"),
            std::ffi::OsStr::new("target"),
        )
        .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::Unsupported);
        assert_eq!(fs::read(root.join("source")).unwrap(), b"original");
        assert!(!root.join("target").exists());
        drop(parent);
        fs::remove_dir_all(root).unwrap();
    }
}
