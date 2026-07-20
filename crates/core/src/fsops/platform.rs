use std::ffi::OsStr;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

pub(super) struct VerifiedSource {
    path: PathBuf,
    identity: String,
    is_directory: bool,
    is_reparse_or_link: bool,
    #[cfg(windows)]
    handle: fs::File,
}

pub(super) struct RecycleIdentityGuard {
    identity: String,
    #[cfg(windows)]
    handle: fs::File,
}

impl RecycleIdentityGuard {
    pub(super) fn verify_shell_handoff(&self, expected_identity: &str) -> io::Result<()> {
        if self.identity != expected_identity {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "recycle guard identity does not match the journal",
            ));
        }
        verify_shell_handoff(self)
    }
}

#[derive(Debug)]
pub(super) struct VerifiedTargetParent {
    #[cfg_attr(windows, allow(dead_code))]
    path: PathBuf,
    identity: String,
    #[cfg(any(windows, unix))]
    handle: fs::File,
}

impl VerifiedTargetParent {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        open_verified_target_parent(path)
    }

    pub(super) fn identity(&self) -> &str {
        &self.identity
    }
}

impl VerifiedSource {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        open_verified_source(path)
    }

    pub(super) fn identity(&self) -> &str {
        &self.identity
    }

    pub(super) fn rename_no_replace(
        &self,
        target_parent: &VerifiedTargetParent,
        target_name: &OsStr,
    ) -> io::Result<()> {
        validate_target_leaf(target_name)?;
        rename_no_replace(self, target_parent, target_name)
    }

    pub(super) fn delete_permanent(self) -> io::Result<()> {
        if self.is_reparse_or_link {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "permanent delete refused for link or reparse point: {}",
                    self.path.display()
                ),
            ));
        }
        delete_permanent(self)
    }

    pub(super) fn recycle_identity_guard(&self) -> io::Result<RecycleIdentityGuard> {
        recycle_identity_guard(self)
    }
}

fn validate_target_leaf(target_name: &OsStr) -> io::Result<()> {
    let path = Path::new(target_name);
    if target_name.is_empty()
        || path.is_absolute()
        || path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "rename target must be a single relative path component",
        ));
    }
    Ok(())
}

#[cfg(windows)]
fn open_verified_source(path: &Path) -> io::Result<VerifiedSource> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, DELETE, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let handle = fs::OpenOptions::new()
        .access_mode(DELETE | FILE_READ_ATTRIBUTES)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut information as *mut _) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(VerifiedSource {
        path: path.to_path_buf(),
        identity: format!("windows:{}:{file_index}", information.dwVolumeSerialNumber),
        is_directory: information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY != 0,
        is_reparse_or_link: information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0,
        handle,
    })
}

#[cfg(windows)]
fn open_verified_target_parent(path: &Path) -> io::Result<VerifiedTargetParent> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, BY_HANDLE_FILE_INFORMATION, FILE_ATTRIBUTE_DIRECTORY,
        FILE_ATTRIBUTE_REPARSE_POINT, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT,
        FILE_LIST_DIRECTORY, FILE_READ_ATTRIBUTES, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FILE_TRAVERSE,
    };

    let handle = fs::OpenOptions::new()
        .access_mode(FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES | FILE_TRAVERSE)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe { GetFileInformationByHandle(handle.as_raw_handle(), &mut information as *mut _) }
        == 0
    {
        return Err(io::Error::last_os_error());
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "rename target parent is not a directory: {}",
                path.display()
            ),
        ));
    }
    if information.dwFileAttributes & FILE_ATTRIBUTE_REPARSE_POINT != 0 {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "rename target parent is a link or reparse point: {}",
                path.display()
            ),
        ));
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    Ok(VerifiedTargetParent {
        path: path.to_path_buf(),
        identity: format!("windows:{}:{file_index}", information.dwVolumeSerialNumber),
        handle,
    })
}

#[cfg(windows)]
fn recycle_identity_guard(source: &VerifiedSource) -> io::Result<RecycleIdentityGuard> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle};
    use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
    use windows_sys::Win32::Storage::FileSystem::{
        ReOpenFile, FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    let raw = unsafe {
        ReOpenFile(
            source.handle.as_raw_handle(),
            FILE_READ_ATTRIBUTES,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT,
        )
    };
    if raw == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(RecycleIdentityGuard {
        identity: source.identity.clone(),
        handle: unsafe { fs::File::from_raw_handle(raw) },
    })
}

