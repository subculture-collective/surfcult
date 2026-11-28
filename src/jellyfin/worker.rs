use std::cell::Cell;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use super::client::{FetchedLibrary, JellyfinClient, JellyfinError};
use super::sync::{SyncReport, unchanged, write_fetched};
use crate::store::Store;

/// One fetch's outcome, sent from the worker thread to the store's owner,
/// with its [fingerprint](FetchedLibrary::fingerprint), worked out on the
/// worker thread.
type Fetched = Result<(FetchedLibrary, u64), JellyfinError>;

/// Fetches the Jellyfin library on a background thread, so the thread that
/// owns the [`Store`] never waits on the network. The owner calls
/// [`apply`](Self::apply) to write what arrived; the worker never touches the
/// store.
///
/// The thread, named `jellyfin-sync`, fetches as soon as it starts and then
/// once per interval. Dropping the worker tells the thread to stop and returns
/// without waiting: the thread ends at once if it is between fetches, or once
/// the fetch in progress finishes. A whole fetch can take minutes on a large
/// library, and the thread never touches the store, so nothing waits for it;
/// if the process exits first, the fetch is simply abandoned.
pub struct SyncWorker {
    results: Receiver<Fetched>,
    /// The fingerprint of the snapshot [`apply`](Self::apply) last wrote or
    /// found unchanged; until then the one the store recorded is used.
    written: Cell<Option<u64>>,
    /// Dropped with the worker to stop the thread; the thread waits on the
    /// other end.
    _stop: Sender<()>,
}

impl SyncWorker {
    /// Starts the worker thread with `client`, fetching now and then every
    /// `every`. Fails only if the operating system cannot start a thread.
    pub fn spawn(client: JellyfinClient, every: Duration) -> std::io::Result<Self> {
        let (results_tx, results) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel::<()>();
        thread::Builder::new()
            .name("jellyfin-sync".to_string())
            .spawn(move || {
                loop {
                    let fetched = client.fetch_library().map(|library| {
                        let fingerprint = library.fingerprint();
                        (library, fingerprint)
                    });
                    if results_tx.send(fetched).is_err() {
                        return;
                    }
                    match stop_rx.recv_timeout(every) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => return,
                    }
                }
            })?;
        Ok(Self {
            results,
            written: Cell::new(None),
            _stop: stop,
        })
    }

    /// Writes what the worker has fetched since the last call, without
    /// waiting. `synced_at_ms` is recorded as the sync time, as in
    /// [`sync`](super::sync).
    ///
    /// Every pending result is taken. Each snapshot is a whole library, so
    /// only the newest successful one is written; older ones are discarded
    /// unwritten. A snapshot that matches the one this worker last wrote, or
    /// before its first write the one the store recorded
    /// ([`Store::library_fingerprint`]), perhaps before a restart, is not
    /// written again: the cache already holds it, and writing tens of
    /// thousands of songs costs seconds on a Raspberry Pi. Only the sync time
    /// moves ([`Store::mark_library_synced`]), and the report says
    /// `written: false`. What comes back depends on the newest pending result:
    ///
    /// - `None` when nothing is pending. Nothing is written.
    /// - `Some(Ok(report))` when the newest result is a snapshot: the report of
    ///   writing it.
    /// - `Some(Err(error))` when the newest result is a failed fetch: that
    ///   fetch's error. A successful snapshot taken in the same call is still
    ///   written first, so the cache moves to the newest library that arrived.
    ///
    /// If writing the snapshot fails, the cache stays as it was and that write
    /// error is returned instead, whatever the newest result was.
    ///
    /// If the worker thread has died (it panicked), results it sent before
    /// dying are still applied; after that this returns `None`. It never
    /// panics.
    pub fn apply(
        &self,
        store: &mut Store,
        synced_at_ms: i64,
    ) -> Option<Result<SyncReport, JellyfinError>> {
        let mut newest_snapshot = None;
        let mut newest_error = None;
        loop {
            match self.results.try_recv() {
                Ok(Ok(snapshot)) => {
                    newest_snapshot = Some(snapshot);
                    newest_error = None;
                }
                Ok(Err(error)) => newest_error = Some(error),
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        let written = newest_snapshot.map(|(snapshot, fingerprint)| {
            // Until this worker writes, the last write is the one the store
            // recorded, perhaps before a restart.
            let last = match self.written.get() {
                Some(last) => Some(last),
                None => store.library_fingerprint()?,
            };
            if last == Some(fingerprint) {
                store.mark_library_synced(synced_at_ms)?;
                self.written.set(Some(fingerprint));
                return Ok(unchanged(&snapshot));
            }
            let report = write_fetched(store, &snapshot, synced_at_ms)?;
            store.record_library_fingerprint(fingerprint)?;
            self.written.set(Some(fingerprint));
            Ok(report)
        });
        match (written, newest_error) {
            (Some(Err(write_error)), _) => Some(Err(write_error)),
            (_, Some(fetch_error)) => Some(Err(fetch_error)),
            (written, None) => written,
        }
    }
}
