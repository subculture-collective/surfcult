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

/// What the Jellyfin client needs from HTTP: a GET and a POST that return the
/// body, and a DELETE. Tests supply recorded responses through this trait.
///
/// Every method takes the `Authorization` header value. The client builds the
/// header; a transport must send it as given and must not log it.
pub trait JellyfinHttp: Send + Sync {
    /// Sends `GET <server><path_and_query>` and returns the response body.
    fn get(&self, path_and_query: &str, authorization: &str) -> Result<String, HttpError>;

    /// Sends `POST <server><path_and_query>` with `json` as an
    /// `application/json` body and returns the response body.
    fn post_json(
        &self,
        path_and_query: &str,
        json: &str,
        authorization: &str,
    ) -> Result<String, HttpError>;

    /// Sends `DELETE <server><path_and_query>`. The response body is ignored.
    fn delete(&self, path_and_query: &str, authorization: &str) -> Result<(), HttpError>;

    /// As [`JellyfinHttp::get`], for a body that is not text, such as an
    /// image. The default is `get`'s body as bytes, which test transports
    /// use.
    fn get_bytes(&self, path_and_query: &str, authorization: &str) -> Result<Vec<u8>, HttpError> {
        self.get(path_and_query, authorization)
            .map(String::into_bytes)
    }
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

impl UreqHttp {
    fn url(&self, path_and_query: &str) -> String {
        format!("{}{path_and_query}", self.base_url)
    }
}

impl JellyfinHttp for UreqHttp {
    fn get(&self, path_and_query: &str, authorization: &str) -> Result<String, HttpError> {
        let response = self
            .agent
            .get(&self.url(path_and_query))
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .call();
        read_body(response)
    }

    fn post_json(
        &self,
        path_and_query: &str,
        json: &str,
        authorization: &str,
    ) -> Result<String, HttpError> {
        let response = self
            .agent
            .post(&self.url(path_and_query))
            .header("Authorization", authorization)
            .header("Accept", "application/json")
            .header("Content-Type", "application/json")
            .send(json);
        read_body(response)
    }

    fn delete(&self, path_and_query: &str, authorization: &str) -> Result<(), HttpError> {
        let response = self
            .agent
            .delete(&self.url(path_and_query))
            .header("Authorization", authorization)
            .call();
        read_body(response).map(drop)
    }

    fn get_bytes(&self, path_and_query: &str, authorization: &str) -> Result<Vec<u8>, HttpError> {
        let response = self
            .agent
            .get(&self.url(path_and_query))
            .header("Authorization", authorization)
            .call();
        let mut response = response.map_err(|source| HttpError::Unreachable(source.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(HttpError::Status(status));
        }
        let mut body = Vec::new();
        response
            .body_mut()
            .as_reader()
            .take(MAX_BODY_BYTES)
            .read_to_end(&mut body)
            .map_err(|source| HttpError::Unreachable(source.to_string()))?;
        Ok(body)
    }
}

/// The body of a 2xx response, or why there is none.
fn read_body(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
) -> Result<String, HttpError> {
    let mut response = response.map_err(|source| HttpError::Unreachable(source.to_string()))?;
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
