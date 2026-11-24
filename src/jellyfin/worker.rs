use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use super::client::{FetchedLibrary, JellyfinClient, JellyfinError};
use super::sync::{SyncReport, write_fetched};
use crate::store::Store;

/// One fetch's outcome, sent from the worker thread to the store's owner.
type Fetched = Result<FetchedLibrary, JellyfinError>;

/// Fetches the Jellyfin library on a background thread, so the thread that
/// owns the [`Store`] never waits on the network. The owner calls
/// [`apply`](Self::apply) to write what arrived; the worker never touches the
/// store.
///
/// The thread, named `jellyfin-sync`, fetches as soon as it starts and then
/// once per interval. Dropping the worker stops the thread and waits for it to
/// end: at once if it is between fetches, or once the fetch in progress
/// finishes (the HTTP transport bounds how long that takes).
pub struct SyncWorker {
    results: Receiver<Fetched>,
    /// Dropped to stop the thread; the thread waits on the other end.
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl SyncWorker {
    /// Starts the worker thread with `client`, fetching now and then every
    /// `every`. Fails only if the operating system cannot start a thread.
    pub fn spawn(client: JellyfinClient, every: Duration) -> std::io::Result<Self> {
        let (results_tx, results) = mpsc::channel();
        let (stop, stop_rx) = mpsc::channel::<()>();
        let thread = thread::Builder::new()
            .name("jellyfin-sync".to_string())
            .spawn(move || {
                loop {
                    if results_tx.send(client.fetch_library()).is_err() {
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
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    /// Writes what the worker has fetched since the last call, without
    /// waiting. `synced_at_ms` is recorded as the sync time, as in
    /// [`sync`](super::sync).
    ///
    /// Every pending result is taken. Each snapshot is a whole library, so
    /// only the newest successful one is written; older ones are discarded
    /// unwritten. What comes back depends on the newest pending result:
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
        let written = newest_snapshot.map(|snapshot| write_fetched(store, &snapshot, synced_at_ms));
        match (written, newest_error) {
            (Some(Err(write_error)), _) => Some(Err(write_error)),
            (_, Some(fetch_error)) => Some(Err(fetch_error)),
            (written, None) => written,
        }
    }
}

impl Drop for SyncWorker {
    /// Stops the thread and joins it. A panic on the worker thread is ignored
    /// here, so dropping the worker never panics the owner.
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
