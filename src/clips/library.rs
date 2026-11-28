use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, UNIX_EPOCH};

use super::pool::{Clip, Issue, IssueKind, PathRefused, Pool, confine};
use super::probe::{ProbeError, Prober};
use crate::config::InterstitialPool;

/// What a probe said about one file, kept so an unchanged file is not probed
/// again. A failure is kept too: a bad file stays bad until it changes.
pub type Probed = Result<Duration, String>;

/// A file's modified time, in nanoseconds since the Unix epoch, and its size
/// in bytes. A file whose stamp is unchanged is not probed again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stamp {
    pub modified_ns: i64,
    pub size: u64,
}

/// One probe result as the store keeps it between runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRecord {
    /// The canonical path.
    pub path: PathBuf,
    pub stamp: Stamp,
    pub probed: Probed,
}

/// The probe results to write to the store and the paths whose results to
/// delete, since the last [`ClipLibrary::take_changes`].
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProbeChanges {
    pub saved: Vec<ProbeRecord>,
    pub removed: Vec<PathBuf>,
}

impl ProbeChanges {
    pub fn is_empty(&self) -> bool {
        self.saved.is_empty() && self.removed.is_empty()
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    /// `None` when the file has no readable modified time: the result is
    /// used until the next rescan, which probes the file again.
    stamp: Option<Stamp>,
    probed: Probed,
}

/// One entry of a pool directory as the last rescan found it, in pool order.
#[derive(Debug, Clone)]
enum Listed {
    /// Skipped before probing.
    Issue(Issue),
    /// A file to take a duration for, from the cache once it is probed.
    File {
        path: PathBuf,
        canonical: PathBuf,
        stamp: Option<Stamp>,
    },
}

/// How files get probed.
enum Probing {
    /// In the caller's thread, during [`ClipLibrary::rescan`].
    Inline(Box<dyn Prober>),
    /// On worker threads; [`ClipLibrary::collect`] takes the results.
    Background(Background),
}

struct Job {
    canonical: PathBuf,
    stamp: Option<Stamp>,
}

struct Background {
    jobs: Sender<Job>,
    results: Receiver<(Job, Probed)>,
    /// Files queued and not yet answered, with the stamp they were queued
    /// with. A result for another stamp is out of date and dropped.
    pending: HashMap<PathBuf, Option<Stamp>>,
    /// Set when the library is dropped, so the workers skip the jobs still
    /// queued instead of probing them.
    stop: Arc<AtomicBool>,
}

impl Drop for Background {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// The clips of every configured pool, with durations.
pub struct ClipLibrary {
    pools: Vec<Pool>,
    listings: Vec<Vec<Listed>>,
    probing: Probing,
    cache: HashMap<PathBuf, CacheEntry>,
    changes: ProbeChanges,
}

impl ClipLibrary {
    /// Scans every pool, probing each file in this thread before it returns.
    /// A bad directory or file never fails the scan; it is recorded in
    /// [`Pool::issues`] and the rest is still read.
    pub fn scan(pools: &[InterstitialPool], prober: impl Prober + 'static) -> Self {
        let mut library = Self::empty(pools, Probing::Inline(Box::new(prober)));
        library.rescan();
        library
    }

    /// Scans every pool without probing in this thread. A file in `known`
    /// whose path, modified time and size still match keeps its stored
    /// result; every other file is probed on `workers` background threads
    /// (at least one) and joins its pool when [`collect`](Self::collect)
    /// takes its result. Until then it is in no pool.
    pub fn start(
        pools: &[InterstitialPool],
        prober: impl Prober + 'static,
        known: Vec<ProbeRecord>,
        workers: usize,
    ) -> Self {
        let mut library = Self::empty(pools, Probing::Background(spawn(prober, workers)));
        library.cache = known
            .into_iter()
            .map(|record| {
                let entry = CacheEntry {
                    stamp: Some(record.stamp),
                    probed: record.probed,
                };
                (record.path, entry)
            })
            .collect();
        library.rescan();
        library
    }

