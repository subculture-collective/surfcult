use std::collections::{HashMap, HashSet};
use std::fmt;

use serde_json::Value;

use super::config::{ApiKey, JellyfinConfig};
use super::http::{HttpError, JellyfinHttp, UreqHttp};
use super::wire::{self, Parsed};
use crate::library::{Collection, MediaId};
use crate::store::{LibraryEntry, StoreError};

/// Items asked for per request.
const PAGE_SIZE: usize = 500;

/// Why talking to Jellyfin, or caching what it said, failed. None of these
/// carry the API key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JellyfinError {
    /// Jellyfin did not answer.
    Unreachable(String),
    /// Jellyfin answered 401 or 403: the key is wrong or may not read the library.
    Unauthorized,
    /// Jellyfin answered with another error status.
    Status(u16),
    /// The answer was not the JSON surfcult expects.
    BadResponse(String),
    /// Jellyfin offered no way to play the item, or refused to (its
    /// `ErrorCode`, such as `NoCompatibleStream`).
    NoPlayback(String),
    /// Jellyfin's transcode URL is for another item than the one asked for.
    /// Holds the item id the URL names.
    WrongItem(String),
    /// The local cache could not be read or written.
    Store(StoreError),
}

impl fmt::Display for JellyfinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unreachable(reason) => write!(f, "Jellyfin is unreachable: {reason}"),
            Self::Unauthorized => f.write_str("Jellyfin refused the API key (HTTP 401 or 403)"),
            Self::Status(code) => write!(f, "Jellyfin answered with HTTP status {code}"),
            Self::BadResponse(reason) => {
                write!(f, "Jellyfin sent a response surfcult cannot read: {reason}")
            }
            Self::NoPlayback(reason) => write!(f, "Jellyfin cannot play the item: {reason}"),
            Self::WrongItem(other) => {
                write!(f, "Jellyfin offered a transcode of another item ({other})")
            }
            Self::Store(source) => write!(f, "library cache: {source}"),
        }
    }
}

impl std::error::Error for JellyfinError {}

impl From<StoreError> for JellyfinError {
    fn from(source: StoreError) -> Self {
        Self::Store(source)
    }
}

/// Everything one fetch read from Jellyfin.
pub(super) struct FetchedLibrary {
    pub entries: Vec<LibraryEntry>,
    pub collections: Vec<Collection>,
    /// Items listed but not kept: placeholders, items without a runtime, and
    /// items whose JSON had the wrong shape.
    pub skipped: usize,
}

/// A read-only Jellyfin client. It holds the API key and sends it only in the
/// `Authorization` header, never in a URL.
pub struct JellyfinClient {
    http: Box<dyn JellyfinHttp>,
    key: ApiKey,
}

impl JellyfinClient {
    /// A client that talks to `config.base_url` over HTTP(S).
    pub fn new(config: JellyfinConfig) -> Self {
        let http = UreqHttp::new(&config.base_url);
        Self::with_transport(config.key, http)
    }

    /// A client that sends its requests through `http`. Tests use this with
    /// recorded responses.
    pub fn with_transport(key: ApiKey, http: impl JellyfinHttp + 'static) -> Self {
        Self {
            http: Box::new(http),
            key,
        }
    }

    fn get(&self, path_and_query: &str) -> Result<String, JellyfinError> {
        self.http
            .get(path_and_query, &self.key.authorization_header())
            .map_err(|error| self.http_error(error))
    }

    pub(super) fn post_json(
        &self,
        path_and_query: &str,
        json: &str,
    ) -> Result<String, JellyfinError> {
        self.http
            .post_json(path_and_query, json, &self.key.authorization_header())
            .map_err(|error| self.http_error(error))
    }

    pub(super) fn delete(&self, path_and_query: &str) -> Result<(), JellyfinError> {
        self.http
            .delete(path_and_query, &self.key.authorization_header())
            .map_err(|error| self.http_error(error))
    }

