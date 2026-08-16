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

#[cfg(windows)]
fn nt_open_relative(
    parent: &fs::File,
    leaf: &std::ffi::OsStr,
    directory: bool,
    create: bool,
) -> io::Result<fs::File> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Wdk::Foundation::OBJECT_ATTRIBUTES;
    use windows_sys::Wdk::Storage::FileSystem::{
        NtCreateFile, FILE_CREATE, FILE_DIRECTORY_FILE, FILE_NON_DIRECTORY_FILE, FILE_OPEN,
        FILE_OPEN_REPARSE_POINT, FILE_SYNCHRONOUS_IO_NONALERT,
    };
    use windows_sys::Win32::Foundation::{RtlNtStatusToDosError, UNICODE_STRING};
    use windows_sys::Win32::Storage::FileSystem::{
        DELETE, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE,
        FILE_SHARE_READ, FILE_SHARE_WRITE, SYNCHRONIZE,
    };
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
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE | SYNCHRONIZE,
            &attributes,
            &mut status,
            std::ptr::null(),
            FILE_ATTRIBUTE_NORMAL,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
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
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;
    let source_handle = nt_open_relative(parent, source, directory, false)?;
    if expected_identity.is_some_and(|expected| {
        identity_from_handle(&source_handle).ok().as_deref() != Some(expected)
    }) {
        return Err(io::Error::new(io::ErrorKind::Other, "path-changed"));
    }
    let target: Vec<u16> = target.encode_wide().collect();
    let header = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let byte_len = header + target.len() * 2 + 2;
    let mut storage = vec![0u64; byte_len.div_ceil(8)];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut status = IO_STATUS_BLOCK::default();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = replace;
        (*info).RootDirectory = parent.as_raw_handle();
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
fn is_platform_sync_root(path: &Path) -> bool {
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
fn is_platform_sync_root(_path: &Path) -> bool {
    false
}

#[cfg(not(windows))]
fn platform_known_cloud_roots() -> Vec<PathBuf> {
    Vec::new()
}

fn probe_writable(root: &Path) -> bool {
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
fn free_space_bytes(path: &Path) -> io::Result<u64> {
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
fn free_space_bytes(path: &Path) -> io::Result<u64> {
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
fn free_space_bytes(_path: &Path) -> io::Result<u64> {
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

fn has_existing_reparse_component(path: &Path) -> bool {
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
}