    fn empty(pools: &[InterstitialPool], probing: Probing) -> Self {
        Self {
            pools: pools
                .iter()
                .map(|p| Pool::new(p.name.clone(), p.path.clone(), Vec::new(), Vec::new()))
                .collect(),
            listings: vec![Vec::new(); pools.len()],
            probing,
            cache: HashMap::new(),
            changes: ProbeChanges::default(),
        }
    }

    /// Reads every pool directory again. Added files appear and removed files
    /// disappear. A file whose modified time and size are unchanged keeps its
    /// earlier duration and is not probed again. Returns how many files were
    /// queued for a background probe; inline probing queues none.
    pub fn rescan(&mut self) -> usize {
        let listings: Vec<Vec<Listed>> = self
            .pools
            .iter()
            .map(|pool| list_pool(pool.name(), pool.dir()))
            .collect();

        let mut old = std::mem::take(&mut self.cache);
        let mut queued = 0;
        for listed in listings.iter().flatten() {
            let Listed::File {
                canonical, stamp, ..
            } = listed
            else {
                continue;
            };
            // A file listed in two pools is looked up once.
            if self.cache.contains_key(canonical) {
                continue;
            }
            let fresh = old
                .remove(canonical)
                .filter(|entry| stamp.is_some() && entry.stamp == *stamp);
            if let Some(entry) = fresh {
                self.cache.insert(canonical.clone(), entry);
                continue;
            }
            match &mut self.probing {
                Probing::Inline(prober) => {
                    let probed = probe(prober.as_ref(), canonical);
                    self.record(canonical.clone(), *stamp, probed);
                }
                Probing::Background(background) => {
                    if background.pending.get(canonical) == Some(stamp) {
                        continue;
                    }
                    background.pending.insert(canonical.clone(), *stamp);
                    let job = Job {
                        canonical: canonical.clone(),
                        stamp: *stamp,
                    };
                    // The workers live as long as the sender, so this
                    // only fails if one panicked; the file stays unprobed.
                    let _ = background.jobs.send(job);
                    queued += 1;
                }
            }
        }
        // Only files seen in this scan are kept, so the cache cannot grow
        // with files that were deleted.
        self.changes.removed.extend(old.into_keys());
        if let Probing::Background(background) = &mut self.probing {
            let listed: std::collections::HashSet<&PathBuf> = listings
                .iter()
                .flatten()
                .filter_map(|listed| match listed {
                    Listed::File { canonical, .. } => Some(canonical),
                    Listed::Issue(_) => None,
                })
                .collect();
            background.pending.retain(|path, _| listed.contains(path));
        }
        self.listings = listings;
        self.rebuild();
        queued
    }

    /// Takes the background probe results that have arrived and adds their
    /// files to their pools. Never waits. Returns whether any pool changed.
    pub fn collect(&mut self) -> bool {
        let Probing::Background(background) = &mut self.probing else {
            return false;
        };
        let mut done = Vec::new();
        while let Ok((job, probed)) = background.results.try_recv() {
            if background.pending.get(&job.canonical) == Some(&job.stamp) {
                background.pending.remove(&job.canonical);
                done.push((job, probed));
            }
        }
        if done.is_empty() {
            return false;
        }
        for (job, probed) in done {
            self.record(job.canonical, job.stamp, probed);
        }
        self.rebuild();
        true
    }

    /// How many files wait for a background probe.
    pub fn pending(&self) -> usize {
        match &self.probing {
            Probing::Inline(_) => 0,
            Probing::Background(background) => background.pending.len(),
        }
    }

