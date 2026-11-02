//! One master debug switch for the whole Core.
//!
//! Diagnostic instrumentation (writer-actor tracing, per-step timing of the
//! thumbnail-cache enqueue, startup self-tests) stays permanently in the code
//! but is silent unless the operator turns it on. Flip it on with the
//! `MANGOO_DEBUG` environment variable (`MANGOO_DEBUG=1`); leave it unset in
//! production. This is deliberately a single global gate — every diagnostic in
//! Core routes through [`enabled()`] / [`debug_log!`] so there is exactly one
//! knob to reason about, rather than scattered ad-hoc `eprintln!`s that get
//! added and stripped each debugging session.

use std::sync::OnceLock;

/// Whether verbose Core debug instrumentation is enabled. Read once from the
/// `MANGOO_DEBUG` env var on first use and cached, so hot paths pay only an
/// atomic load. Any non-empty value other than `0`/`false` counts as enabled.
pub fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| {
        std::env::var("MANGOO_DEBUG")
            .ok()
            .map(|value| {
                let value = value.trim();
                !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
            })
            .unwrap_or(false)
    })
}

/// Emit a debug line to stderr, but only when the master switch is on. Prefixed
/// with `[mangoo-debug]` so it is greppable and obviously not a user-facing log.
/// Arguments are only evaluated when debug is enabled.
#[macro_export]
macro_rules! debug_log {
    ($($arg:tt)*) => {
        if $crate::debug::enabled() {
            eprintln!("[mangoo-debug] {}", format!($($arg)*));
        }
    };
}

/// Time a block and, when debug is enabled, log how long it took under `label`.
/// Returns the block's value. Zero cost when debug is off apart from the
/// (already-cached) `enabled()` check.
pub fn timed<T>(label: &str, op: impl FnOnce() -> T) -> T {
    if !enabled() {
        return op();
    }
    let start = std::time::Instant::now();
    let value = op();
    eprintln!("[mangoo-debug] {label}: {}ms", start.elapsed().as_millis());
    value
}
