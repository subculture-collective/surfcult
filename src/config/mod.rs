//! `channels.toml`: the source of truth for channels, rules and breaks.
//!
//! [`Config::load`] parses the file and checks it. It reads no media and does
//! not look at the filesystem beyond the file itself; sources are resolved
//! against Jellyfin elsewhere.

mod duration;
mod raw;

use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub use chrono_tz::Tz;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub timezone: Tz,
    pub channels: Vec<Channel>,
    pub pools: Vec<InterstitialPool>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub number: u32,
    pub name: String,
    pub plan: Plan,
    /// Programs start on this grid and gaps are filled.
    pub grid: Option<Duration>,
    pub breaks: Option<Breaks>,
    /// Pool used to fill grid gaps and to pad commercial breaks the break
    /// pool leaves short.
    pub filler: Option<String>,
}

/// What a channel plays: one source in one mode, or time-of-day blocks that
/// each have their own.
#[derive(Debug, Clone, PartialEq)]
pub enum Plan {
    Fixed { source: Source, mode: Mode },
    Blocks(Vec<Block>),
}

/// Where a channel's items come from in Jellyfin.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    JellyfinLibrary(String),
    JellyfinCollection(String),
    JellyfinTag(String),
    JellyfinGenre(String),
    JellyfinSeries(String),
}

/// How items are ordered. A channel's `mode = "blocks"` is a [`Plan::Blocks`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Sequential,
    Shuffle,
    Weighted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Day {
    Mon,
    Tue,
    Wed,
    Thu,
    Fri,
    Sat,
    Sun,
}

/// A time-of-day window with its own source and mode.
///
/// Times are minutes after midnight in the config time zone. `end_minute` may be
/// 1440 (24:00); a block does not wrap past midnight.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub days: Vec<Day>,
    pub start_minute: u16,
    pub end_minute: u16,
    pub source: Source,
    pub mode: Mode,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Breaks {
    pub every: Duration,
    pub length: Duration,
    pub pool: String,
}

/// A named directory of short local clips.
#[derive(Debug, Clone, PartialEq)]
pub struct InterstitialPool {
    pub name: String,
    pub path: PathBuf,
}

/// One thing wrong with the file, located by its TOML key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    /// For example `channel[1].mode`. Indexes count from zero.
    pub field: String,
    pub message: String,
}

#[derive(Debug)]
pub enum ConfigError {
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    /// The text is not valid TOML or has the wrong shape. The message carries
    /// the line and the key.
    Parse(String),
    Invalid(Vec<Problem>),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Read { path, source } => write!(f, "cannot read {}: {source}", path.display()),
            Self::Parse(message) => f.write_str(message.trim_end()),
            Self::Invalid(problems) => {
                for (i, problem) in problems.iter().enumerate() {
                    if i > 0 {
                        writeln!(f)?;
                    }
                    write!(f, "{}: {}", problem.field, problem.message)?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    /// Reads and checks a `channels.toml` file.
    pub fn load(path: &Path) -> Result<Self, ConfigError> {
        let text = std::fs::read_to_string(path).map_err(|source| ConfigError::Read {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(&text)
    }

    /// Parses and checks `channels.toml` text.
    pub fn parse(text: &str) -> Result<Self, ConfigError> {
        let raw: raw::RawConfig =
            toml::from_str(text).map_err(|e| ConfigError::Parse(e.to_string()))?;
        raw.into_config().map_err(ConfigError::Invalid)
    }
}

#[cfg(test)]
mod tests;
