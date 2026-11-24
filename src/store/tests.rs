use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::*;

/// A database path in the temp directory that is removed, with its WAL files,
/// when the test ends.
struct TempDb(PathBuf);

impl TempDb {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        Self(
            std::env::temp_dir().join(format!("surfcult-store-test-{}-{n}.db", std::process::id())),
        )
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        for suffix in ["", "-wal", "-shm"] {
            let mut name = self.0.clone().into_os_string();
            name.push(suffix);
            let _ = std::fs::remove_file(name);
        }
    }
}

fn ids(names: &[&str]) -> Vec<MediaId> {
    names.iter().map(|name| MediaId::new(*name)).collect()
}

fn event(start_ms: i64, end_ms: i64, media: &str, kind: EventKind) -> Event {
    Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media),
        media_offset_ms: 0,
        media_duration_ms: 1_320_000,
        kind,
    }
}

/// `a` 0..10s, a break 10..12s, `a` again 12..20s from offset 10s, `b` 30..40s.
fn schedule() -> Vec<Event> {
    let mut resumed = event(12_000, 20_000, "a", EventKind::Program);
    resumed.media_offset_ms = 10_000;
    vec![
        event(0, 10_000, "a", EventKind::Program),
        event(10_000, 12_000, "ad", EventKind::Break),
        resumed,
        event(30_000, 40_000, "b", EventKind::Filler),
    ]
}

fn starts(events: &[Event]) -> Vec<i64> {
    events.iter().map(|e| e.start_ms).collect()
}

