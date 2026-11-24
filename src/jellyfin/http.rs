use std::io::Read;
use std::time::Duration;

/// How long a request may take to connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a request may take in all, reading the body included. A page of
/// 500 items with streams and chapters is a few hundred kilobytes.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// The most a response body may hold, to keep a wrong server from filling memory.
const MAX_BODY_BYTES: u64 = 64 * 1024 * 1024;

/// Why a request produced no body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// No answer: the connection failed, timed out or broke.
    Unreachable(String),
    /// The server answered with a status other than 2xx.
    Status(u16),
}

/// The one thing the Jellyfin client needs from HTTP: a GET that returns the
/// body. Tests supply recorded responses through this trait.
pub trait JellyfinHttp: Send + Sync {
    /// Sends `GET <server><path_and_query>` with the given `Authorization`
    /// header value and returns the response body. The client builds the header;
    /// a transport must send it as given and must not log it.
    fn get(&self, path_and_query: &str, authorization: &str) -> Result<String, HttpError>;
}

/// [`JellyfinHttp`] over `ureq`, with a short timeout. Redirects are not
/// followed, so the key can never be sent to a host other than the configured one;
/// a redirect shows up as [`HttpError::Status`]. Use the address the server
/// serves from directly.
#[derive(Debug)]
pub struct UreqHttp {
    base_url: String,
    agent: ureq::Agent,
}

impl UreqHttp {
    pub fn new(base_url: &str) -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_global(Some(REQUEST_TIMEOUT))
            .http_status_as_error(false)
            .max_redirects(0)
            .build()
            .into();
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            agent,
        }
    }
}

impl JellyfinHttp for UreqHttp {
    fn get(&self, path_and_query: &str, authorization: &str) -> Result<String, HttpError> {
        let url = format!("{}{path_and_query}", self.base_url);
        let mut response = self
            .agent
            .get(&url)
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .call()
            .map_err(|source| HttpError::Unreachable(source.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(HttpError::Status(status));
        }
        let mut body = String::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_BODY_BYTES)
            .read_to_string(&mut body)
            .map_err(|source| HttpError::Unreachable(source.to_string()))?;
        Ok(body)
    }
}
