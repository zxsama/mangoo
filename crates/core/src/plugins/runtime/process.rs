use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::{Child, Command};
use tokio::process::{ChildStderr, ChildStdin, ChildStdout};

use super::trust::VerifiedExecutable;

pub struct LaunchSpec {
    pub runtime_dir: PathBuf,
    pub safe_env: HashMap<String, String>,
}
pub struct ChildProcess {
    child: Child,
    #[cfg(windows)]
    job: WindowsJob,
}
impl ChildProcess {
    pub fn spawn_verified(
        executable: &VerifiedExecutable,
        spec: &LaunchSpec,
    ) -> std::io::Result<Self> {
        std::fs::create_dir_all(&spec.runtime_dir)?;
        #[cfg(unix)]
        let mut cmd = unix_verified_command(executable, &spec.safe_env)?;
        #[cfg(not(unix))]
        let mut cmd = Command::new(executable.path());
        cmd.current_dir(&spec.runtime_dir)
            .env_clear()
            .envs(platform_runtime_env())
            .envs(&spec.safe_env)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use windows_sys::Win32::System::Threading::CREATE_SUSPENDED;
            cmd.as_std_mut().creation_flags(CREATE_SUSPENDED);
        }
        let mut child = cmd.spawn()?;
        #[cfg(windows)]
        let job = match WindowsJob::attach_verified_and_resume(&child, executable) {
            Ok(job) => job,
            Err(error) => {
                let _ = child.start_kill();
                return Err(error);
            }
        };
        Ok(Self {
            child,
            #[cfg(windows)]
            job,
        })
    }
    pub fn id(&self) -> Option<u32> {
        self.child.id()
    }
    pub fn take_stdio(&mut self) -> std::io::Result<(ChildStdin, ChildStdout, ChildStderr)> {
        Ok((
            self.child
                .stdin
                .take()
                .ok_or_else(|| std::io::Error::other("missing child stdin"))?,
            self.child
                .stdout
                .take()
                .ok_or_else(|| std::io::Error::other("missing child stdout"))?,
            self.child
                .stderr
                .take()
                .ok_or_else(|| std::io::Error::other("missing child stderr"))?,
        ))
    }
    pub async fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
        self.child.wait().await
    }
    pub fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }
    pub async fn terminate_tree(&mut self) -> std::io::Result<()> {
        #[cfg(unix)]
        if let Some(pid) = self.child.id() {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
        }
        self.child.kill().await.or_else(|e| {
            if e.kind() == std::io::ErrorKind::InvalidInput {
                Ok(())
            } else {
                Err(e)
            }
        })
    }
}

fn platform_runtime_env() -> std::collections::HashMap<String, String> {
    #[cfg(windows)]
    {
        ["SystemRoot", "WINDIR"]
            .into_iter()
            .filter_map(|key| std::env::var(key).ok().map(|value| (key.to_owned(), value)))
            .collect()
    }
    #[cfg(not(windows))]
    {
        Default::default()
    }
}

#[cfg(unix)]
fn unix_verified_command(
    executable: &VerifiedExecutable,
    safe_env: &HashMap<String, String>,
) -> std::io::Result<Command> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;

    fn cstring(value: &str, kind: &str) -> std::io::Result<CString> {
        CString::new(value).map_err(|_| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("{kind} contains a NUL byte"),
            )
        })
    }

    let argv0 = cstring(&executable.path().to_string_lossy(), "executable path")?;
    let environment = safe_env
        .iter()
        .map(|(key, value)| cstring(&format!("{key}={value}"), "environment entry"))
        .collect::<std::io::Result<Vec<_>>>()?;
    let verified_fd = executable.file().as_raw_fd();
    let mut command = Command::new("/bin/false");
    unsafe {
        command.pre_exec(move || {
            if libc::setpgid(0, 0) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            let argv = [argv0.as_ptr(), std::ptr::null()];
            let mut envp = environment
                .iter()
                .map(|entry| entry.as_ptr())
                .collect::<Vec<_>>();
            envp.push(std::ptr::null());
            libc::fexecve(verified_fd, argv.as_ptr(), envp.as_ptr());
            Err(std::io::Error::last_os_error())
        });
    }
    Ok(command)
}