#[cfg(windows)]
fn verify_shell_handoff(guard: &RecycleIdentityGuard) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
        FILE_NAME_NORMALIZED, VOLUME_NAME_DOS,
    };

    let mut information = BY_HANDLE_FILE_INFORMATION::default();
    if unsafe {
        GetFileInformationByHandle(guard.handle.as_raw_handle(), &mut information as *mut _)
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let file_index =
        (u64::from(information.nFileIndexHigh) << 32) | u64::from(information.nFileIndexLow);
    let current_identity = format!("windows:{}:{file_index}", information.dwVolumeSerialNumber);
    if current_identity != guard.identity {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "recycle guard changed filesystem identity",
        ));
    }

    let mut path = vec![0u16; 32_768];
    let length = unsafe {
        GetFinalPathNameByHandleW(
            guard.handle.as_raw_handle(),
            path.as_mut_ptr(),
            path.len() as u32,
            FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
        )
    };
    if length == 0 {
        return Err(io::Error::last_os_error());
    }
    if length as usize >= path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "recycle destination path exceeded the verification buffer",
        ));
    }
    let final_path = String::from_utf16_lossy(&path[..length as usize]).to_ascii_lowercase();
    if !final_path.contains("\\$recycle.bin\\") {
        return Err(io::Error::new(
            io::ErrorKind::Interrupted,
            format!("Shell did not move the guarded object into the recycle bin: {final_path}"),
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn open_verified_source(path: &Path) -> io::Result<VerifiedSource> {
    use std::os::unix::fs::{FileTypeExt, MetadataExt};

    let metadata = fs::symlink_metadata(path)?;
    Ok(VerifiedSource {
        path: path.to_path_buf(),
        identity: format!("unix:{}:{}", metadata.dev(), metadata.ino()),
        is_directory: metadata.is_dir(),
        is_reparse_or_link: metadata.file_type().is_symlink()
            || metadata.file_type().is_block_device()
            || metadata.file_type().is_char_device(),
    })
}

#[cfg(unix)]
fn open_verified_target_parent(path: &Path) -> io::Result<VerifiedTargetParent> {
    use std::os::unix::fs::{MetadataExt, OpenOptionsExt};

    let link_metadata = fs::symlink_metadata(path)?;
    if link_metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!("rename target parent is a symlink: {}", path.display()),
        ));
    }
    let handle = fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(path)?;
    let metadata = handle.metadata()?;
    if !metadata.is_dir()
        || metadata.dev() != link_metadata.dev()
        || metadata.ino() != link_metadata.ino()
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "rename target parent changed while opening: {}",
                path.display()
            ),
        ));
    }
    Ok(VerifiedTargetParent {
        path: path.to_path_buf(),
        identity: format!("unix:{}:{}", metadata.dev(), metadata.ino()),
        handle,
    })
}

#[cfg(not(windows))]
fn recycle_identity_guard(source: &VerifiedSource) -> io::Result<RecycleIdentityGuard> {
    Ok(RecycleIdentityGuard {
        identity: source.identity.clone(),
    })
}

#[cfg(not(windows))]
fn verify_shell_handoff(_guard: &RecycleIdentityGuard) -> io::Result<()> {
    Ok(())
}

#[cfg(not(any(windows, unix)))]
fn open_verified_source(path: &Path) -> io::Result<VerifiedSource> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "stable file-operation identity is unsupported on this platform: {}",
            path.display()
        ),
    ))
}

#[cfg(not(any(windows, unix)))]
fn open_verified_target_parent(path: &Path) -> io::Result<VerifiedTargetParent> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "stable target-parent identity is unsupported on this platform: {}",
            path.display()
        ),
    ))
}

