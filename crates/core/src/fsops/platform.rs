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

impl VerifiedSource {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        open_verified_source(path)
    }

    pub(super) fn identity(&self) -> &str {
        &self.identity
    }

    pub(super) fn rename_no_replace(self, target: &Path) -> io::Result<()> {
        rename_no_replace(self, target)
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

#[cfg(windows)]
fn rename_no_replace(source: VerifiedSource, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileRenameInfo, SetFileInformationByHandle, FILE_RENAME_INFO,
    };

    let target_wide: Vec<u16> = target.as_os_str().encode_wide().collect();
    let header_len = std::mem::offset_of!(FILE_RENAME_INFO, FileName);
    let file_name_byte_len = target_wide.len().saturating_mul(std::mem::size_of::<u16>());
    let byte_len = header_len
        .checked_add(file_name_byte_len)
        .and_then(|length| length.checked_add(std::mem::size_of::<u16>()))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long"))?;
    let word_len = byte_len.div_ceil(std::mem::size_of::<u64>());
    let mut storage = vec![0u64; word_len];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = false;
        (*info).RootDirectory = std::ptr::null_mut();
        (*info).FileNameLength = u32::try_from(file_name_byte_len).map_err(|_| {
            io::Error::new(io::ErrorKind::InvalidInput, "rename target is too long")
        })?;
        std::ptr::copy_nonoverlapping(
            target_wide.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            target_wide.len(),
        );
        if SetFileInformationByHandle(
            source.handle.as_raw_handle(),
            FileRenameInfo,
            info.cast(),
            u32::try_from(byte_len).map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidInput, "rename buffer is too large")
            })?,
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "android"))]
fn rename_no_replace(source: VerifiedSource, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source_path = CString::new(source.path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
    let target_path = CString::new(target.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "target path contains NUL"))?;
    let result = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            source_path.as_ptr(),
            libc::AT_FDCWD,
            target_path.as_ptr(),
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
fn rename_no_replace(source: VerifiedSource, target: &Path) -> io::Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    let source_path = CString::new(source.path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "source path contains NUL"))?;
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
fn rename_no_replace(source: VerifiedSource, target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "atomic no-replace rename is unsupported on this platform: {} -> {}",
            source.path.display(),
            target.display()
        ),
    ))
}

#[cfg(not(any(windows, unix)))]
fn rename_no_replace(source: VerifiedSource, target: &Path) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        format!(
            "atomic no-replace rename is unsupported on this platform: {} -> {}",
            source.path.display(),
            target.display()
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

        let error = verified
            .rename_no_replace(&target)
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
            .rename_no_replace(&target)
            .expect("handle-bound rename");
        assert_eq!(fs::read(&target).expect("renamed object"), b"original");
        assert!(!displaced.exists());
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
            .rename_no_replace(&staging)
            .expect("stage original by handle");
        fs::write(&source, b"replacement").expect("write replacement");
        trash::delete(&staging).expect("recycle staged original");
        assert_eq!(
            fs::read(&source).expect("replacement remains"),
            b"replacement"
        );
        let _ = fs::remove_dir_all(dir);
    }
}
