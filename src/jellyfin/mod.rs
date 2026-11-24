//! The Jellyfin sync step: copies the library from Jellyfin into the library
//! cache in the [`Store`].
//!
//! [`sync`] fetches every playable movie and episode, and every collection, and
//! replaces the cache in one transaction. Nothing else in surfcult talks to
//! Jellyfin: the `Store` is the [`MediaSource`] the rest reads, so the library
//! stays available while Jellyfin is down, and a failed sync leaves the cache
//! as it was.
//!
//! The API key comes from [`ApiKey::from_env`] and is sent only in the
//! `Authorization: MediaBrowser Token="..."` header, the one form Jellyfin 12
//! accepts. It is never put in a URL, an error message or `Debug` output.
//!
//! Out of scope here: playback URLs and turning collections, libraries, tags
//! or series into episodes for a channel. See `docs/reference/jellyfin.md`.
//!
//! [`MediaSource`]: crate::library::MediaSource
//! [`Store`]: crate::store::Store

mod client;
mod config;
mod http;
mod sync;
#[cfg(test)]
mod tests;
mod wire;

pub use client::{JellyfinClient, JellyfinError};
pub use config::{ApiKey, JellyfinConfig, KEY_ENV, KEY_FILE_ENV, KeyError};
pub use http::{HttpError, JellyfinHttp, UreqHttp};
pub use sync::{SyncReport, sync};
