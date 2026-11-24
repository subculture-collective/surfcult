use std::fmt;
use std::fs;
use std::net::{Ipv4Addr, SocketAddr};

/// The environment variable that holds the address to listen on.
pub const ADDR_ENV: &str = "SURFCULT_API_ADDR";
/// The environment variable that holds the bearer token itself.
pub const TOKEN_ENV: &str = "SURFCULT_API_TOKEN";
/// The environment variable that holds the path of a file containing the token.
pub const TOKEN_FILE_ENV: &str = "SURFCULT_API_TOKEN_FILE";

/// Where the server listens when nothing else is configured: localhost only.
pub const DEFAULT_ADDR: SocketAddr =
    SocketAddr::new(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST), 8090);

/// A bearer token clients must send. The value never appears in `Debug`
/// output or in an error message, and the type has no `Display`.
#[derive(Clone, PartialEq, Eq)]
pub struct ApiToken(String);

impl ApiToken {
    /// Wraps a token. Surrounding whitespace is trimmed. A token that is empty
    /// or holds whitespace or control characters is refused, because it could
    /// not be sent as one `Authorization: Bearer` value.
    pub fn new(token: impl AsRef<str>) -> Result<Self, SettingsError> {
        let token = token.as_ref().trim();
        if token.is_empty() {
            return Err(SettingsError::EmptyToken);
        }
        if token.chars().any(|c| c.is_control() || c.is_whitespace()) {
            return Err(SettingsError::BadToken);
        }
        Ok(Self(token.to_string()))
    }

    /// Whether `presented` is this token. The comparison takes the same time
    /// wherever the first difference is, so timing does not reveal how much
    /// of a guess was right. It does reveal whether the lengths match.
    pub(super) fn matches(&self, presented: &str) -> bool {
        let expected = self.0.as_bytes();
        let presented = presented.as_bytes();
        if expected.len() != presented.len() {
            return false;
        }
        let difference = expected
            .iter()
            .zip(presented)
            .fold(0u8, |acc, (a, b)| acc | (a ^ b));
        std::hint::black_box(difference) == 0
    }
}

impl fmt::Debug for ApiToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ApiToken(<redacted>)")
    }
}

/// How the read API is reached: the address it listens on and the token, if
/// any, that requests must carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub addr: SocketAddr,
    /// `None` means requests need no token.
    pub token: Option<ApiToken>,
}

impl Default for ServerSettings {
    /// Localhost on port 8090, no token.
    fn default() -> Self {
        Self {
            addr: DEFAULT_ADDR,
            token: None,
        }
    }
}

impl ServerSettings {
    /// Reads `SURFCULT_API_ADDR` and the token from `SURFCULT_API_TOKEN`, or
    /// else from the file named by `SURFCULT_API_TOKEN_FILE`. Without an
    /// address the server listens on [`DEFAULT_ADDR`]; without a token it
    /// accepts every request.
    pub fn from_env() -> Result<Self, SettingsError> {
        Self::resolve(|name| std::env::var(name).ok())
    }

    /// [`Self::from_env`] with the environment passed in, so tests need not
    /// change the real one. A variable that is unset or empty counts as absent.
    /// A token file that is named but cannot be read is an error, not an
    /// open server.
    pub(super) fn resolve(env: impl Fn(&str) -> Option<String>) -> Result<Self, SettingsError> {
        let present = |name| env(name).filter(|value| !value.trim().is_empty());
        let addr = match present(ADDR_ENV) {
            Some(text) => text
                .trim()
                .parse()
                .map_err(|_| SettingsError::BadAddr(text))?,
            None => DEFAULT_ADDR,
        };
        let token = if let Some(token) = present(TOKEN_ENV) {
            Some(ApiToken::new(token)?)
        } else if let Some(path) = present(TOKEN_FILE_ENV) {
            let content = fs::read_to_string(&path).map_err(|source| SettingsError::TokenFile {
                path,
                reason: source.kind().to_string(),
            })?;
            Some(ApiToken::new(content)?)
        } else {
            None
        };
        Ok(Self { addr, token })
    }
}

/// Why the server settings could not be loaded. None of these carry the token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsError {
    /// `SURFCULT_API_ADDR` is not an `ip:port` address.
    BadAddr(String),
    /// The token is empty.
    EmptyToken,
    /// The token has whitespace or a control character.
    BadToken,
    /// The token file could not be read.
    TokenFile { path: String, reason: String },
}

impl fmt::Display for SettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadAddr(text) => {
                write!(
                    f,
                    "{ADDR_ENV} is {text:?}, not an address such as 127.0.0.1:8090"
                )
            }
            Self::EmptyToken => f.write_str("the API token is empty"),
            Self::BadToken => f.write_str("the API token has whitespace or a control character"),
            Self::TokenFile { path, reason } => {
                write!(f, "cannot read the API token file {path}: {reason}")
            }
        }
    }
}

impl std::error::Error for SettingsError {}
