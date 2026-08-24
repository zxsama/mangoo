/// Deterministic subprocess crash rendezvous used by the durability matrix.
/// It is compiled only into the Rust test binary; production builds erase the
/// calls completely. The marker is fsynced before the child parks, allowing
/// the parent to terminate the process at an exact business boundary.
#[inline]
pub fn hit(name: &str) {
    #[cfg(test)]
    {
        if std::env::var("MEGLE_SYNC_FAULTPOINT").ok().as_deref() != Some(name) {
            return;
        }
        let Some(marker) = std::env::var_os("MEGLE_SYNC_FAULT_MARKER") else {
            return;
        };
        use std::io::Write;
        let mut file = std::fs::File::create(marker).expect("create sync fault marker");
        file.write_all(name.as_bytes())
            .expect("write sync fault marker");
        file.sync_all().expect("fsync sync fault marker");
        loop {
            std::thread::park_timeout(std::time::Duration::from_secs(1));
        }
    }
    #[cfg(not(test))]
    let _ = name;
}

/// Byte-threshold variant used by the physical media kill/resume probe. The
/// rendezvous is reached only after the corresponding durable cursor has been
/// committed, so a restarted worker can prove it does not retransmit bytes.
#[inline]
pub fn add_and_hit_after_bytes(name: &str, durable_delta: u64) {
    #[cfg(test)]
    {
        let threshold = std::env::var("MEGLE_SYNC_FAULT_AFTER_BYTES")
            .ok()
            .and_then(|value| value.parse::<u64>().ok());
        let durable_bytes = match std::env::var_os("MEGLE_SYNC_FAULT_COUNTER") {
            Some(path) => {
                let path = std::path::PathBuf::from(path);
                let previous = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|value| value.parse::<u64>().ok())
                    .unwrap_or(0);
                let next = previous.saturating_add(durable_delta);
                let temporary = path.with_extension("tmp");
                std::fs::write(&temporary, next.to_string()).expect("write sync fault counter");
                std::fs::rename(&temporary, &path).expect("publish sync fault counter");
                next
            }
            None => durable_delta,
        };
        if threshold.is_some_and(|threshold| durable_bytes >= threshold) {
            hit(name);
        }
    }
    #[cfg(not(test))]
    let _ = (name, durable_delta);
}
