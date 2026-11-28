use super::client::{FetchedLibrary, JellyfinClient, JellyfinError};
use crate::library::LibraryChanges;
use crate::store::Store;

/// What one [`sync`] cached.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncReport {
    /// Items now in the cache.
    pub items: usize,
    /// Collections now in the cache.
    pub collections: usize,
    /// Items Jellyfin listed that were left out: placeholders for missing
    /// episodes, items without a runtime, and items with an unreadable shape.
    pub skipped: usize,
    /// How the cached items changed since the cache this sync replaced:
    /// renamed (a new id for the same file path), resized and vanished items.
    pub changes: LibraryChanges,
    /// Whether the cache was replaced. `false` when
    /// [`SyncWorker::apply`](super::SyncWorker::apply) found the library just
    /// as it last wrote it and left the cache alone; [`sync`] always writes.
    pub written: bool,
}

/// Fetches every movie, episode, music video and song and every collection
/// from Jellyfin and
/// replaces the library cache in `store` with them in one transaction, marked
/// as synced at `synced_at_ms` (Unix epoch milliseconds).
///
/// If the fetch or the write fails, the cache stays exactly as it was and the
/// error says why. The caller supplies the time; nothing here reads the clock.
///
/// This fetches on the calling thread. To keep the fetch off the thread that
/// owns the store, use [`SyncWorker`](super::SyncWorker), which fetches on its
/// own thread and leaves the write to [`SyncWorker::apply`](super::SyncWorker::apply).
pub fn sync(
    client: &JellyfinClient,
    store: &mut Store,
    synced_at_ms: i64,
) -> Result<SyncReport, JellyfinError> {
    let fetched = client.fetch_library()?;
    write_fetched(store, &fetched, synced_at_ms)
}

/// Replaces the library cache with one fetched snapshot and reports what it
/// holds. [`sync`] and [`SyncWorker::apply`](super::SyncWorker::apply) both
/// write through this.
pub(super) fn write_fetched(
    store: &mut Store,
    fetched: &FetchedLibrary,
    synced_at_ms: i64,
) -> Result<SyncReport, JellyfinError> {
    let changes = store.replace_library(&fetched.entries, &fetched.collections, synced_at_ms)?;
    Ok(SyncReport {
        items: fetched.entries.len(),
        collections: fetched.collections.len(),
        skipped: fetched.skipped,
        changes,
        written: true,
    })
}

/// What [`SyncWorker::apply`](super::SyncWorker::apply) reports for a
/// snapshot it did not write because it matched the cache.
pub(super) fn unchanged(fetched: &FetchedLibrary) -> SyncReport {
    SyncReport {
        items: fetched.entries.len(),
        collections: fetched.collections.len(),
        skipped: fetched.skipped,
        changes: LibraryChanges::default(),
        written: false,
    }
}
