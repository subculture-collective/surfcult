use std::fmt;

/// Why a store call failed. SQLite's own error type is turned into text so
/// callers and tests do not depend on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StoreError {
    /// SQLite failed. `action` says what the store was doing.
    Database {
        action: &'static str,
        message: String,
    },
    /// The data to write is not allowed. Nothing was written.
    InvalidEvent(String),
    /// The database was written by a newer build than this one.
    NewerSchema { found: u32, supported: u32 },
    /// A stored value cannot be turned back into its Rust type.
    Corrupt(String),
}

impl StoreError {
    pub(super) fn database(action: &'static str) -> impl FnOnce(rusqlite::Error) -> Self {
        move |source| Self::Database {
            action,
            message: source.to_string(),
        }
    }
}

impl fmt::Display for StoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Database { action, message } => {
                write!(f, "database error while {action}: {message}")
            }
            Self::InvalidEvent(reason) => write!(f, "invalid event: {reason}"),
            Self::NewerSchema { found, supported } => write!(
                f,
                "database schema version {found} is newer than this build supports ({supported})"
            ),
            Self::Corrupt(reason) => write!(f, "stored data is unreadable: {reason}"),
        }
    }
}

impl std::error::Error for StoreError {}
