use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Why a duration could not be read from a file.
#[derive(Debug)]
pub enum ProbeError {
    /// The probe program could not be started, for example because ffprobe is
    /// not installed.
    Spawn(std::io::Error),
    /// The probe program ran longer than its time limit and was killed.
    TimedOut(Duration),
    /// The probe program exited with an error: the file is unreadable or not
    /// media. `None` means it was ended by a signal.
    Failed(Option<i32>),
    /// The probe program printed something that is not a number of seconds.
    Unparseable(String),
    /// The file has no duration, or a duration of zero.
    NoDuration,
}

impl fmt::Display for ProbeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn(e) => write!(f, "cannot run ffprobe: {e}"),
            Self::TimedOut(limit) => write!(f, "ffprobe took longer than {limit:?}"),
            Self::Failed(Some(code)) => write!(f, "ffprobe cannot read the file (exit {code})"),
            Self::Failed(None) => f.write_str("ffprobe was stopped by a signal"),
            Self::Unparseable(text) => write!(f, "ffprobe printed {text:?}, not a duration"),
            Self::NoDuration => f.write_str("the file has no positive duration"),
        }
    }
}

impl std::error::Error for ProbeError {}

/// Reads how long a media file plays.
pub trait Prober: Send + Sync {
    fn duration(&self, path: &Path) -> Result<Duration, ProbeError>;
}

/// A [`Prober`] that runs `ffprobe`. It needs no shell: the path is passed as
/// a single argument.
#[derive(Debug, Clone)]
pub struct FfprobeProber {
    program: PathBuf,
    timeout: Duration,
}

impl FfprobeProber {
    /// How long one file may take before it is given up on.
    pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

    /// Uses `ffprobe` from `PATH`.
    pub fn new() -> Self {
        Self::with_program("ffprobe")
    }

    /// Uses the given program, for an ffprobe outside `PATH`.
    pub fn with_program(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            timeout: Self::DEFAULT_TIMEOUT,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }
}

impl Default for FfprobeProber {
    fn default() -> Self {
        Self::new()
    }
}

impl Prober for FfprobeProber {
    fn duration(&self, path: &Path) -> Result<Duration, ProbeError> {
        let mut child = Command::new(&self.program)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "default=noprint_wrappers=1:nokey=1",
            ])
            // `-i` makes ffprobe take the next argument as the input even
            // when a relative path starts with `-`.
            .arg("-i")
            .arg(path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(ProbeError::Spawn)?;

        let deadline = Instant::now() + self.timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProbeError::TimedOut(self.timeout));
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(ProbeError::Spawn(e));
                }
            }
        };
        if !status.success() {
            return Err(ProbeError::Failed(status.code()));
        }

        // The output is one short line, far below the pipe buffer, so reading
        // after exit cannot block the child.
        let mut text = String::new();
        if let Some(mut out) = child.stdout.take() {
            let _ = out.by_ref().take(4096).read_to_string(&mut text);
        }
        parse_seconds(&text)
    }
}

/// Parses ffprobe's `duration` output. `N/A` and empty output mean the file
/// has no duration.
pub(super) fn parse_seconds(text: &str) -> Result<Duration, ProbeError> {
    let text = text.trim();
    if text.is_empty() || text == "N/A" {
        return Err(ProbeError::NoDuration);
    }
    let seconds: f64 = text
        .parse()
        .map_err(|_| ProbeError::Unparseable(text.to_string()))?;
    if seconds.is_nan() || seconds <= 0.0 {
        return Err(ProbeError::NoDuration);
    }
    Duration::try_from_secs_f64(seconds).map_err(|_| ProbeError::Unparseable(text.to_string()))
}