    fn http_error(&self, error: HttpError) -> JellyfinError {
        match error {
            HttpError::Unreachable(reason) => {
                // The text comes from outside; make sure it cannot repeat the key.
                JellyfinError::Unreachable(self.scrub(&reason))
            }
            HttpError::Status(401 | 403) => JellyfinError::Unauthorized,
            HttpError::Status(code) => JellyfinError::Status(code),
        }
    }

    /// `text` with every copy of the key replaced.
    pub(super) fn scrub(&self, text: &str) -> String {
        self.key.scrub(text)
    }

    /// Reads every page of `/Items?{query}` and returns the raw items.
    fn get_items(&self, query: &str) -> Result<Vec<Value>, JellyfinError> {
        let mut items = Vec::new();
        loop {
            let start = items.len();
            let body = self.get(&format!(
                "/Items?{query}&StartIndex={start}&Limit={PAGE_SIZE}"
            ))?;
            let page: wire::Page = serde_json::from_str(&body).map_err(|source| {
                JellyfinError::BadResponse(self.key.scrub(&source.to_string()))
            })?;
            let got = page.items.len();
            items.extend(page.items);
            let total = items.len();
            match page.total_record_count {
                Some(expected) if (total as u64) >= expected => return Ok(items),
                Some(expected) if got == 0 => {
                    return Err(JellyfinError::BadResponse(format!(
                        "the list ended at {total} items but Jellyfin counted {expected}"
                    )));
                }
                None if got < PAGE_SIZE => return Ok(items),
                _ => {}
            }
        }
    }

    /// Fetches the playable movies and episodes and the collections.
    pub(super) fn fetch_library(&self) -> Result<FetchedLibrary, JellyfinError> {
        let mut skipped = 0;
        let mut entries: Vec<LibraryEntry> = Vec::new();
        let mut seen = HashSet::new();
        for value in self.get_items(
            "Recursive=true&IncludeItemTypes=Movie,Episode\
             &Fields=MediaStreams,Chapters,Path,MediaSources&SortBy=SortName",
        )? {
            match wire::parse_item(value) {
                Parsed::Playable(entry) => {
                    // Paging by name can repeat an item that shares its name with another.
                    if seen.insert(entry.item.id.clone()) {
                        entries.push(entry);
                    }
                }
                Parsed::Skipped | Parsed::Invalid => skipped += 1,
            }
        }

        let collections = self.fetch_collections(&entries)?;
        Ok(FetchedLibrary {
            entries,
            collections,
            skipped,
        })
    }

    /// Fetches each `BoxSet` and its children. Children that are not cached
    /// items (a series, or an item that was skipped) are left out: turning
    /// those into episodes is the channel generator's job.
    fn fetch_collections(
        &self,
        entries: &[LibraryEntry],
    ) -> Result<Vec<Collection>, JellyfinError> {
        let known: HashMap<String, &MediaId> = entries
            .iter()
            .map(|entry| (normalize_id(entry.item.id.as_str()), &entry.item.id))
            .collect();
        let mut collections = Vec::new();
        for value in self.get_items("Recursive=true&IncludeItemTypes=BoxSet&SortBy=SortName")? {
            let Ok(raw) = serde_json::from_value::<wire::RawCollection>(value) else {
                continue;
            };
            let children = self.get_items(&format!(
                "ParentId={}&IncludeItemTypes=Movie,Episode",
                encode(&raw.id)
            ))?;
            let mut members: Vec<MediaId> = Vec::new();
            for child in children {
                let Ok(child) = serde_json::from_value::<wire::RawChild>(child) else {
                    continue;
                };
                if let Some(id) = known.get(&normalize_id(&child.id))
                    && !members.contains(id)
                {
                    members.push((*id).clone());
                }
            }
            collections.push(Collection {
                name: raw.name.unwrap_or_else(|| raw.id.clone()),
                members,
            });
        }
        Ok(collections)
    }
}

impl fmt::Debug for JellyfinClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JellyfinClient")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

/// Jellyfin writes an id with or without dashes depending on the endpoint.
fn normalize_id(id: &str) -> String {
    id.chars()
        .filter(|c| *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// Percent-encodes a query value, leaving only unreserved characters as they are.
pub(super) fn encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}