#[cfg(windows)]
fn rename_no_replace(
    source: &VerifiedSource,
    target_parent: &VerifiedTargetParent,
    target_name: &OsStr,
) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Wdk::Storage::FileSystem::{
        FileRenameInformation, NtSetInformationFile, FILE_RENAME_INFORMATION,
    };
    use windows_sys::Win32::Foundation::RtlNtStatusToDosError;
    use windows_sys::Win32::System::IO::IO_STATUS_BLOCK;

    let target_wide: Vec<u16> = target_name.encode_wide().collect();
    let header_len = std::mem::offset_of!(FILE_RENAME_INFORMATION, FileName);
    let file_name_byte_len = target_wide.len().saturating_mul(std::mem::size_of::<u16>());
    let byte_len = header_len
        .checked_add(file_name_byte_len)
        .and_then(|length| length.checked_add(std::mem::size_of::<u16>()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
    let word_len = byte_len.div_ceil(std::mem::size_of::<u64>());
    let mut storage = vec![0u64; word_len];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFORMATION>();
    let mut io_status = IO_STATUS_BLOCK::default();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = target_parent.handle.as_raw_handle();
        (*info).FileNameLength = u32::try_from(file_name_byte_len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long")
        })?;
        std::ptr::copy_nonoverlapping(
            target_wide.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            target_wide.len(),
        );
        // The Win32 wrapper rejects a non-null RootDirectory on supported
        // Windows builds. The native information class honors the documented
        // directory-handle + relative-leaf contract and keeps both handles
        // bound through the rename syscall.
        let status = NtSetInformationFile(
            source.handle.as_raw_handle(),
            &mut io_status,
            info.cast(),
            u32::try_from(byte_len).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?,
            FileRenameInformation,
        );
        if status < 0 {
            return Err(io::Error::from_raw_os_error(
                RtlNtStatusToDosError(status) as i32
            ));
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_no_replace(
    source: &VerifiedSource,
    target_parent: &VerifiedTargetParent,
    target_name: &OsStr,
) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::AsRawFd;

    let source_path = CString::new(source.path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target_leaf = CString::new(target_name.as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target name contains NUL"))?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source_path.as_ptr(),
            target_parent.handle.as_raw_fd(),
            target_leaf.as_ptr(),
            libc::RENAME_NOREPLACE,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(target_vendor = "apple")]
fn rename_no_replace(
    source: &VerifiedSource,
    target_parent: &VerifiedTargetParent,
    target_name: &OsStr,
) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source_path = CString::new(source.path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target = target_parent.path.join(target_name);
    let target_path = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path contains NUL"))?;
    let result = unsafe {
        libc::renamex_np(
            source_path.as_ptr(),
            target_path.as_ptr(),
            libc::RENAME_EXCL,
        )
    };
    if result == 0 {
        Ok(())
    } else {
        Err(io::Error::last_os_error())
    }
}

#[cfg(all(
    unix,
    not(any(target_os = "linux", target_os = "android")),
    not(target_vendor = "apple")
))]
fn rename_no_replace(
    source: &VerifiedSource,
    target_parent: &VerifiedTargetParent,
    target_name: &OsStr,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "atomic no-replace rename is unsupported on this platform: {} -> {}",
            source.path.display(),
            target_parent.path.join(target_name).display()
        ),
    ))
}

#[cfg(not(any(windows, unix)))]
fn rename_no_replace(
    source: &VerifiedSource,
    target_parent: &VerifiedTargetParent,
    target_name: &OsStr,
) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "atomic no-replace rename is unsupported on this platform: {} -> {}",
            source.path.display(),
            target_parent.path.join(target_name).display()
        ),
    ))
}

#[cfg(windows)]
fn delete_permanent(source: VerifiedSource) -> io::Result<()> {
    if source.is_directory {
        for entry in fs::read_dir(&source.path)? {
            let child = VerifiedSource::open(&entry?.path())?;
            child.delete_permanent()?;
        }
    }
    set_delete_disposition(&source.handle)
}

