use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Per-root reachability cache: root_id -> last-probed reachable state.
pub type RootReachability = Arc<Mutex<HashMap<i64, bool>>>;

/// True iff the root's own directory can be stat'd as a directory right now.
/// One syscall stands in for the whole share's online status: unreachable root
/// path == NAS offline. Any error (network down, permission, timeout) => false.
pub fn probe_root_reachable(path: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    std::fs::metadata(path).map(|m| m.is_dir()).unwrap_or(false)
}

/// Probes every root's path and replaces the cache wholesale with the fresh results.
pub fn refresh_root_reachability(reach: &RootReachability, roots: &[(i64, String)]) {
    let mut next = HashMap::with_capacity(roots.len());
    for (id, path) in roots {
        next.insert(*id, probe_root_reachable(path));
    }
    if let Ok(mut guard) = reach.lock() {
        *guard = next;
    }
}

#[cfg(test)]
mod tests {
    use super::{probe_root_reachable, refresh_root_reachability, RootReachability};
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    #[test]
    fn probe_root_reachable_true_for_existing_dir_false_for_missing() {
        let dir = std::env::temp_dir();
        assert!(probe_root_reachable(dir.to_str().unwrap()));
        let missing = std::env::temp_dir().join("mangoo-reachability-nonexistent-zzz");
        assert!(!probe_root_reachable(missing.to_str().unwrap()));
        assert!(!probe_root_reachable(""));
    }

    #[test]
    fn refresh_root_reachability_replaces_map_with_fresh_probe_results() {
        let dir = std::env::temp_dir();
        let existing_path = dir.to_str().unwrap().to_string();
        let missing_path = std::env::temp_dir()
            .join("mangoo-reachability-nonexistent-zzz")
            .to_str()
            .unwrap()
            .to_string();

        let reach: RootReachability = Arc::new(Mutex::new(HashMap::new()));
        // Seed with a stale value for a root id that will be probed again, to
        // confirm refresh replaces (not merely inserts into) the map.
        reach.lock().unwrap().insert(1, false);

        let roots = vec![(1, existing_path), (2, missing_path)];
        refresh_root_reachability(&reach, &roots);

        let guard = reach.lock().unwrap();
        assert_eq!(guard.len(), 2);
        assert_eq!(guard.get(&1).copied(), Some(true));
        assert_eq!(guard.get(&2).copied(), Some(false));
    }
}
