use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use super::pool::{Clip, Issue, IssueKind, PathRefused, Pool, confine};
use super::probe::{ProbeError, Prober};
use crate::config::InterstitialPool;

/// What a probe said about one file, kept so an unchanged file is not probed
/// again. A failure is kept too: a bad file stays bad until it changes.
type Probed = Result<Duration, String>;

#[derive(Debug, Clone)]
struct CacheEntry {
    modified: SystemTime,
    size: u64,
    probed: Probed,
}

/// The clips of every configured pool, with durations.
pub struct ClipLibrary {
    pools: Vec<Pool>,
    prober: Box<dyn Prober>,
    cache: HashMap<PathBuf, CacheEntry>,
}

impl ClipLibrary {
    /// Scans every pool. A bad directory or file never fails the scan; it is
    /// recorded in [`Pool::issues`] and the rest is still read.
    pub fn scan(pools: &[InterstitialPool], prober: impl Prober + 'static) -> Self {
        let mut library = Self {
            pools: pools
                .iter()
                .map(|p| Pool::new(p.name.clone(), p.path.clone(), Vec::new(), Vec::new()))
                .collect(),
            prober: Box::new(prober),
            cache: HashMap::new(),
        };
        library.rescan();
        library
    }

    /// Reads every pool directory again. Added files appear and removed files
    /// disappear. A file whose modified time and size are unchanged keeps its
    /// earlier duration and is not probed again.
    pub fn rescan(&mut self) {
        let old = std::mem::take(&mut self.cache);
        let mut cache = HashMap::new();
        let prober = self.prober.as_ref();
        for pool in &mut self.pools {
            *pool = scan_pool(pool.name(), pool.dir(), prober, &old, &mut cache);
        }
        // Only files seen in this scan are kept, so the cache cannot grow
        // with files that were deleted.
        self.cache = cache;
    }

    /// The pools in configuration order.
    pub fn pools(&self) -> &[Pool] {
        &self.pools
    }

    pub fn pool(&self, name: &str) -> Option<&Pool> {
        self.pools.iter().find(|p| p.name() == name)
    }

    /// Everything the last scan skipped, across all pools.
    pub fn issues(&self) -> impl Iterator<Item = &Issue> {
        self.pools.iter().flat_map(|p| p.issues())
    }

    /// Finds the pool that contains `path` and returns it with the path's
    /// canonical form. Refuses a path outside every configured directory,
    /// including `..` tricks and symlinks that lead out.
    pub fn confine(&self, path: &Path) -> Result<(&Pool, PathBuf), PathRefused> {
        let mut refused = PathRefused::Outside;
        for pool in &self.pools {
            match confine(pool.dir(), path) {
                Ok(canonical) => return Ok((pool, canonical)),
                Err(PathRefused::Outside) => {}
                // Report a resolve error only if no pool accepts the path.
                Err(e) => refused = e,
            }
        }
        Err(refused)
    }
}

fn scan_pool(
    name: &str,
    dir: &Path,
    prober: &dyn Prober,
    old: &HashMap<PathBuf, CacheEntry>,
    cache: &mut HashMap<PathBuf, CacheEntry>,
) -> Pool {
    let mut issues = Vec::new();
    let mut issue = |path: &Path, kind: IssueKind| {
        issues.push(Issue {
            pool: name.to_string(),
            path: path.to_path_buf(),
            kind,
        });
    };

    let (canonical_dir, entries) =
        match fs::canonicalize(dir).and_then(|c| Ok((c, fs::read_dir(dir)?))) {
            Ok(found) => found,
            Err(e) => {
                issue(dir, IssueKind::Directory(e.to_string()));
                return Pool::new(name, dir, Vec::new(), issues);
            }
        };

    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(e) => issue(dir, IssueKind::Unreadable(e.to_string())),
        }
    }
    // Sorted by file name so the order does not depend on the filesystem.
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut clips = Vec::new();
    for path in paths {
        // Hidden files (`.DS_Store`, editor leftovers) are not clips.
        if path
            .file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with('.'))
        {
            continue;
        }
        let canonical = match fs::canonicalize(&path) {
            Ok(c) => c,
            Err(e) => {
                issue(&path, IssueKind::Unreadable(e.to_string()));
                continue;
            }
        };
        if canonical == canonical_dir || !canonical.starts_with(&canonical_dir) {
            issue(&path, IssueKind::OutsidePool);
            continue;
        }
        let meta = match fs::metadata(&canonical) {
            Ok(m) => m,
            Err(e) => {
                issue(&path, IssueKind::Unreadable(e.to_string()));
                continue;
            }
        };
        // The scan is not recursive: subdirectories are not clips.
        if !meta.is_file() {
            continue;
        }

        let stamp = meta.modified().ok().map(|modified| (modified, meta.len()));
        let cached = stamp.and_then(|(modified, size)| {
            old.get(&canonical)
                .filter(|e| e.modified == modified && e.size == size)
        });
        let probed = match cached {
            Some(entry) => entry.probed.clone(),
            None => probe(prober, &canonical),
        };
        if let Some((modified, size)) = stamp {
            cache.insert(
                canonical.clone(),
                CacheEntry {
                    modified,
                    size,
                    probed: probed.clone(),
                },
            );
        }
        match probed {
            Ok(duration) => clips.push(Clip {
                path: canonical,
                duration,
            }),
            Err(why) => issue(&path, IssueKind::Probe(why)),
        }
    }
    Pool::new(name, dir, clips, issues)
}

fn probe(prober: &dyn Prober, path: &Path) -> Probed {
    match prober.duration(path) {
        Ok(d) if d.is_zero() => Err(ProbeError::NoDuration.to_string()),
        Ok(d) => Ok(d),
        Err(e) => Err(e.to_string()),
    }
}
