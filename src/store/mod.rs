//! SQLite storage for generated schedule state.
//!
//! `channels.toml` stays the source of truth for channels. This database holds
//! what the program derives from it and what happens at run time:
//!
//! - `events`: each channel's materialized schedule, one row per
//!   [`timeline::Event`]. The primary key `(channel, start_ms)` doubles as the
//!   index for reading a time range. Events of one channel must not overlap;
//!   the store checks that each event ends after it starts, not that they
//!   never overlap, because that is the generator's job.
//!   A sequential channel's position is read from its last program event, so
//!   there is no separate cursor; migration 3 dropped the old `cursors` table.
//! - `playback_history`: one row each time the tuner starts something, with a
//!   short outcome word. Read newest first.
//!
//! - `library_*`: a copy of the Jellyfin library (items, chapters, streams,
//!   libraries, tags, genres, series and collections), replaced as a whole by
//!   each sync. See [`LibraryEntry`].
//!
//! Times are Unix epoch milliseconds (`i64`), as in [`crate::timeline`].
//!
//! # Migrations
//!
//! The schema lives in numbered SQL files in `migrations/`, compiled into the
//! binary with `include_str!`. [`Store::open`] applies the ones a database has
//! not seen, in order, each inside a transaction, and records them in
//! `schema_migrations`. To change the schema, add the next numbered file and
//! list it in `migrate.rs`. Never edit a migration that has shipped; add a new
//! one instead.

mod error;
mod library;
mod migrate;
#[cfg(test)]
mod tests;

use std::path::Path;
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension, params};

use crate::library::MediaId;
use crate::timeline::{Event, EventKind};

pub use error::StoreError;
pub use library::LibraryEntry;

/// The query behind [`Store::last_program_before`]; a test checks its plan.
const LAST_PROGRAM_BEFORE: &str = "SELECT media_id FROM events
     WHERE channel = ?1 AND kind = 'program' AND start_ms < ?2
     ORDER BY start_ms DESC
     LIMIT 1";

/// The query behind [`Store::recent_programs_before`]; a test checks its plan.
const RECENT_PROGRAMS_BEFORE: &str = "SELECT start_ms, media_id FROM events
     WHERE channel = ?1 AND kind = 'program' AND start_ms < ?2
     ORDER BY start_ms DESC
     LIMIT ?3";

/// The query behind [`Store::breaks_between`]; a test checks its plan.
const BREAKS_BETWEEN: &str = "SELECT start_ms, media_id FROM events
     WHERE channel = ?1 AND kind = 'break' AND start_ms >= ?2 AND start_ms < ?3
     ORDER BY start_ms DESC";

/// How long a call waits for another connection's lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// One row of playback history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaybackRecord {
    pub channel: u32,
    pub media_id: MediaId,
    pub started_at_ms: i64,
    pub outcome: String,
}