#[test]
fn migrations_run_on_an_empty_database_and_are_recorded() {
    let db = TempDb::new();
    let store = Store::open(db.path()).unwrap();
    let recorded: Vec<(u32, String)> = store
        .conn
        .prepare("SELECT version, name FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let expected: Vec<(u32, String)> = migrate::names()
        .into_iter()
        .map(|(version, name)| (version, name.to_string()))
        .collect();
    assert_eq!(recorded, expected);
    assert_eq!(recorded[0], (1, "schedule".to_string()));
    let applied_at: i64 = store
        .conn
        .query_row(
            "SELECT applied_at_ms FROM schema_migrations WHERE version = 1",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert!(
        applied_at > 1_700_000_000_000,
        "applied_at_ms is epoch milliseconds"
    );
}

#[test]
fn migration_numbers_start_at_one_and_have_no_gaps() {
    let versions: Vec<u32> = migrate::names().into_iter().map(|(v, _)| v).collect();
    let expected: Vec<u32> = (1..=versions.len() as u32).collect();
    assert_eq!(versions, expected);
}

#[test]
fn reopening_a_migrated_database_applies_nothing() {
    let db = TempDb::new();
    drop(Store::open(db.path()).unwrap());
    let before: i64 = {
        let store = Store::open(db.path()).unwrap();
        store
            .conn
            .query_row(
                "SELECT SUM(applied_at_ms) FROM schema_migrations",
                [],
                |r| r.get(0),
            )
            .unwrap()
    };
    let store = Store::open(db.path()).unwrap();
    let (count, sum): (u32, i64) = store
        .conn
        .query_row(
            "SELECT COUNT(*), SUM(applied_at_ms) FROM schema_migrations",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .unwrap();
    assert_eq!(count as usize, migrate::names().len());
    assert_eq!(sum, before);
}

#[test]
fn a_database_from_a_newer_build_is_refused() {
    let db = TempDb::new();
    let store = Store::open(db.path()).unwrap();
    let newer = migrate::latest_version() + 1;
    store
        .conn
        .execute(
            "INSERT INTO schema_migrations (version, name, applied_at_ms) VALUES (?1, 'future', 0)",
            [newer],
        )
        .unwrap();
    drop(store);
    match Store::open(db.path()) {
        Err(StoreError::NewerSchema { found, supported }) => {
            assert_eq!((found, supported), (newer, migrate::latest_version()));
        }
        other => panic!("expected NewerSchema, got {:?}", other.err()),
    }
}

#[test]
fn the_database_uses_wal_a_busy_timeout_and_foreign_keys() {
    let db = TempDb::new();
    let store = Store::open(db.path()).unwrap();
    let mode: String = store
        .conn
        .query_row("PRAGMA journal_mode", [], |r| r.get(0))
        .unwrap();
    let timeout: i64 = store
        .conn
        .query_row("PRAGMA busy_timeout", [], |r| r.get(0))
        .unwrap();
    let foreign_keys: i64 = store
        .conn
        .query_row("PRAGMA foreign_keys", [], |r| r.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    assert_eq!(timeout, BUSY_TIMEOUT.as_millis() as i64);
    assert_eq!(foreign_keys, 1);
}

#[test]
fn events_read_back_by_time_range_in_start_order() {
    let mut store = Store::open_in_memory().unwrap();
    let mut events = schedule();
    // Insert out of order; reads still come back sorted.
    events.reverse();
    store.write_events(3, &events).unwrap();

    let all = store.events_between(3, 0, 100_000).unwrap();
    assert_eq!(all, schedule());

    // An event that began before the range but is still on is included.
    let middle = store.events_between(3, 5_000, 12_000).unwrap();
    assert_eq!(starts(&middle), [0, 10_000]);

    // The range is half-open on both sides.
    assert_eq!(
        starts(&store.events_between(3, 10_000, 12_000).unwrap()),
        [10_000]
    );
    assert_eq!(
        starts(&store.events_between(3, 20_000, 30_000).unwrap()),
        Vec::<i64>::new()
    );
    assert_eq!(
        starts(&store.events_between(3, 20_000, 30_001).unwrap()),
        [30_000]
    );

    // An empty or reversed range has no events.
    assert!(store.events_between(3, 5_000, 5_000).unwrap().is_empty());
    assert!(store.events_between(3, 9_000, 1_000).unwrap().is_empty());
}

#[test]
fn every_kind_and_field_survives_a_round_trip() {
    let mut store = Store::open_in_memory().unwrap();
    let events: Vec<Event> = [
        EventKind::Program,
        EventKind::Break,
        EventKind::Filler,
        EventKind::Ident,
    ]
    .into_iter()
    .enumerate()
    .map(|(i, kind)| {
        let start = i as i64 * 1_000;
        let mut e = event(start, start + 1_000, &format!("media-{i}"), kind);
        e.media_offset_ms = 7 + i as u64;
        e.media_duration_ms = u32::MAX as u64 * 4;
        e
    })
    .collect();
    store.write_events(1, &events).unwrap();
    assert_eq!(store.events_between(1, 0, 10_000).unwrap(), events);
}

#[test]
fn events_survive_closing_and_reopening_the_database() {
    let db = TempDb::new();
    {
        let mut store = Store::open(db.path()).unwrap();
        store.write_events(3, &schedule()).unwrap();
    }
    let store = Store::open(db.path()).unwrap();
    assert_eq!(store.events_between(3, 0, 100_000).unwrap(), schedule());
}

#[test]
fn replacing_events_from_a_time_leaves_earlier_events_alone() {
    let db = TempDb::new();
    let mut store = Store::open(db.path()).unwrap();
    store.write_events(3, &schedule()).unwrap();

    // Replace from 12s: the second `a` and `b` go, the first two stay.
    let replacement = vec![
        event(12_000, 25_000, "c", EventKind::Program),
        event(25_000, 26_000, "id", EventKind::Ident),
    ];
    store.replace_events_from(3, 12_000, &replacement).unwrap();

    let events = store.events_between(3, 0, 100_000).unwrap();
    assert_eq!(&events[..2], &schedule()[..2]);
    assert_eq!(&events[2..], &replacement[..]);
    assert_eq!(events.len(), 4);

    drop(store);
    let store = Store::open(db.path()).unwrap();
    assert_eq!(store.events_between(3, 0, 100_000).unwrap(), events);
}

#[test]
fn replacing_with_no_events_only_deletes() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, &schedule()).unwrap();
    store.replace_events_from(3, 10_000, &[]).unwrap();
    assert_eq!(starts(&store.events_between(3, 0, 100_000).unwrap()), [0]);
}

#[test]
fn a_failed_replace_changes_nothing() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, &schedule()).unwrap();

    // The second new event repeats the first one's start: the insert fails
    // after the delete has run, and the whole transaction must roll back.
    let clash = vec![
        event(12_000, 14_000, "x", EventKind::Program),
        event(12_000, 15_000, "y", EventKind::Program),
    ];
    assert!(matches!(
        store.replace_events_from(3, 12_000, &clash),
        Err(StoreError::Database { .. })
    ));
    assert_eq!(store.events_between(3, 0, 100_000).unwrap(), schedule());

    // A failed write adds none of its events either.
    let more = vec![
        event(50_000, 51_000, "p", EventKind::Program),
        event(0, 1_000, "q", EventKind::Program),
    ];
    assert!(store.write_events(3, &more).is_err());
    assert_eq!(store.events_between(3, 0, 100_000).unwrap(), schedule());
}