#[cfg(windows)]
fn set_delete_disposition(handle: &fs::File) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, FileDispositionInfoEx, SetFileInformationByHandle,
        FILE_DISPOSITION_FLAG_DELETE, FILE_DISPOSITION_FLAG_POSIX_SEMANTICS, FILE_DISPOSITION_INFO,
        FILE_DISPOSITION_INFO_EX,
    };

    let extended = FILE_DISPOSITION_INFO_EX {
        Flags: FILE_DISPOSITION_FLAG_DELETE | FILE_DISPOSITION_FLAG_POSIX_SEMANTICS,
    };
    let succeeded = unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfoEx,
            std::ptr::addr_of!(extended).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO_EX>() as u32,
        )
    } != 0;
    if succeeded {
        return Ok(());
    }

    let basic = FILE_DISPOSITION_INFO { DeleteFile: true };
    if unsafe {
        SetFileInformationByHandle(
            handle.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::addr_of!(basic).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    } == 0
    {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn delete_permanent(source: VerifiedSource) -> io::Result<()> {
    if source.is_directory {
        fs::remove_dir_all(source.path)
    } else {
        fs::remove_file(source.path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "megle-platform-{label}-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ))
    }

    #[test]
    fn native_rename_no_replace_preserves_existing_target() {
        let dir = temp_dir("no-replace");
        fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("source.txt");
        let target = dir.join("target.txt");
        fs::write(&source, b"source").expect("write source");
        fs::write(&target, b"target").expect("write target");
        let verified = VerifiedSource::open(&source).expect("open source");
        let target_parent = VerifiedTargetParent::open(&dir).expect("open target parent");

        let error = verified
            .rename_no_replace(&target_parent, target.file_name().expect("target leaf"))
            .expect_err("existing target must not be replaced");

        assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
        assert_eq!(fs::read(&source).expect("source remains"), b"source");
        assert_eq!(fs::read(&target).expect("target remains"), b"target");
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn verified_handle_blocks_path_replacement_until_handle_rename() {
        let dir = temp_dir("protected-rename");
        fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("source.txt");
        let displaced = dir.join("displaced.txt");
        let target = dir.join("target.txt");
        fs::write(&source, b"original").expect("write source");
        let verified = VerifiedSource::open(&source).expect("open protected source");
        let target_parent = VerifiedTargetParent::open(&dir).expect("open target parent");

        let replacement_attempt = std::thread::spawn({
            let source = source.clone();
            let displaced = displaced.clone();
            move || fs::rename(source, displaced)
        })
        .join()
        .expect("replacement thread");
        assert!(
            replacement_attempt.is_err(),
            "protected source was replaceable"
        );

        verified
            .rename_no_replace(&target_parent, target.file_name().expect("target leaf"))
            .expect("handle-bound rename");
        assert_eq!(fs::read(&target).expect("renamed object"), b"original");
        assert!(!displaced.exists());
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn verified_target_parent_blocks_replacement_and_binds_relative_rename() {
        let dir = temp_dir("protected-target-parent");
        let outside = temp_dir("protected-target-parent-outside");
        let source_dir = dir.join("source");
        let target_dir = dir.join("target");
        let displaced_dir = outside.join("displaced-target");
        fs::create_dir_all(&source_dir).expect("create source dir");
        fs::create_dir_all(&target_dir).expect("create target dir");
        fs::create_dir_all(&outside).expect("create outside dir");
        let source = source_dir.join("source.txt");
        fs::write(&source, b"original").expect("write source");
        let verified_source = VerifiedSource::open(&source).expect("open source");
        let verified_parent = VerifiedTargetParent::open(&target_dir).expect("open target parent");

        let replacement_attempt = std::thread::spawn({
            let target_dir = target_dir.clone();
            let displaced_dir = displaced_dir.clone();
            move || fs::rename(target_dir, displaced_dir)
        })
        .join()
        .expect("replacement thread");
        assert!(
            replacement_attempt.is_err(),
            "protected target parent was replaceable"
        );

        verified_source
            .rename_no_replace(&verified_parent, std::ffi::OsStr::new("moved.txt"))
            .expect("parent-bound rename");
        assert_eq!(
            fs::read(target_dir.join("moved.txt")).expect("moved object"),
            b"original"
        );
        assert!(!displaced_dir.exists());
        let _ = fs::remove_dir_all(dir);
        let _ = fs::remove_dir_all(outside);
    }

    #[cfg(windows)]
    #[test]
    fn target_parent_reparse_point_is_refused() {
        use std::os::windows::fs::symlink_dir;

        let dir = temp_dir("target-parent-reparse");
        let real = dir.join("real");
        let link = dir.join("link");
        fs::create_dir_all(&real).expect("create real target");
        if symlink_dir(&real, &link).is_err() {
            let _ = fs::remove_dir_all(dir);
            return;
        }

        let error = VerifiedTargetParent::open(&link)
            .expect_err("target parent reparse point must be rejected");
        assert_eq!(error.kind(), io::ErrorKind::PermissionDenied);
        let _ = fs::remove_dir_all(dir);
    }

    #[cfg(windows)]
    #[test]
    fn recycle_staging_cannot_delete_same_path_replacement() {
        let dir = temp_dir("recycle-staging");
        fs::create_dir_all(&dir).expect("create temp dir");
        let source = dir.join("source.txt");
        let displaced = dir.join("displaced.txt");
        let staging = dir.join(".megle-recycle-stage");
        fs::write(&source, b"original").expect("write source");
        let verified = VerifiedSource::open(&source).expect("open protected source");
        let target_parent = VerifiedTargetParent::open(&dir).expect("open target parent");

        let replacement_attempt = std::thread::spawn({
            let source = source.clone();
            let displaced = displaced.clone();
            move || fs::rename(source, displaced)
        })
        .join()
        .expect("replacement thread");
        assert!(
            replacement_attempt.is_err(),
            "protected source was replaceable"
        );

        verified
            .rename_no_replace(&target_parent, staging.file_name().expect("staging leaf"))
            .expect("stage original by handle");
        fs::write(&source, b"replacement").expect("write replacement");
        trash::delete(&staging)
            .expect_err("Shell cannot delete while the identity handle denies DELETE sharing");
        let recycle_guard = verified
            .recycle_identity_guard()
            .expect("reopen same-object recycle guard");
        let identity = verified.identity().to_owned();
        drop(verified);
        trash::delete(&staging).expect("recycle staged original");
        recycle_guard
            .verify_shell_handoff(&identity)
            .expect("guarded object reached recycle bin");
        assert_eq!(
            fs::read(&source).expect("replacement remains"),
            b"replacement"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