/// A SQLite database of schedule events, playback history and the library cache.
///
/// Reads take `&self`; anything that writes takes `&mut self`.
pub struct Store {
    conn: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it if needed, and migrates it to
    /// the newest schema. Uses write-ahead logging.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let conn = Connection::open(path).map_err(StoreError::database("opening the database"))?;
        Self::init(conn, true)
    }

    /// Opens the existing database at `path` for reading only. Unlike
    /// [`Store::open`] it does not create the file, change the journal mode
    /// or migrate, so a missing file is an error and the schema stays at
    /// whatever version the writer left it. Every write through it fails.
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, StoreError> {
        let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
        let conn = Connection::open_with_flags(path, flags)
            .map_err(StoreError::database("opening the database"))?;
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(StoreError::database("configuring the database"))?;
        Ok(Self { conn })
    }

    /// An empty database that lives in memory. For tests.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn =
            Connection::open_in_memory().map_err(StoreError::database("opening the database"))?;
        Self::init(conn, false)
    }

    fn init(mut conn: Connection, wal: bool) -> Result<Self, StoreError> {
        let action = "configuring the database";
        conn.busy_timeout(BUSY_TIMEOUT)
            .map_err(StoreError::database(action))?;
        conn.pragma_update(None, "foreign_keys", true)
            .map_err(StoreError::database(action))?;
        if wal {
            conn.pragma_update_and_check(None, "journal_mode", "WAL", |_| Ok(()))
                .map_err(StoreError::database(action))?;
        }
        migrate::run(&mut conn)?;
        Ok(Self { conn })
    }

    /// Adds events for `channel`. All of them are written or none are. An
    /// event whose start time the channel already has is an error.
    pub fn write_events(&mut self, channel: u32, events: &[Event]) -> Result<(), StoreError> {
        validate(events)?;
        let tx = self
            .conn
            .transaction()
            .map_err(StoreError::database("writing events"))?;
        insert_events(&tx, channel, events)?;
        tx.commit().map_err(StoreError::database("writing events"))
    }

    /// Events of `channel` that overlap `[from_ms, to_ms)`, sorted by start. An
    /// event that began before `from_ms` but is still on at `from_ms` is
    /// included; one that ends exactly at `from_ms` is not.
    pub fn events_between(
        &self,
        channel: u32,
        from_ms: i64,
        to_ms: i64,
    ) -> Result<Vec<Event>, StoreError> {
        if from_ms >= to_ms {
            return Ok(Vec::new());
        }
        let action = "reading events";
        // Events do not overlap, so only the last event starting at or before
        // `from_ms` can reach into the range from the left. Starting the scan
        // there keeps it on the primary key instead of reading old events.
        let mut statement = self
            .conn
            .prepare_cached(
                "SELECT start_ms, end_ms, media_id, media_offset_ms, media_duration_ms, kind
                 FROM events
                 WHERE channel = ?1
                   AND start_ms >= COALESCE(
                       (SELECT MAX(start_ms) FROM events WHERE channel = ?1 AND start_ms <= ?2),
                       ?2)
                   AND start_ms < ?3
                   AND end_ms > ?2
                 ORDER BY start_ms",
            )
            .map_err(StoreError::database(action))?;
        let rows = statement
            .query_map(params![channel, from_ms, to_ms], read_event)
            .map_err(StoreError::database(action))?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row.map_err(StoreError::database(action))??);
        }
        Ok(events)
    }

    /// In one transaction, deletes `channel`'s events that start at or after
    /// `from_ms` and writes `events` in their place. Earlier events are not
    /// touched. Every new event must start at or after `from_ms`.
    pub fn replace_events_from(
        &mut self,
        channel: u32,
        from_ms: i64,
        events: &[Event],
    ) -> Result<(), StoreError> {
        validate(events)?;
        if let Some(early) = events.iter().find(|event| event.start_ms < from_ms) {
            return Err(StoreError::InvalidEvent(format!(
                "event starting at {} is before the replace point {from_ms}",
                early.start_ms
            )));
        }
        let action = "replacing events";
        let tx = self
            .conn
            .transaction()
            .map_err(StoreError::database(action))?;
        tx.execute(
            "DELETE FROM events WHERE channel = ?1 AND start_ms >= ?2",
            params![channel, from_ms],
        )
        .map_err(StoreError::database(action))?;
        insert_events(&tx, channel, events)?;
        tx.commit().map_err(StoreError::database(action))
    }

    /// Start times and media ids of the channel's last `limit` program events
    /// that start before `before_ms`, newest first. Break, filler and ident
    /// events are left out; a program cut into several events appears once per
    /// event. Walks back from `before_ms` on the primary key, so its cost grows
    /// with `limit`, not with the stored history. To read further back, call it
    /// again with the oldest start time it returned.
    pub fn recent_programs_before(
        &self,
        channel: u32,
        before_ms: i64,
        limit: u32,
    ) -> Result<Vec<(i64, MediaId)>, StoreError> {
        let action = "reading recent programs";
        let mut statement = self
            .conn
            .prepare_cached(RECENT_PROGRAMS_BEFORE)
            .map_err(StoreError::database(action))?;
        let rows = statement
            .query_map(params![channel, before_ms, limit], |row| {
                Ok((row.get(0)?, MediaId::new(row.get::<_, String>(1)?)))
            })
            .map_err(StoreError::database(action))?;
        rows.collect::<Result<_, _>>()
            .map_err(StoreError::database(action))
    }

    /// The media id of the channel's last program event that starts before
    /// `before_ms`, or `None` if it has none. Reads one row, walking back from
    /// `before_ms` on the primary key, so its cost does not grow with history.
    pub fn last_program_before(
        &self,
        channel: u32,
        before_ms: i64,
    ) -> Result<Option<MediaId>, StoreError> {
        self.conn
            .prepare_cached(LAST_PROGRAM_BEFORE)
            .and_then(|mut statement| {
                statement
                    .query_row(params![channel, before_ms], |row| {
                        row.get::<_, String>(0).map(MediaId::new)
                    })
                    .optional()
            })
            .map_err(StoreError::database("reading the last program"))
    }

    /// The start time and media id of each of the channel's break events
    /// that start in `[since_ms, before_ms)`, newest first: the clips a break
    /// pool's rotation continues after and its repeat limit counts from.
    /// Walks back from `before_ms` on the primary key and stops at
    /// `since_ms`, so the cost is one window of events, not the channel's
    /// whole history.
    pub fn breaks_between(
        &self,
        channel: u32,
        since_ms: i64,
        before_ms: i64,
    ) -> Result<Vec<(i64, MediaId)>, StoreError> {
        let action = "reading recent breaks";
        let mut statement = self
            .conn
            .prepare_cached(BREAKS_BETWEEN)
            .map_err(StoreError::database(action))?;
        let rows = statement
            .query_map(params![channel, since_ms, before_ms], |row| {
                Ok((row.get(0)?, MediaId::new(row.get::<_, String>(1)?)))
            })
            .map_err(StoreError::database(action))?;
        rows.collect::<Result<_, _>>()
            .map_err(StoreError::database(action))
    }

    /// The channel's last event that starts before `before_ms`, or `None` if
    /// it has none. Reads one row, walking back on the primary key.
    pub fn last_event_before(
        &self,
        channel: u32,
        before_ms: i64,
    ) -> Result<Option<Event>, StoreError> {
        let action = "reading the last event";
        let event = self
            .conn
            .prepare_cached(
                "SELECT start_ms, end_ms, media_id, media_offset_ms, media_duration_ms, kind
                 FROM events
                 WHERE channel = ?1 AND start_ms < ?2
                 ORDER BY start_ms DESC
                 LIMIT 1",
            )
            .and_then(|mut statement| {
                statement
                    .query_row(params![channel, before_ms], read_event)
                    .optional()
            })
            .map_err(StoreError::database(action))?;
        event.transpose()
    }

    /// When the channel's last stored event ends, or `None` if it has none.
    /// Events do not overlap, so this is the end of the event that starts
    /// last; reading it walks one row of the primary key.
    pub fn schedule_end_ms(&self, channel: u32) -> Result<Option<i64>, StoreError> {
        self.conn
            .prepare_cached(
                "SELECT end_ms FROM events WHERE channel = ?1 ORDER BY start_ms DESC LIMIT 1",
            )
            .and_then(|mut statement| {
                statement
                    .query_row(params![channel], |row| row.get(0))
                    .optional()
            })
            .map_err(StoreError::database("reading the schedule end"))
    }

    /// The newest migration applied to this database.
    pub fn schema_version(&self) -> Result<u32, StoreError> {
        self.conn
            .query_row(
                "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
                [],
                |row| row.get(0),
            )
            .map_err(StoreError::database("reading the schema version"))
    }

    /// The schema version this build migrates to and expects to read.
    pub fn supported_schema_version() -> u32 {
        migrate::latest_version()
    }

    /// Adds one row to the playback history.
    pub fn record_playback(
        &mut self,
        channel: u32,
        media_id: &MediaId,
        started_at_ms: i64,
        outcome: &str,
    ) -> Result<(), StoreError> {
        self.conn
            .execute(
                "INSERT INTO playback_history (channel, media_id, started_at_ms, outcome)
                 VALUES (?1, ?2, ?3, ?4)",
                params![channel, media_id.as_str(), started_at_ms, outcome],
            )
            .map_err(StoreError::database("recording playback"))?;
        Ok(())
    }

    /// The latest `limit` history rows of `channel`, newest first. Rows that
    /// started at the same time come back in the order they were recorded,
    /// newest first.
    pub fn playback_history(
        &self,
        channel: u32,
        limit: u32,
    ) -> Result<Vec<PlaybackRecord>, StoreError> {
        let action = "reading playback history";
        let mut statement = self
            .conn
            .prepare_cached(
                "SELECT channel, media_id, started_at_ms, outcome FROM playback_history
                 WHERE channel = ?1
                 ORDER BY started_at_ms DESC, id DESC
                 LIMIT ?2",
            )
            .map_err(StoreError::database(action))?;
        let rows = statement
            .query_map(params![channel, limit], |row| {
                Ok(PlaybackRecord {
                    channel: row.get(0)?,
                    media_id: MediaId::new(row.get::<_, String>(1)?),
                    started_at_ms: row.get(2)?,
                    outcome: row.get(3)?,
                })
            })
            .map_err(StoreError::database(action))?;
        rows.collect::<Result<_, _>>()
            .map_err(StoreError::database(action))
    }
}