#[test]
fn replace_rejects_events_before_the_replace_point() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, &schedule()).unwrap();
    let early = vec![event(5_000, 15_000, "x", EventKind::Program)];
    assert!(matches!(
        store.replace_events_from(3, 12_000, &early),
        Err(StoreError::InvalidEvent(_))
    ));
    assert_eq!(store.events_between(3, 0, 100_000).unwrap(), schedule());
}

#[test]
fn channels_do_not_see_each_others_events() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, &schedule()).unwrap();
    let other = vec![event(0, 5_000, "z", EventKind::Program)];
    store.write_events(4, &other).unwrap();

    assert_eq!(store.events_between(4, 0, 100_000).unwrap(), other);
    assert_eq!(store.events_between(3, 0, 100_000).unwrap(), schedule());
    assert!(store.events_between(5, 0, 100_000).unwrap().is_empty());

    // Replacing on one channel leaves the other alone.
    store.replace_events_from(3, 0, &[]).unwrap();
    assert!(store.events_between(3, 0, 100_000).unwrap().is_empty());
    assert_eq!(store.events_between(4, 0, 100_000).unwrap(), other);
}

#[test]
fn an_event_that_does_not_end_after_it_starts_is_rejected() {
    let mut store = Store::open_in_memory().unwrap();
    for (start, end) in [(5_000, 5_000), (5_000, 4_000)] {
        let events = vec![
            event(0, 1_000, "ok", EventKind::Program),
            event(start, end, "bad", EventKind::Program),
        ];
        let error = store.write_events(3, &events).unwrap_err();
        assert!(matches!(error, StoreError::InvalidEvent(_)));
        assert!(error.to_string().contains("not after its start"), "{error}");
    }
    // Nothing was written, not even the valid event before the bad one.
    assert!(store.events_between(3, 0, 100_000).unwrap().is_empty());
}

#[test]
fn a_value_too_large_for_sqlite_is_rejected() {
    let mut store = Store::open_in_memory().unwrap();
    let mut huge = event(0, 1_000, "a", EventKind::Program);
    huge.media_duration_ms = u64::MAX;
    assert!(matches!(
        store.write_events(3, &[huge]),
        Err(StoreError::InvalidEvent(_))
    ));
}

#[test]
fn a_second_event_at_the_same_start_is_an_error() {
    let mut store = Store::open_in_memory().unwrap();
    store
        .write_events(3, &[event(0, 1_000, "a", EventKind::Program)])
        .unwrap();
    let error = store
        .write_events(3, &[event(0, 2_000, "b", EventKind::Program)])
        .unwrap_err();
    assert!(matches!(error, StoreError::Database { .. }), "{error}");
}

