//! Dependency-free child-process supervision shared by every subprocess-backed
//! decoder. Kept out of any one decoder so the "never leak a zombie, never
//! block forever" discipline has exactly one implementation.

use std::process::Child;
use std::time::{Duration, Instant};

/// Polling cadence for [`wait_with_timeout`]. Deliberately dependency-free so
/// the worker doesn't pull in tokio/wait_timeout just to bound a subprocess.
const CHILD_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Outcome of [`wait_with_timeout`]. The exit-status case is mapped onto
/// `std::process::Output` by callers that piped streams.
pub enum ChildWaitOutcome {
    Exited(std::process::ExitStatus),
    TimedOut,
}

/// Polls `child.try_wait()` until it exits or `timeout` elapses. On timeout the
/// child is killed and reaped so we don't leak a zombie. Stdout/stderr are
/// *not* drained here — callers that piped them must read before the child
/// exits to avoid deadlocking on a full pipe.
pub fn wait_with_timeout(
    child: &mut Child,
    timeout: Duration,
) -> std::io::Result<ChildWaitOutcome> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(ChildWaitOutcome::Exited(status));
        }
        if Instant::now() >= deadline {
            // Best-effort kill + reap. If the child already exited between the
            // try_wait above and the kill, both calls succeed cleanly.
            let _ = child.kill();
            let _ = child.wait();
            return Ok(ChildWaitOutcome::TimedOut);
        }
        std::thread::sleep(CHILD_WAIT_POLL_INTERVAL);
    }
}
