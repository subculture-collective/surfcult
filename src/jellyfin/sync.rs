use super::client::{JellyfinClient, JellyfinError};
use crate::store::Store;

/// What one [`sync`] cached.
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

/// Fetches every movie and episode and every collection from Jellyfin and
/// replaces the library cache in `store` with them in one transaction, marked
/// as synced at `synced_at_ms` (Unix epoch milliseconds).
///
/// If the fetch or the write fails, the cache stays exactly as it was and the
/// error says why. The caller supplies the time; nothing here reads the clock.
///
/// [`Store`] is not `Sync`. A sync run on a blocking thread should fetch there
/// and send the result back for the thread that owns the store to write.
pub fn sync(
    client: &JellyfinClient,
    store: &mut Store,
    synced_at_ms: i64,
) -> Result<SyncReport, JellyfinError> {
    let fetched = client.fetch_library()?;
    store.replace_library(&fetched.entries, &fetched.collections, synced_at_ms)?;
    Ok(SyncReport {
        items: fetched.entries.len(),
        collections: fetched.collections.len(),
        skipped: fetched.skipped,
    })
}