#[test]
fn reading_a_channels_time_range_uses_the_index() {
    let store = Store::open_in_memory().unwrap();
    let plan: Vec<String> = store
        .conn
        .prepare(
            "EXPLAIN QUERY PLAN
             SELECT * FROM events
             WHERE channel = 1 AND start_ms >= 5 AND start_ms < 10 AND end_ms > 5
             ORDER BY start_ms",
        )
        .unwrap()
        .query_map([], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let plan = plan.join("; ");
    assert!(plan.contains("SEARCH events USING PRIMARY KEY"), "{plan}");
    assert!(!plan.contains("SCAN"), "{plan}");
}

#[test]
fn programs_before_lists_program_ids_oldest_first() {
    let db = TempDb::new();
    let mut store = Store::open(db.path()).unwrap();
    store.write_events(3, &schedule()).unwrap();
    store
        .write_events(3, &[event(40_000, 50_000, "c", EventKind::Program)])
        .unwrap();
    store
        .write_events(4, &[event(0, 10_000, "other", EventKind::Program)])
        .unwrap();

    // The break and the filler are left out; `a` appears once per event.
    assert_eq!(
        store.programs_before(3, i64::MAX).unwrap(),
        ids(&["a", "a", "c"])
    );
    // The bound is exclusive: `c` starts at 40s.
    assert_eq!(store.programs_before(3, 40_000).unwrap(), ids(&["a", "a"]));
    assert_eq!(store.programs_before(3, 0).unwrap(), ids(&[]));
    assert_eq!(store.programs_before(4, i64::MAX).unwrap(), ids(&["other"]));
    assert!(store.programs_before(5, i64::MAX).unwrap().is_empty());

    drop(store);
    let store = Store::open(db.path()).unwrap();
    assert_eq!(
        store.programs_before(3, i64::MAX).unwrap(),
        ids(&["a", "a", "c"])
    );
}

#[test]
fn migration_3_drops_the_cursors_table() {
    let cursors = |conn: &Connection| -> u32 {
        conn.query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name = 'cursors'",
            [],
            |r| r.get(0),
        )
        .unwrap()
    };
    let store = Store::open_in_memory().unwrap();
    assert_eq!(cursors(&store.conn), 0);

    // A database a version-2 build left behind, with an event and a cursor.
    let db = TempDb::new();
    {
        let conn = Connection::open(db.path()).unwrap();
        conn.execute_batch(concat!(
            "CREATE TABLE schema_migrations (
                version       INTEGER PRIMARY KEY,
                name          TEXT    NOT NULL,
                applied_at_ms INTEGER NOT NULL
            );",
            include_str!("../../migrations/0001_schedule.sql"),
            include_str!("../../migrations/0002_library_cache.sql"),
            "INSERT INTO schema_migrations VALUES (1, 'schedule', 1), (2, 'library_cache', 2);
             INSERT INTO events VALUES (3, 0, 10000, 'a', 0, 10000, 'program');
             INSERT INTO cursors VALUES (3, 'a');",
        ))
        .unwrap();
        assert_eq!(cursors(&conn), 1);
    }

    let store = Store::open(db.path()).unwrap();
    assert_eq!(cursors(&store.conn), 0);
    let applied: Vec<(u32, i64)> = store
        .conn
        .prepare("SELECT version, applied_at_ms FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    // Only migration 3 ran; 1 and 2 kept their recorded times.
    assert_eq!(applied.len(), 3);
    assert_eq!(&applied[..2], [(1, 1), (2, 2)]);
    assert_eq!(applied[2].0, 3);
    assert_eq!(
        store.events_between(3, i64::MIN, i64::MAX).unwrap(),
        [event(0, 10_000, "a", EventKind::Program)].map(|mut e| {
            e.media_duration_ms = 10_000;
            e
        })
    );
}

#[test]
fn playback_history_round_trips_newest_first_per_channel() {
    let db = TempDb::new();
    let mut store = Store::open(db.path()).unwrap();
    store
        .record_playback(3, &MediaId::new("a"), 1_000, "played")
        .unwrap();
    store
        .record_playback(3, &MediaId::new("b"), 3_000, "failed")
        .unwrap();
    store
        .record_playback(3, &MediaId::new("c"), 2_000, "played")
        .unwrap();
    store
        .record_playback(4, &MediaId::new("z"), 9_000, "played")
        .unwrap();

    let record = |media: &str, at: i64, outcome: &str| PlaybackRecord {
        channel: 3,
        media_id: MediaId::new(media),
        started_at_ms: at,
        outcome: outcome.to_string(),
    };
    let expected = vec![
        record("b", 3_000, "failed"),
        record("c", 2_000, "played"),
        record("a", 1_000, "played"),
    ];
    assert_eq!(store.playback_history(3, 10).unwrap(), expected);
    assert_eq!(store.playback_history(3, 2).unwrap(), expected[..2]);
    assert_eq!(store.playback_history(4, 10).unwrap().len(), 1);
    assert!(store.playback_history(5, 10).unwrap().is_empty());

    drop(store);
    let store = Store::open(db.path()).unwrap();
    assert_eq!(store.playback_history(3, 10).unwrap(), expected);
}

#[test]
fn an_unknown_kind_in_the_database_is_reported_as_corrupt() {
    // The CHECK constraint blocks bad kinds, so test the reader directly.
    let error = build_event(0, 1, "a".into(), 0, 0, "weird").unwrap_err();
    assert!(matches!(error, StoreError::Corrupt(_)));
    let error = build_event(0, 1, "a".into(), -1, 0, "program").unwrap_err();
    assert!(matches!(error, StoreError::Corrupt(_)));
}

#[test]
fn the_error_type_works_as_a_std_error() {
    fn takes_error(_: &dyn std::error::Error) {}
    takes_error(&StoreError::Corrupt("x".into()));
}
