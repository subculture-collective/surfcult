use std::fmt;
use std::sync::{Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use super::client::{JellyfinClient, JellyfinError};
use crate::library::{Collection, MediaId, MediaItem, MediaSource, SourceError};
use crate::store::{LibraryEntry, Store, StoreError};

/// What one [`JellyfinSource::sync`] cached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncReport {
    /// Items now in the cache.
    pub items: usize,
    /// Collections now in the cache.
    pub collections: usize,
    /// Items Jellyfin listed that were left out: placeholders for missing
    /// episodes, items without a runtime, and items with an unreadable shape.
    pub skipped: usize,
}

/// A [`MediaSource`] backed by Jellyfin and a SQLite cache.
///
/// [`Self::sync`] copies the library from Jellyfin into the cache. Every read
/// goes to the cache, never to Jellyfin, so reads keep working, with the data
/// of the last good sync, when Jellyfin is down.
pub struct JellyfinSource {
    client: JellyfinClient,
    store: Mutex<Store>,
}

impl JellyfinSource {
    pub fn new(client: JellyfinClient, store: Store) -> Self {
        Self {
            client,
            store: Mutex::new(store),
        }
    }

    /// Fetches every movie and episode and every collection from Jellyfin and
    /// replaces the cache with them in one transaction. If anything fails, the
    /// cache stays exactly as it was and the error says why.
    pub fn sync(&self) -> Result<SyncReport, JellyfinError> {
        self.sync_at(now_ms())
    }

    fn sync_at(&self, synced_at_ms: i64) -> Result<SyncReport, JellyfinError> {
        // Fetch before taking the lock, so reads are not held up by the network.
        let fetched = self.client.fetch_library()?;
        self.store()
            .replace_library(&fetched.entries, &fetched.collections, synced_at_ms)?;
        Ok(SyncReport {
            items: fetched.entries.len(),
            collections: fetched.collections.len(),
            skipped: fetched.skipped,
        })
    }

    /// The cached items with the file path Jellyfin reported for each.
    pub fn cached_entries(&self) -> Result<Vec<LibraryEntry>, SourceError> {
        self.store().library_items().map_err(unavailable)
    }

    /// When the cache was last filled, in Unix epoch milliseconds, or `None`
    /// if no sync has succeeded yet.
    pub fn last_synced_at_ms(&self) -> Result<Option<i64>, SourceError> {
        self.store().library_synced_at_ms().map_err(unavailable)
    }

    fn store(&self) -> MutexGuard<'_, Store> {
        // A panic while holding the lock cannot leave the database half-written.
        self.store.lock().unwrap_or_else(|e| e.into_inner())
    }
}

impl MediaSource for JellyfinSource {
    fn list_items(&self) -> Result<Vec<MediaItem>, SourceError> {
        let entries = self.cached_entries()?;
        Ok(entries.into_iter().map(|entry| entry.item).collect())
    }

    fn resolve(&self, id: &MediaId) -> Result<MediaItem, SourceError> {
        match self.store().library_item(id).map_err(unavailable)? {
            Some(entry) => Ok(entry.item),
            None => Err(SourceError::NotFound(id.clone())),
        }
    }

    fn list_collections(&self) -> Result<Vec<Collection>, SourceError> {
        self.store().library_collections().map_err(unavailable)
    }
}

impl fmt::Debug for JellyfinSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("JellyfinSource")
            .field("client", &self.client)
            .finish_non_exhaustive()
    }
}

fn unavailable(source: StoreError) -> SourceError {
    SourceError::Unavailable(format!("library cache: {source}"))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}
