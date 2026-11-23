use std::fmt;
use std::fs;

/// The environment variable that holds the API key itself.
pub const KEY_ENV: &str = "SURFCULT_JELLYFIN_API_KEY";
/// The environment variable that holds the path of a file containing the key.
pub const KEY_FILE_ENV: &str = "SURFCULT_JELLYFIN_API_KEY_FILE";

/// A Jellyfin API key. The value never appears in `Debug` output or in an
/// error message, and the type has no `Display`. The only way to read it back
/// is to build the `Authorization` header with `authorization_header`.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    /// Wraps a key. Surrounding whitespace is trimmed. A key that is empty or
    /// holds characters that cannot sit inside a quoted header value (control
    /// characters, a double quote or a backslash) is refused.
    pub fn new(key: impl AsRef<str>) -> Result<Self, KeyError> {
        let key = key.as_ref().trim();
        if key.is_empty() {
            return Err(KeyError::Empty);
        }
        if key.chars().any(|c| c.is_control() || c == '"' || c == '\\') {
            return Err(KeyError::BadCharacters);
        }
        Ok(Self(key.to_string()))
    }

    /// Reads the key from `SURFCULT_JELLYFIN_API_KEY`, or else from the file
    /// named by `SURFCULT_JELLYFIN_API_KEY_FILE`.
    pub fn from_env() -> Result<Self, KeyError> {
        Self::resolve(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with the environment passed in, so tests need not
    /// change the real one. A variable that is unset or empty counts as absent.
    pub(super) fn resolve(env: impl Fn(&str) -> Option<String>) -> Result<Self, KeyError> {
        let present = |name| env(name).filter(|value| !value.trim().is_empty());
        if let Some(key) = present(KEY_ENV) {
            return Self::new(key);
        }
        if let Some(path) = present(KEY_FILE_ENV) {
            let content = fs::read_to_string(&path).map_err(|source| KeyError::File {
                path,
                reason: source.kind().to_string(),
            })?;
            return Self::new(content);
        }
        Err(KeyError::NotConfigured)
    }

    /// The only place the key leaves this type: the value of the
    /// `Authorization` header Jellyfin 12 accepts.
    pub(super) fn authorization_header(&self) -> String {
        format!("MediaBrowser Token=\"{}\"", self.0)
    }

    /// `text` with every copy of the key replaced, for text that comes from a
    /// transport and might repeat what it was sent.
    pub(super) fn scrub(&self, text: &str) -> String {
        text.replace(&self.0, "[redacted]")
    }
}

impl fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiKey(<redacted>)")
    }
}

/// Why the API key could not be loaded. None of these carry the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// Neither environment variable is set.
    NotConfigured,
    /// The key is empty.
    Empty,
    /// The key has a control character, a double quote or a backslash.
    BadCharacters,
    /// The key file could not be read.
    File { path: String, reason: String },
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotConfigured => write!(
                f,
                "no Jellyfin API key: set {KEY_ENV}, or set {KEY_FILE_ENV} to a file that holds it"
            ),
            Self::Empty => f.write_str("the Jellyfin API key is empty"),
            Self::BadCharacters => f.write_str(
                "the Jellyfin API key has a control character, a double quote or a backslash",
            ),
            Self::File { path, reason } => {
                write!(f, "cannot read the Jellyfin API key file {path}: {reason}")
            }
        }
    }
}

impl std::error::Error for KeyError {}

/// Where Jellyfin is and how to sign in.
#[derive(Debug, Clone)]
pub struct JellyfinConfig {
    /// The server's address, such as `http://10.0.0.200:8096`. A trailing
    /// slash is ignored.
    pub base_url: String,
    pub key: ApiKey,
}
