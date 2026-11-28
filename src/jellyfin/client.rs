use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use serde_json::Value;

use super::config::{ApiKey, JellyfinConfig};
use super::http::{HttpError, JellyfinHttp, UreqHttp};
use super::wire::{self, Parsed};
use crate::library::{Collection, MediaId, MediaKind};
use crate::store::{LibraryEntry, StoreError};

/// Items asked for per request.
const PAGE_SIZE: usize = 500;

/// The Jellyfin video types the sync caches, as `IncludeItemTypes` lists them.
/// `wire::parse_item` maps each one to a `MediaKind`. Songs (`Audio`) are
/// fetched apart, from each music library ([`JellyfinClient::add_songs`]).
const PLAYABLE_TYPES: &str = "Movie,Episode,MusicVideo";

/// The library type (`CollectionType`) whose songs the sync caches.
const MUSIC_LIBRARY: &str = "music";

/// What a song request asks for. Artists, album and album id come without
/// asking; images and user data are left out to keep 32,000 songs small.
const SONG_QUERY: &str = "Recursive=true&IncludeItemTypes=Audio\
     &Fields=Path,Genres,MediaStreams&EnableImages=false&EnableUserData=false\
     &SortBy=AlbumArtist,Album,ParentIndexNumber,IndexNumber,SortName";

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

impl FetchedLibrary {
    /// A hash of the items and collections, to tell whether two fetches
    /// found the same library without keeping both.
    pub fn fingerprint(&self) -> u64 {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        self.entries.hash(&mut hasher);
        self.collections.hash(&mut hasher);
        hasher.finish()
    }
}

/// A read-only Jellyfin client. It holds the API key and sends it only in the
/// `Authorization` header, never in a URL.
pub struct JellyfinClient {
    http: Box<dyn JellyfinHttp>,
    key: ApiKey,
    /// The user `PlaybackInfo` is requested for; looked up once if not given.
    user_id: Mutex<Option<String>>,
}

impl JellyfinClient {
    /// A client that talks to `config.base_url` over HTTP(S).
    pub fn new(config: JellyfinConfig) -> Self {
        let http = UreqHttp::new(&config.base_url);
        let client = Self::with_transport(config.key, http);
        match config.user_id {
            Some(id) => client.with_user_id(id),
            None => client,
        }
    }

    /// This client with playback requested for the user `id`, instead of the
    /// first enabled user.
    pub fn with_user_id(self, id: impl Into<String>) -> Self {
        *self.user_id.lock().unwrap_or_else(|e| e.into_inner()) = Some(id.into());
        self
    }

    /// A client that sends its requests through `http`. Tests use this with
    /// recorded responses.
    pub fn with_transport(key: ApiKey, http: impl JellyfinHttp + 'static) -> Self {
        Self {
            http: Box::new(http),
            key,
            user_id: Mutex::new(None),
        }
    }

