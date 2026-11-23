use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// One playable clip in a pool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Clip {
    /// The canonical path: absolute, with symlinks resolved.
    pub path: PathBuf,
    pub duration: Duration,
}

/// Why a file or directory in a pool was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IssueKind {
    /// The pool directory is missing or cannot be read. The pool is empty.
    Directory(String),
    /// An entry could not be resolved, for example a broken symlink.
    Unreadable(String),
    /// The entry resolves to a place outside the pool directory, for example
    /// through a symlink.
    OutsidePool,
    /// ffprobe could not read the file or found no positive duration.
    Probe(String),
}

impl fmt::Display for IssueKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory(why) => write!(f, "cannot read the pool directory: {why}"),
            Self::Unreadable(why) => write!(f, "cannot resolve the path: {why}"),
            Self::OutsidePool => f.write_str("resolves outside the pool directory"),
            Self::Probe(why) => f.write_str(why),
        }
    }
}

/// A file or directory that a scan skipped, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub pool: String,
    pub path: PathBuf,
    pub kind: IssueKind,
}

impl fmt::Display for Issue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.pool, self.path.display(), self.kind)
    }
}

/// A named directory and the clips found in it by the last scan.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pool {
    name: String,
    dir: PathBuf,
    clips: Vec<Clip>,
    issues: Vec<Issue>,
}

impl Pool {
    pub fn new(
        name: impl Into<String>,
        dir: impl Into<PathBuf>,
        clips: Vec<Clip>,
        issues: Vec<Issue>,
    ) -> Self {
        Self {
            name: name.into(),
            dir: dir.into(),
            clips,
            issues,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The directory as configured.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The clips, sorted by file name. The order is the same on every scan of
    /// the same files.
    pub fn clips(&self) -> &[Clip] {
        &self.clips
    }

    /// What the last scan skipped.
    pub fn issues(&self) -> &[Issue] {
        &self.issues
    }

    /// The length of all clips together.
    pub fn total_duration(&self) -> Duration {
        self.clips.iter().map(|c| c.duration).sum()
    }

    /// Checks that `path` is inside this pool's directory and returns its
    /// canonical form. See [`confine`].
    pub fn confine(&self, path: &Path) -> Result<PathBuf, PathRefused> {
        confine(&self.dir, path)
    }
}

/// Why a path was refused.
#[derive(Debug)]
pub enum PathRefused {
    /// The path or the directory does not exist or cannot be resolved.
    Unresolvable(std::io::Error),
    /// The path resolves to a place outside the directory.
    Outside,
}

impl fmt::Display for PathRefused {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unresolvable(e) => write!(f, "cannot resolve the path: {e}"),
            Self::Outside => f.write_str("the path is outside the configured directory"),
        }
    }
}

impl std::error::Error for PathRefused {}

/// Returns the canonical form of `path` if it is inside `dir`, and refuses
/// everything else, including `dir` itself.
///
/// Both are resolved first, so `..` components and symlinks are followed
/// before the check: a link inside `dir` that points elsewhere is refused. A
/// path that does not exist is refused too, because it cannot be resolved.
pub fn confine(dir: &Path, path: &Path) -> Result<PathBuf, PathRefused> {
    let dir = dir.canonicalize().map_err(PathRefused::Unresolvable)?;
    let path = path.canonicalize().map_err(PathRefused::Unresolvable)?;
    if path != dir && path.starts_with(&dir) {
        Ok(path)
    } else {
        Err(PathRefused::Outside)
    }
}
