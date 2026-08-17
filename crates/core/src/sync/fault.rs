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
