// Interactive-write coordination between the API's latency-sensitive writers
// (thumbnail priority-scope sync, on-demand thumbnail-state promotion) and the
// background thumbnail workers that stream generated WebP blobs into SQLite.
//
// All writers share one WAL writer lock. Background blob commits are numerous
// during a "generate all", so an interactive write issuing `BEGIN IMMEDIATE`
// can wait behind a burst of them (measured begin-lock waits of 150-400ms on a
// million-file library — the scroll stutter). SQLite has no priority for the
// write lock, so we coordinate in the application: an interactive writer marks
// its intent, and a background blob worker briefly yields the writer before
// taking its own `BEGIN IMMEDIATE` while any interactive write is pending.
//
// This is advisory and bounded — it never blocks a background write
// indefinitely (a hard cap on the yield), and it does not touch selected/
// foreground thumbnails (those are also user-awaited).

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::LazyLock;
use std::time::{Duration, Instant};

/// Number of times a background blob worker polls the pending-interactive count
/// before giving up and taking the write lock anyway. With the sleep below this
/// bounds the yield to ~SPIN * SLEEP, so a background write is never starved.
const YIELD_MAX_SPINS: u32 = 120;
/// Sleep between polls while yielding to a pending interactive write.
const YIELD_SLEEP: Duration = Duration::from_micros(250);
/// Absolute ceiling on how long a single background write will yield, so a
/// stuck/never-committing interactive writer cannot stall ingestion. Background
/// thumbnail generation is not latency-critical, so this can be generous — it
/// only delays blob ingestion, never blocks it.
const YIELD_MAX_WAIT: Duration = Duration::from_millis(30);

/// Process-global gate. The Core runs a single logical database, so a global is
/// simpler than threading an `Arc` through `start_worker` and every API state.
static INTERACTIVE_WRITE_GATE: LazyLock<InteractiveWriteGate> =
    LazyLock::new(InteractiveWriteGate::new);

/// The shared production gate used by the API interactive-write paths and the
/// background thumbnail workers.
pub fn interactive_write_gate() -> &'static InteractiveWriteGate {
    &INTERACTIVE_WRITE_GATE
}

#[derive(Default)]
pub struct InteractiveWriteGate {
    pending: AtomicUsize,
}

impl InteractiveWriteGate {
    fn new() -> Self {
        Self::default()
    }

    /// Mark that an interactive (latency-sensitive) write is about to run. The
    /// returned guard clears the mark on drop. Wrap the API-side
    /// `run_shared_database` call for priority-scope / thumbnail-state promotion.
    pub fn begin_interactive_write(&self) -> InteractiveWriteGuard<'_> {
        self.pending.fetch_add(1, Ordering::SeqCst);
        InteractiveWriteGuard { gate: self }
    }

    fn interactive_write_pending(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

    /// Called by a background/bulk blob worker just before it takes the WAL
    /// write lock. If an interactive write is pending, yield the writer (bounded)
    /// so the interactive write acquires `BEGIN IMMEDIATE` first.
    pub fn yield_to_pending_interactive_writes(&self) {
        if !self.interactive_write_pending() {
            return;
        }
        let deadline = Instant::now() + YIELD_MAX_WAIT;
        let mut spins = 0;
        while self.interactive_write_pending() && spins < YIELD_MAX_SPINS {
            if Instant::now() >= deadline {
                break;
            }
            std::thread::sleep(YIELD_SLEEP);
            spins += 1;
        }
    }
}

pub struct InteractiveWriteGuard<'a> {
    gate: &'a InteractiveWriteGate,
}

impl Drop for InteractiveWriteGuard<'_> {
    fn drop(&mut self) {
        self.gate.pending.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Tests construct their own local gate so concurrent test execution cannot
    // race on the shared process-global counter.

    #[test]
    fn no_pending_writes_means_no_yield() {
        let gate = InteractiveWriteGate::new();
        let start = Instant::now();
        gate.yield_to_pending_interactive_writes();
        assert!(start.elapsed() < Duration::from_millis(5));
    }

    #[test]
    fn guard_marks_and_clears_pending() {
        let gate = InteractiveWriteGate::new();
        assert!(!gate.interactive_write_pending());
        {
            let _guard = gate.begin_interactive_write();
            assert!(gate.interactive_write_pending());
        }
        assert!(!gate.interactive_write_pending());
    }

    #[test]
    fn nested_guards_keep_pending_until_all_drop() {
        let gate = InteractiveWriteGate::new();
        let a = gate.begin_interactive_write();
        let b = gate.begin_interactive_write();
        assert!(gate.interactive_write_pending());
        drop(a);
        assert!(gate.interactive_write_pending());
        drop(b);
        assert!(!gate.interactive_write_pending());
    }

    #[test]
    fn yield_is_bounded_when_interactive_write_stays_pending() {
        let gate = InteractiveWriteGate::new();
        let _guard = gate.begin_interactive_write();
        // The interactive write never clears during this call, so the yield must
        // still return within the hard ceiling rather than spin forever.
        let start = Instant::now();
        gate.yield_to_pending_interactive_writes();
        assert!(start.elapsed() < YIELD_MAX_WAIT + Duration::from_millis(20));
    }
}
