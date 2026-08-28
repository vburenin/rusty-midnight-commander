//! Shared directory-listing cache for remote, archive, and extfs VFS backends.
//!
//! GNU mc’s Options → Virtual FS “Directory cache timeout” (seconds):
//! - `0` — do not cache (always re-list)
//! - `N > 0` — reuse a listing until `N` seconds have elapsed, then refresh
//!
//! Local disk listings are not stored here; [`crate::composite::CompositeFs`]
//! consults this cache only for non-local routes.

use crate::{DirEntry, FsResult};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default timeout matching GNU mc (`900` seconds).
pub const DEFAULT_DIR_CACHE_TIMEOUT_SECS: u32 = 900;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CacheKey {
    path: PathBuf,
    show_hidden: bool,
}

#[derive(Debug, Clone)]
struct CacheEntry {
    listed_at: Instant,
    entries: Vec<DirEntry>,
}

/// In-memory listing cache keyed by directory path and hidden-file flag.
#[derive(Debug, Default)]
pub struct DirListingCache {
    map: HashMap<CacheKey, CacheEntry>,
}

impl DirListingCache {
    pub fn new() -> Self {
        Self {
            map: HashMap::new(),
        }
    }

    fn key(path: &Path, show_hidden: bool) -> CacheKey {
        CacheKey {
            path: path.to_path_buf(),
            show_hidden,
        }
    }

    /// Return a cached listing when `timeout_secs > 0` and the entry is younger
    /// than `timeout_secs`. Timeout `0` never hits the cache.
    pub fn lookup(
        &self,
        path: &Path,
        show_hidden: bool,
        timeout_secs: u32,
        now: Instant,
    ) -> Option<Vec<DirEntry>> {
        if timeout_secs == 0 {
            return None;
        }
        let entry = self.map.get(&Self::key(path, show_hidden))?;
        let age = now
            .checked_duration_since(entry.listed_at)
            .unwrap_or(Duration::ZERO);
        if age >= Duration::from_secs(u64::from(timeout_secs)) {
            return None;
        }
        Some(entry.entries.clone())
    }

    /// Store a listing. Callers should skip this when `timeout_secs == 0`.
    pub fn store(&mut self, path: &Path, show_hidden: bool, entries: Vec<DirEntry>, now: Instant) {
        self.map.insert(
            Self::key(path, show_hidden),
            CacheEntry {
                listed_at: now,
                entries,
            },
        );
    }

    /// Drop cached listings for `path` (both hidden and non-hidden variants).
    pub fn invalidate_path(&mut self, path: &Path) {
        self.map.retain(|k, _| k.path != path);
    }

    pub fn clear(&mut self) {
        self.map.clear();
    }

