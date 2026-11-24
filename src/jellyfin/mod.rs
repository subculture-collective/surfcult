//! The Jellyfin sync step: copies the library from Jellyfin into the library
//! cache in the [`Store`].
//!
//! [`sync`] fetches every playable movie and episode, and every collection, and
//! replaces the cache in one transaction. Nothing else in surfcult talks to
//! Jellyfin: the `Store` is the [`MediaSource`] the rest reads, so the library
//! stays available while Jellyfin is down, and a failed sync leaves the cache
//! as it was.
//!
//! [`sync`] fetches on the calling thread. [`SyncWorker`] fetches on a
//! background thread at a fixed interval instead and sends each result back;
//! the thread that owns the `Store` writes it with [`SyncWorker::apply`]. The
//! `Store` is not `Sync`, so only its owner writes to it.
//!
//! The API key comes from [`ApiKey::from_env`] and is sent only in the
//! `Authorization: MediaBrowser Token="..."` header, the one form Jellyfin 12
//! accepts. It is never put in a URL, an error message or `Debug` output.
//!
//! [`JellyfinClient::playback`] asks Jellyfin how to play one item with the
//! device profile from [`crate::library::profile`] and returns a [`Playback`]:
//! a direct-play URL or a progressive MPEG-TS transcode URL that starts at a
//! given position. [`JellyfinClient::stop_transcode`] stops that transcode.
//! Playback is never reported to Jellyfin.
//!
//! The sync records what channel sources need: each item's libraries, tags
//! and genres, an episode's series, and collections that list a series.
//! Turning a source into episodes is `library::resolve_pool`'s job. See
//! `docs/reference/jellyfin.md`.
//!
//! [`MediaSource`]: crate::library::MediaSource
//! [`Store`]: crate::store::Store

mod client;
mod config;
mod http;
mod playback;
mod sync;
#[cfg(test)]
mod tests;
mod wire;
mod worker;

pub use client::{JellyfinClient, JellyfinError};
pub use config::{ApiKey, JellyfinConfig, KEY_ENV, KEY_FILE_ENV, KeyError};
pub use http::{HttpError, JellyfinHttp, UreqHttp};
pub use playback::{DEVICE_ID, Playback, TranscodeSession};
pub use sync::{SyncReport, sync};
pub use worker::SyncWorker;