/// Checks events before anything is written, so a bad one fails the whole call.
fn validate(events: &[Event]) -> Result<(), StoreError> {
    for event in events {
        if event.end_ms <= event.start_ms {
            return Err(StoreError::InvalidEvent(format!(
                "event for {} ends at {} which is not after its start at {}",
                event.media_id, event.end_ms, event.start_ms
            )));
        }
        for (name, value) in [
            ("media_offset_ms", event.media_offset_ms),
            ("media_duration_ms", event.media_duration_ms),
        ] {
            if i64::try_from(value).is_err() {
                return Err(StoreError::InvalidEvent(format!(
                    "{name} {value} is too large to store"
                )));
            }
        }
    }
    Ok(())
}

fn insert_events(
    tx: &rusqlite::Transaction<'_>,
    channel: u32,
    events: &[Event],
) -> Result<(), StoreError> {
    let action = "inserting events";
    let mut statement = tx
        .prepare_cached(
            "INSERT INTO events
               (channel, start_ms, end_ms, media_id, media_offset_ms, media_duration_ms, kind)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        )
        .map_err(StoreError::database(action))?;
    for event in events {
        // `validate` has already checked that both values fit in an i64.
        statement
            .execute(params![
                channel,
                event.start_ms,
                event.end_ms,
                event.media_id.as_str(),
                event.media_offset_ms as i64,
                event.media_duration_ms as i64,
                kind_name(event.kind),
            ])
            .map_err(StoreError::database(action))?;
    }
    Ok(())
}

