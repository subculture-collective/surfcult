//! Jellyfin as a media source, read through a local SQLite cache.
//!
//! [`JellyfinSource::sync`] fetches every playable movie and episode, and every
//! collection, and replaces the cache in one transaction. The [`MediaSource`]
//! methods read only the cache, so the library stays available while Jellyfin
//! is down; a failed sync leaves the cache as it was.
//!
//! The API key comes from [`ApiKey::from_env`] and is sent only in the
//! `Authorization: MediaBrowser Token="..."` header, the one form Jellyfin 12
//! accepts. It is never put in a URL, an error message or `Debug` output.
//!
//! Out of scope here: playback URLs and turning collections, libraries, tags
//! or series into episodes for a channel. See `docs/reference/jellyfin.md`.
//!
//! [`MediaSource`]: crate::library::MediaSource

mod client;
mod config;
mod http;
mod source;
#[cfg(test)]
mod tests;
mod wire;

pub use client::{JellyfinClient, JellyfinError};
pub use config::{ApiKey, JellyfinConfig, KEY_ENV, KEY_FILE_ENV, KeyError};
pub use http::{HttpError, JellyfinHttp, UreqHttp};
pub use source::{JellyfinSource, SyncReport};