    /// Lookup, or `fetch` and optionally store. Timeout `0` never reads or writes.
    pub fn get_or_fetch<F>(
        &mut self,
        path: &Path,
        show_hidden: bool,
        timeout_secs: u32,
        now: Instant,
        fetch: F,
    ) -> FsResult<Vec<DirEntry>>
    where
        F: FnOnce() -> FsResult<Vec<DirEntry>>,
    {
        if let Some(hit) = self.lookup(path, show_hidden, timeout_secs, now) {
            return Ok(hit);
        }
        let entries = fetch()?;
        if timeout_secs > 0 {
            self.store(path, show_hidden, entries.clone(), now);
        }
        Ok(entries)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Metadata;
    use std::cell::Cell;
    use std::rc::Rc;
    use std::time::SystemTime;

    fn entry(name: &str) -> DirEntry {
        DirEntry {
            name: name.to_string(),
            path: PathBuf::from(name),
            meta: Metadata {
                is_dir: false,
                is_symlink: false,
                symlink_target: None,
                is_executable: false,
                size: 0,
                modified: SystemTime::UNIX_EPOCH,
                permissions: 0,
                owner: None,
                group: None,
                nlink: 1,
                accessed: SystemTime::UNIX_EPOCH,
                changed: SystemTime::UNIX_EPOCH,
                inode: 0,
            },
        }
    }

    fn names(list: &[DirEntry]) -> Vec<&str> {
        list.iter().map(|e| e.name.as_str()).collect()
    }

    #[test]
    fn timeout_zero_never_hits_cache() {
        let mut cache = DirListingCache::new();
        let path = Path::new("/ftp://host/dir");
        let now = Instant::now();
        let fetches = Rc::new(Cell::new(0u32));
        let fetches_c = fetches.clone();
        let fetch = || {
            fetches_c.set(fetches_c.get() + 1);
            Ok(vec![entry("a")])
        };

        let first = cache.get_or_fetch(path, false, 0, now, fetch).unwrap();
        assert_eq!(names(&first), ["a"]);
        assert_eq!(fetches.get(), 1);

        let fetches_c = fetches.clone();
        let second = cache
            .get_or_fetch(path, false, 0, now, || {
                fetches_c.set(fetches_c.get() + 1);
                Ok(vec![entry("b")])
            })
            .unwrap();
        assert_eq!(
            names(&second),
            ["b"],
            "timeout 0 must re-list even with a stored entry"
        );
        assert_eq!(fetches.get(), 2);

        // Direct lookup also misses.
        cache.store(path, false, vec![entry("cached")], now);
        assert!(cache.lookup(path, false, 0, now).is_none());
    }

    #[test]
    fn timeout_n_reuses_within_n_and_refreshes_after() {
        let mut cache = DirListingCache::new();
        let path = Path::new("sftp://host/pub");
        let t0 = Instant::now();
        let fetches = Rc::new(Cell::new(0u32));

        let fetches_c = fetches.clone();
        let first = cache
            .get_or_fetch(path, true, 5, t0, || {
                fetches_c.set(fetches_c.get() + 1);
                Ok(vec![entry("one")])
            })
            .unwrap();
        assert_eq!(names(&first), ["one"]);
        assert_eq!(fetches.get(), 1);

        let fetches_c = fetches.clone();
        let within = cache
            .get_or_fetch(path, true, 5, t0 + Duration::from_secs(4), || {
                fetches_c.set(fetches_c.get() + 1);
                Ok(vec![entry("stale-fetch")])
            })
            .unwrap();
        assert_eq!(names(&within), ["one"]);
        assert_eq!(fetches.get(), 1);

        let fetches_c = fetches.clone();
        let after = cache
            .get_or_fetch(path, true, 5, t0 + Duration::from_secs(5), || {
                fetches_c.set(fetches_c.get() + 1);
                Ok(vec![entry("two")])
            })
            .unwrap();
        assert_eq!(names(&after), ["two"]);
        assert_eq!(fetches.get(), 2);
    }

    #[test]
    fn refresh_invalidate_bypasses_fresh_cache() {
        let mut cache = DirListingCache::new();
        let path = Path::new("ftp://host/dir");
        let now = Instant::now();
        cache.store(path, false, vec![entry("old")], now);
        assert_eq!(
            names(&cache.lookup(path, false, 900, now).unwrap()),
            ["old"]
        );

        cache.invalidate_path(path);
        assert!(
            cache.lookup(path, false, 900, now).is_none(),
            "Refresh must drop the cached listing even when still within TTL"
        );

        let fetches = Cell::new(0u32);
        let listed = cache
            .get_or_fetch(path, false, 900, now, || {
                fetches.set(fetches.get() + 1);
                Ok(vec![entry("new")])
            })
            .unwrap();
        assert_eq!(names(&listed), ["new"]);
        assert_eq!(fetches.get(), 1);
    }

    #[test]
    fn hidden_flag_is_part_of_key() {
        let mut cache = DirListingCache::new();
        let path = Path::new("zip#/");
        let now = Instant::now();
        cache.store(path, false, vec![entry("visible")], now);
        cache.store(path, true, vec![entry("visible"), entry(".secret")], now);
        assert_eq!(
            names(&cache.lookup(path, false, 30, now).unwrap()),
            ["visible"]
        );
        assert_eq!(names(&cache.lookup(path, true, 30, now).unwrap()).len(), 2);
        cache.invalidate_path(path);
        assert!(cache.lookup(path, false, 30, now).is_none());
        assert!(cache.lookup(path, true, 30, now).is_none());
    }
}