fn kind_name(kind: EventKind) -> &'static str {
    match kind {
        EventKind::Program => "program",
        EventKind::Break => "break",
        EventKind::Filler => "filler",
        EventKind::Ident => "ident",
    }
}

fn kind_from_name(name: &str) -> Result<EventKind, StoreError> {
    match name {
        "program" => Ok(EventKind::Program),
        "break" => Ok(EventKind::Break),
        "filler" => Ok(EventKind::Filler),
        "ident" => Ok(EventKind::Ident),
        other => Err(StoreError::Corrupt(format!("unknown event kind {other:?}"))),
    }
}

/// Reads one `events` row. The inner `Result` carries values that SQLite
/// returned but that do not fit the Rust type.
fn read_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<Result<Event, StoreError>> {
    let start_ms = row.get(0)?;
    let end_ms = row.get(1)?;
    let media_id = row.get(2)?;
    let media_offset_ms: i64 = row.get(3)?;
    let media_duration_ms: i64 = row.get(4)?;
    let kind: String = row.get(5)?;
    Ok(build_event(
        start_ms,
        end_ms,
        media_id,
        media_offset_ms,
        media_duration_ms,
        &kind,
    ))
}

fn build_event(
    start_ms: i64,
    end_ms: i64,
    media_id: String,
    media_offset_ms: i64,
    media_duration_ms: i64,
    kind: &str,
) -> Result<Event, StoreError> {
    let unsigned = |name: &str, value: i64| {
        u64::try_from(value).map_err(|_| StoreError::Corrupt(format!("negative {name}")))
    };
    Ok(Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media_id),
        media_offset_ms: unsigned("media_offset_ms", media_offset_ms)?,
        media_duration_ms: unsigned("media_duration_ms", media_duration_ms)?,
        kind: kind_from_name(kind)?,
    })
}