    /// The user playback is requested for: the one given in the config, or
    /// else the first enabled user Jellyfin lists, found once and kept.
    pub(super) fn user_id(&self) -> Result<String, JellyfinError> {
        let mut cached = self.user_id.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(id) = cached.as_ref() {
            return Ok(id.clone());
        }
        let body = self.get("/Users")?;
        let users: Vec<Value> = serde_json::from_str(&body)
            .map_err(|source| JellyfinError::BadResponse(self.scrub(&source.to_string())))?;
        let id = users
            .iter()
            .find(|user| user["Policy"]["IsDisabled"] != true)
            .and_then(|user| user["Id"].as_str())
            .ok_or_else(|| {
                JellyfinError::NoPlayback("Jellyfin lists no enabled user to play as".into())
            })?
            .to_string();
        *cached = Some(id.clone());
        Ok(id)
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

    /// The primary image of the item `id` as a PNG at most `side` pixels
    /// wide and tall: for a song's album id, the album cover. Jellyfin
    /// converts and scales it.
    pub fn primary_image(&self, id: &MediaId, side: u32) -> Result<Vec<u8>, JellyfinError> {
        self.http
            .get_bytes(
                &format!(
                    "/Items/{}/Images/Primary?format=Png&maxWidth={side}&maxHeight={side}",
                    encode(id.as_str())
                ),
                &self.key.authorization_header(),
            )
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

    /// Reads every page of `{path}?{query}` and returns the raw items.
    fn get_list(&self, path: &str, query: &str) -> Result<Vec<Value>, JellyfinError> {
        let mut items = Vec::new();
        self.for_each_page(path, query, |page| items.extend(page))?;
        Ok(items)
    }

    /// Reads every page of `{path}?{query}` and hands each page's raw items
    /// to `each` before reading the next, so only one page is held as JSON.
    fn for_each_page(
        &self,
        path: &str,
        query: &str,
        mut each: impl FnMut(Vec<Value>),
    ) -> Result<(), JellyfinError> {
        let mut total = 0;
        loop {
            let start = total;
            let body = self.get(&format!(
                "{path}?{query}&StartIndex={start}&Limit={PAGE_SIZE}"
            ))?;
            let page: wire::Page = serde_json::from_str(&body).map_err(|source| {
                JellyfinError::BadResponse(self.key.scrub(&source.to_string()))
            })?;
            drop(body);
            let got = page.items.len();
            total += got;
            each(page.items);
            match page.total_record_count {
                Some(expected) if (total as u64) >= expected => return Ok(()),
                Some(expected) if got == 0 => {
                    return Err(JellyfinError::BadResponse(format!(
                        "the list ended at {total} items but Jellyfin counted {expected}"
                    )));
                }
                None if got < PAGE_SIZE => return Ok(()),
                _ => {}
            }
        }
    }

    /// Reads every page of `/Items?{query}` and returns the raw items.
    fn get_items(&self, query: &str) -> Result<Vec<Value>, JellyfinError> {
        self.get_list("/Items", query)
    }

    /// Fetches the playable movies, episodes and music videos with their
    /// series, artists, tags, genres and libraries, the songs of every music
    /// library, and the collections.
    pub(super) fn fetch_library(&self) -> Result<FetchedLibrary, JellyfinError> {
        let mut skipped = 0;
        let mut entries: Vec<LibraryEntry> = Vec::new();
        let mut seen = HashSet::new();
        for value in self.get_items(&format!(
            "Recursive=true&IncludeItemTypes={PLAYABLE_TYPES}\
             &Fields=MediaStreams,Chapters,Path,MediaSources,Tags,Genres&SortBy=SortName"
        ))? {
            match wire::parse_item(value) {
                Parsed::Playable(entry) => {
                    // Paging by name can repeat an item that shares its name with another.
                    if seen.insert(entry.item.id.clone()) {
                        entries.push(*entry);
                    }
                }
                Parsed::Skipped | Parsed::Invalid => skipped += 1,
            }
        }

        self.add_series_labels(&mut entries)?;
        let music = self.add_libraries(&mut entries)?;
        for (id, name) in &music {
            self.add_songs(id, name, &mut entries, &mut skipped)?;
        }
        let collections = self.fetch_collections(&entries)?;
        Ok(FetchedLibrary {
            entries,
            collections,
            skipped,
        })
    }

    /// Fetches every series and adds its tags and genres to its episodes,
    /// after the episode's own and without repeating a name.
    fn add_series_labels(&self, entries: &mut [LibraryEntry]) -> Result<(), JellyfinError> {
        let mut series = HashMap::new();
        for value in self.get_items(
            "Recursive=true&IncludeItemTypes=Series&Fields=Tags,Genres&SortBy=SortName",
        )? {
            let Ok(raw) = serde_json::from_value::<wire::RawSeries>(value) else {
                continue;
            };
            series.insert(
                normalize_id(&raw.id),
                (wire::names(raw.tags), wire::names(raw.genres)),
            );
        }
        for entry in entries {
            let item = &mut entry.item;
            let Some(episode) = &item.series else {
                continue;
            };
            let Some((tags, genres)) = series.get(&normalize_id(episode.series_id.as_str())) else {
                continue;
            };
            extend_unique(&mut item.tags, tags);
            extend_unique(&mut item.genres, genres);
        }
        Ok(())
    }

    /// Fetches each library (Jellyfin's media folders) and the playable
    /// videos in it, and adds the library's name to those items. Returns the
    /// id and name of each music library, whose songs [`Self::add_songs`]
    /// fetches.
    fn add_libraries(
        &self,
        entries: &mut [LibraryEntry],
    ) -> Result<Vec<(String, String)>, JellyfinError> {
        let mut music = Vec::new();
        let index: HashMap<String, usize> = entries
            .iter()
            .enumerate()
            .map(|(i, entry)| (normalize_id(entry.item.id.as_str()), i))
            .collect();
        for value in self.get_list("/Library/MediaFolders", "IsHidden=false")? {
            let Ok(raw) = serde_json::from_value::<wire::RawCollection>(value) else {
                continue;
            };
            let name = raw.name.unwrap_or_else(|| raw.id.clone());
            if raw.collection_type.as_deref() == Some(MUSIC_LIBRARY) {
                music.push((raw.id.clone(), name.clone()));
            }
            let children = self.get_items(&format!(
                "ParentId={}&Recursive=true&IncludeItemTypes={PLAYABLE_TYPES}&SortBy=SortName",
                encode(&raw.id)
            ))?;
            for child in children {
                let Ok(child) = serde_json::from_value::<wire::RawChild>(child) else {
                    continue;
                };
                if let Some(&i) = index.get(&normalize_id(&child.id)) {
                    extend_unique(&mut entries[i].item.libraries, std::slice::from_ref(&name));
                }
            }
        }
        Ok(music)
    }

    /// Fetches the songs of the music library `id`, named `name`, page by
    /// page, and appends them to `entries` with the library's name. A song
    /// already listed, by an earlier page or another music library, gets
    /// the name added instead.
    fn add_songs(
        &self,
        id: &str,
        name: &str,
        entries: &mut Vec<LibraryEntry>,
        skipped: &mut usize,
    ) -> Result<(), JellyfinError> {
        let mut index: HashMap<String, usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.item.kind == MediaKind::Song)
            .map(|(i, entry)| (normalize_id(entry.item.id.as_str()), i))
            .collect();
        let library = [name.to_string()];
        self.for_each_page(
            "/Items",
            &format!("ParentId={}&{SONG_QUERY}", encode(id)),
            |page| {
                for value in page {
                    let Parsed::Playable(entry) = wire::parse_item(value) else {
                        *skipped += 1;
                        continue;
                    };
                    let mut entry = *entry;
                    if entry.item.kind != MediaKind::Song {
                        *skipped += 1;
                        continue;
                    }
                    let key = normalize_id(entry.item.id.as_str());
                    if let Some(&i) = index.get(&key) {
                        extend_unique(&mut entries[i].item.libraries, &library);
                        continue;
                    }
                    entry.item.libraries = library.to_vec();
                    index.insert(key, entries.len());
                    entries.push(entry);
                }
            },
        )
    }

    /// Fetches each `BoxSet` and its children. A child that is a cached item
    /// is kept by its id, and a series with a cached episode by its series id;
    /// `library::resolve_pool` turns the series into its episodes. Other
    /// children (an item that was skipped, a series with no cached episode)
    /// are left out.
    fn fetch_collections(
        &self,
        entries: &[LibraryEntry],
    ) -> Result<Vec<Collection>, JellyfinError> {
        let mut known: HashMap<String, &MediaId> = HashMap::new();
        for entry in entries {
            if let Some(episode) = &entry.item.series {
                known.insert(normalize_id(episode.series_id.as_str()), &episode.series_id);
            }
        }
        for entry in entries {
            known.insert(normalize_id(entry.item.id.as_str()), &entry.item.id);
        }
        let mut collections = Vec::new();
        for value in self.get_items("Recursive=true&IncludeItemTypes=BoxSet&SortBy=SortName")? {
            let Ok(raw) = serde_json::from_value::<wire::RawCollection>(value) else {
                continue;
            };
            let children = self.get_items(&format!(
                "ParentId={}&IncludeItemTypes={PLAYABLE_TYPES},Series",
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

/// Appends the names in `more` that `list` does not have yet.
fn extend_unique(list: &mut Vec<String>, more: &[String]) {
    for name in more {
        if !list.contains(name) {
            list.push(name.clone());
        }
    }
}