#[cfg(windows)]
struct WindowsJob(windows_sys::Win32::Foundation::HANDLE);
// SAFETY: `WindowsJob` exclusively owns the Job Object handle. Windows kernel
// handles may be used and closed from a different thread, and the type exposes
// no borrowed thread-affine state.
#[cfg(windows)]
unsafe impl Send for WindowsJob {}
#[cfg(windows)]
impl WindowsJob {
    fn attach_verified_and_resume(
        child: &Child,
        executable: &VerifiedExecutable,
    ) -> std::io::Result<Self> {
        use windows_sys::Win32::System::JobObjects::*;
        use windows_sys::Win32::System::Threading::{
            OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
            PROCESS_SET_QUOTA, PROCESS_SUSPEND_RESUME, PROCESS_TERMINATE,
        };
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err(std::io::Error::last_os_error());
            }
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let process = OpenProcess(
                PROCESS_QUERY_LIMITED_INFORMATION
                    | PROCESS_SET_QUOTA
                    | PROCESS_SUSPEND_RESUME
                    | PROCESS_TERMINATE,
                0,
                child
                    .id()
                    .ok_or_else(|| std::io::Error::other("child has no process id"))?,
            );
            if process.is_null() {
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(std::io::Error::last_os_error());
            }

            let mut image = vec![0_u16; 32_768];
            let mut image_len = image.len() as u32;
            if QueryFullProcessImageNameW(process, 0, image.as_mut_ptr(), &mut image_len) == 0 {
                windows_sys::Win32::Foundation::CloseHandle(process);
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(std::io::Error::last_os_error());
            }
            image.truncate(image_len as usize);
            let loaded_path = PathBuf::from(String::from_utf16(&image).map_err(|_| {
                std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid loaded image path")
            })?);
            if !executable.matches_path(&loaded_path).map_err(|error| {
                std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("loaded image verification failed: {error:?}"),
                )
            })? {
                windows_sys::Win32::Foundation::CloseHandle(process);
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "loaded image identity or hash differs from verified executable",
                ));
            }

            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &info as *const _ as _,
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(job, process) == 0
            {
                windows_sys::Win32::Foundation::CloseHandle(process);
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(std::io::Error::last_os_error());
            }
            #[link(name = "ntdll")]
            unsafe extern "system" {
                fn NtResumeProcess(process: windows_sys::Win32::Foundation::HANDLE) -> i32;
            }
            if NtResumeProcess(process) < 0 {
                windows_sys::Win32::Foundation::CloseHandle(process);
                windows_sys::Win32::Foundation::CloseHandle(job);
                return Err(std::io::Error::last_os_error());
            }
            windows_sys::Win32::Foundation::CloseHandle(process);
            Ok(Self(job))
        }
    }
}
#[cfg(windows)]
impl Drop for WindowsJob {
    fn drop(&mut self) {
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.0);
        }
    }
}
pub fn is_bundled_root(path: &Path) -> bool {
    #[cfg(windows)]
    {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|p| p.join("bundled-plugins")))
            .is_some_and(|expected| expected == path)
    }
    #[cfg(unix)]
    {
        path == Path::new("/opt/megle/plugins")
            || std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|p| p.join("bundled-plugins")))
                .is_some_and(|expected| expected == path)
    }
    #[cfg(not(any(windows, unix)))]
    {
        let _ = path;
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "megle_{name}_{}_{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ))
    }

    #[test]
    fn arbitrary_resources_directory_is_not_a_bundled_root() {
        let root = test_root("untrusted_resources").join("resources");
        assert!(!is_bundled_root(&root));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn swap_at_spawn_executes_verified_descriptor_not_replacement() {
        let root = test_root("verified_spawn");
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let executable = root.join("provider");
        std::fs::copy("/bin/true", &executable).unwrap();
        let verified = VerifiedExecutable::for_test(&executable).unwrap();
        std::fs::rename(&executable, root.join("provider.verified")).unwrap();
        std::fs::copy("/bin/false", &executable).unwrap();

        let mut child = ChildProcess::spawn_verified(
            &verified,
            &LaunchSpec {
                runtime_dir: runtime,
                safe_env: HashMap::new(),
            },
        )
        .unwrap();
        assert!(child.wait().await.unwrap().success());
        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(windows)]
    #[test]
    fn swap_at_spawn_is_rejected_before_resume() {
        let root = test_root("verified_spawn");
        let runtime = root.join("runtime");
        std::fs::create_dir_all(&runtime).unwrap();
        let system = PathBuf::from(std::env::var_os("SystemRoot").unwrap()).join("System32");
        let executable = root.join("provider.exe");
        std::fs::copy(system.join("cmd.exe"), &executable).unwrap();
        let verified = VerifiedExecutable::for_test(&executable).unwrap();
        std::fs::rename(&executable, root.join("provider.verified.exe")).unwrap();
        std::fs::copy(system.join("where.exe"), &executable).unwrap();

        let result = ChildProcess::spawn_verified(
            &verified,
            &LaunchSpec {
                runtime_dir: runtime,
                safe_env: HashMap::new(),
            },
        );
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }
}