    /// The probe results to store and the stored results to delete since the
    /// last call. A result for a file without a modified time is not stored.
    pub fn take_changes(&mut self) -> ProbeChanges {
        std::mem::take(&mut self.changes)
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

    fn record(&mut self, canonical: PathBuf, stamp: Option<Stamp>, probed: Probed) {
        if let Some(stamp) = stamp {
            self.changes.saved.push(ProbeRecord {
                path: canonical.clone(),
                stamp,
                probed: probed.clone(),
            });
        }
        self.cache.insert(canonical, CacheEntry { stamp, probed });
    }

    /// Builds the pools from the last listings and the cache. Reads no files.
    fn rebuild(&mut self) {
        for (pool, listing) in self.pools.iter_mut().zip(&self.listings) {
            let mut clips = Vec::new();
            let mut issues = Vec::new();
            for listed in listing {
                match listed {
                    Listed::Issue(issue) => issues.push(issue.clone()),
                    Listed::File {
                        path, canonical, ..
                    } => match self.cache.get(canonical).map(|e| &e.probed) {
                        Some(Ok(duration)) => clips.push(Clip {
                            path: canonical.clone(),
                            duration: *duration,
                        }),
                        Some(Err(why)) => issues.push(Issue {
                            pool: pool.name().to_string(),
                            path: path.clone(),
                            kind: IssueKind::Probe(why.clone()),
                        }),
                        // Still being probed.
                        None => {}
                    },
                }
            }
            *pool = Pool::new(pool.name(), pool.dir(), clips, issues);
        }
    }
}

/// Starts the probe workers. They stop when the job sender is dropped.
fn spawn(prober: impl Prober + 'static, workers: usize) -> Background {
    let (jobs, job_rx) = mpsc::channel::<Job>();
    let (result_tx, results) = mpsc::channel();
    let job_rx = Arc::new(Mutex::new(job_rx));
    let prober: Arc<dyn Prober> = Arc::new(prober);
    let stop = Arc::new(AtomicBool::new(false));
    for n in 0..workers.max(1) {
        let stop = Arc::clone(&stop);
        let job_rx = Arc::clone(&job_rx);
        let result_tx = result_tx.clone();
        let prober = Arc::clone(&prober);
        let spawned = std::thread::Builder::new()
            .name(format!("surfcult-probe-{n}"))
            .spawn(move || {
                loop {
                    let job = job_rx.lock().unwrap_or_else(PoisonError::into_inner).recv();
                    let Ok(job) = job else { break };
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let probed = probe(prober.as_ref(), &job.canonical);
                    if result_tx.send((job, probed)).is_err() {
                        break;
                    }
                }
            });
        if let Err(e) = spawned {
            eprintln!("surfcult: cannot start a clip probe thread: {e}");
        }
    }
    Background {
        jobs,
        results,
        pending: HashMap::new(),
        stop,
    }
}

/// Lists one pool directory in clip order: every file to probe and every
/// entry skipped before probing.
fn list_pool(name: &str, dir: &Path) -> Vec<Listed> {
    let mut listing = Vec::new();
    let issue = |path: &Path, kind: IssueKind| {
        Listed::Issue(Issue {
            pool: name.to_string(),
            path: path.to_path_buf(),
            kind,
        })
    };

    let (canonical_dir, entries) =
        match fs::canonicalize(dir).and_then(|c| Ok((c, fs::read_dir(dir)?))) {
            Ok(found) => found,
            Err(e) => return vec![issue(dir, IssueKind::Directory(e.to_string()))],
        };

    let mut paths = Vec::new();
    for entry in entries {
        match entry {
            Ok(entry) => paths.push(entry.path()),
            Err(e) => listing.push(issue(dir, IssueKind::Unreadable(e.to_string()))),
        }
    }
    // Sorted by file name so the order does not depend on the filesystem.
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

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
                listing.push(issue(&path, IssueKind::Unreadable(e.to_string())));
                continue;
            }
        };
        if canonical == canonical_dir || !canonical.starts_with(&canonical_dir) {
            listing.push(issue(&path, IssueKind::OutsidePool));
            continue;
        }
        let meta = match fs::metadata(&canonical) {
            Ok(m) => m,
            Err(e) => {
                listing.push(issue(&path, IssueKind::Unreadable(e.to_string())));
                continue;
            }
        };
        // The scan is not recursive: subdirectories are not clips.
        if !meta.is_file() {
            continue;
        }
        let stamp = meta
            .modified()
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .and_then(|since| i64::try_from(since.as_nanos()).ok())
            .map(|modified_ns| Stamp {
                modified_ns,
                size: meta.len(),
            });
        listing.push(Listed::File {
            path,
            canonical,
            stamp,
        });
    }
    listing
}

fn probe(prober: &dyn Prober, path: &Path) -> Probed {
    match prober.duration(path) {
        Ok(d) if d.is_zero() => Err(ProbeError::NoDuration.to_string()),
        Ok(d) => Ok(d),
        Err(e) => Err(e.to_string()),
    }
}
