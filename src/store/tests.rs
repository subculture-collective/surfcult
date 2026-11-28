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
fn recent_programs_before_lists_program_ids_newest_first() {
    let recent = |store: &Store, channel, before_ms, limit| -> Vec<MediaId> {
        store
            .recent_programs_before(channel, before_ms, limit)
            .unwrap()
            .into_iter()
            .map(|(_, id)| id)
            .collect()
    };
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
    assert_eq!(recent(&store, 3, i64::MAX, 10), ids(&["c", "a", "a"]));
    // The limit keeps the newest rows.
    assert_eq!(recent(&store, 3, i64::MAX, 2), ids(&["c", "a"]));
    // The bound is exclusive: `c` starts at 40s.
    assert_eq!(recent(&store, 3, 40_000, 10), ids(&["a", "a"]));
    assert_eq!(recent(&store, 3, 0, 10), ids(&[]));
    assert_eq!(recent(&store, 4, i64::MAX, 10), ids(&["other"]));
    assert!(recent(&store, 5, i64::MAX, 10).is_empty());
    // Each row carries its start time, the bound for reading further back.
    assert_eq!(
        store.recent_programs_before(3, i64::MAX, 1).unwrap(),
        vec![(40_000, MediaId::new("c"))]
    );

    drop(store);
    let store = Store::open(db.path()).unwrap();
    assert_eq!(recent(&store, 3, i64::MAX, 10), ids(&["c", "a", "a"]));
}

#[test]
fn reading_recent_programs_uses_the_index() {
    let store = Store::open_in_memory().unwrap();
    let plan: Vec<String> = store
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {RECENT_PROGRAMS_BEFORE}"))
        .unwrap()
        .query_map(params![3, 10, 20], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let plan = plan.join("; ");
    assert!(plan.contains("SEARCH events USING PRIMARY KEY"), "{plan}");
    assert!(!plan.contains("SCAN"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
}

#[test]
fn last_program_before_is_the_newest_program_that_starts_before_the_bound() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, &schedule()).unwrap();
    store
        .write_events(3, &[event(40_000, 50_000, "c", EventKind::Program)])
        .unwrap();

    assert_eq!(
        store.last_program_before(3, i64::MAX).unwrap(),
        Some(MediaId::new("c"))
    );
    // The bound is exclusive, and the break and filler after `a` are skipped.
    assert_eq!(
        store.last_program_before(3, 40_000).unwrap(),
        Some(MediaId::new("a"))
    );
    assert_eq!(store.last_program_before(3, 0).unwrap(), None);
    assert_eq!(store.last_program_before(4, i64::MAX).unwrap(), None);
}

#[test]
fn reading_the_last_program_uses_the_index() {
    let store = Store::open_in_memory().unwrap();
    let plan: Vec<String> = store
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {LAST_PROGRAM_BEFORE}"))
        .unwrap()
        .query_map(params![3, 10], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let plan = plan.join("; ");
    assert!(plan.contains("SEARCH events USING PRIMARY KEY"), "{plan}");
    assert!(!plan.contains("SCAN"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
}

#[test]
fn clips_between_are_the_clips_in_the_window_newest_first() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, &schedule()).unwrap();
    store
        .write_events(
            3,
            &[
                event(40_000, 41_000, "ad2", EventKind::Break),
                event(41_000, 42_000, "bump", EventKind::Ident),
            ],
        )
        .unwrap();
    let ad = (10_000, MediaId::new("ad"));
    let filler = (30_000, MediaId::new("b"));
    let ad2 = (40_000, MediaId::new("ad2"));
    let bump = (41_000, MediaId::new("bump"));

    assert_eq!(
        store.clips_between(3, 0, i64::MAX).unwrap(),
        [bump.clone(), ad2.clone(), filler.clone(), ad.clone()]
    );
    // The upper bound is exclusive; programs are skipped.
    assert_eq!(
        store.clips_between(3, 0, 40_000).unwrap(),
        [filler.clone(), ad.clone()]
    );
    assert!(store.clips_between(3, 0, 10_000).unwrap().is_empty());
    assert!(store.clips_between(4, 0, i64::MAX).unwrap().is_empty());
    // The lower bound is inclusive, and the walk stops there.
    assert_eq!(store.clips_between(3, 10_000, 30_000).unwrap(), [ad]);
    assert!(store.clips_between(3, 10_001, 30_000).unwrap().is_empty());
    assert_eq!(store.clips_between(3, 40_001, i64::MAX).unwrap(), [bump]);
}

#[test]
fn reading_recent_clips_uses_the_index() {
    let store = Store::open_in_memory().unwrap();
    let plan: Vec<String> = store
        .conn
        .prepare(&format!("EXPLAIN QUERY PLAN {CLIPS_BETWEEN}"))
        .unwrap()
        .query_map(params![3, 0, 10], |row| row.get::<_, String>(3))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let plan = plan.join("; ");
    assert!(plan.contains("SEARCH events USING PRIMARY KEY"), "{plan}");
    assert!(!plan.contains("SCAN"), "{plan}");
    assert!(!plan.contains("TEMP B-TREE"), "{plan}");
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
    // Migration 3 and the later ones ran; 1 and 2 kept their recorded times.
    assert_eq!(applied.len() as u32, migrate::latest_version());
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
fn migration_4_keeps_cached_collections_and_adds_source_fields() {
    use crate::library::{Collection, MediaItem, MediaKind, SeriesEpisode};

    // A database a version-3 build left behind, with a cached collection.
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
            include_str!("../../migrations/0003_drop_cursors.sql"),
            "INSERT INTO schema_migrations VALUES
                 (1, 'schedule', 1), (2, 'library_cache', 2), (3, 'drop_cursors', 3);
             INSERT INTO library_items VALUES ('a', 0, 'A', 'episode', 1000, NULL, 7);
             INSERT INTO library_collections VALUES (0, 'Cartoons');
             INSERT INTO library_collection_members VALUES (0, 0, 'a');",
        ))
        .unwrap();
    }

    let mut store = Store::open(db.path()).unwrap();

    // The old row has no source fields until the next sync.
    let items = store.library_items().unwrap();
    assert_eq!(
        items[0].item,
        MediaItem::new("a", "A", MediaKind::Episode, 1000)
    );
    assert_eq!(
        store.library_collections().unwrap(),
        [Collection {
            name: "Cartoons".into(),
            members: ids(&["a"]),
        }]
    );
    // A collection can now list a series.
    let episode = LibraryEntry {
        item: MediaItem {
            series: Some(SeriesEpisode {
                series_id: MediaId::new("s"),
                series_name: "S".into(),
                season: Some(1),
                number: Some(1),
            }),
            tags: vec!["Retro".into()],
            ..MediaItem::new("b", "B", MediaKind::Episode, 1000)
        },
        path: None,
    };
    let series_collection = Collection {
        name: "Series".into(),
        members: ids(&["s"]),
    };
    store
        .replace_library(
            std::slice::from_ref(&episode),
            std::slice::from_ref(&series_collection),
            8,
        )
        .unwrap();
    assert_eq!(store.library_items().unwrap(), [episode]);
    assert_eq!(store.library_collections().unwrap(), [series_collection]);
}

#[test]
fn migration_7_keeps_the_cached_library_and_adds_music_videos() {
    use crate::library::{
        AudioStream, Chapter, Collection, MediaItem, MediaKind, SeriesEpisode, VideoStream,
    };

    // A database a version-6 build left behind, with an episode that has a
    // chapter, streams and labels, and a collection that lists it.
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
            include_str!("../../migrations/0003_drop_cursors.sql"),
            include_str!("../../migrations/0004_library_sources.sql"),
            include_str!("../../migrations/0005_substitutions.sql"),
            include_str!("../../migrations/0006_viewer_settings.sql"),
            "INSERT INTO schema_migrations VALUES
                 (1, 'schedule', 1), (2, 'library_cache', 2), (3, 'drop_cursors', 3),
                 (4, 'library_sources', 4), (5, 'substitutions', 5),
                 (6, 'viewer_settings', 6);
             INSERT INTO library_items
                 (id, position, title, kind, runtime_ms, path, synced_at_ms,
                  series_id, series_name, season, episode)
                 VALUES ('a', 0, 'A', 'episode', 1000, '/a.mkv', 7, 's', 'S', 1, 2);
             INSERT INTO library_chapters VALUES ('a', 0, 500, 'Two');
             INSERT INTO library_streams VALUES
                 ('a', 'video', 0, 'h264', 640, 480, 8, NULL, NULL),
                 ('a', 'audio', 0, 'aac', NULL, NULL, NULL, 2, 'eng');
             INSERT INTO library_item_labels VALUES
                 ('a', 'library', 0, 'TV'), ('a', 'tag', 0, 'Retro'), ('a', 'genre', 0, 'Comedy');
             INSERT INTO library_collections VALUES (0, 'Cartoons');
             INSERT INTO library_collection_members VALUES (0, 0, 's');",
        ))
        .unwrap();
    }

    let mut store = Store::open(db.path()).unwrap();

    let episode = LibraryEntry {
        item: MediaItem {
            chapters: vec![Chapter {
                start_ms: 500,
                name: Some("Two".into()),
            }],
            video: vec![VideoStream {
                codec: "h264".into(),
                width: 640,
                height: 480,
                bit_depth: Some(8),
            }],
            audio: vec![AudioStream {
                codec: "aac".into(),
                channels: 2,
                language: Some("eng".into()),
            }],
            libraries: vec!["TV".into()],
            tags: vec!["Retro".into()],
            genres: vec!["Comedy".into()],
            series: Some(SeriesEpisode {
                series_id: MediaId::new("s"),
                series_name: "S".into(),
                season: Some(1),
                number: Some(2),
            }),
            ..MediaItem::new("a", "A", MediaKind::Episode, 1000)
        },
        path: Some("/a.mkv".into()),
    };
    assert_eq!(store.library_items().unwrap(), [episode]);
    assert_eq!(
        store.library_collections().unwrap(),
        [Collection {
            name: "Cartoons".into(),
            members: ids(&["s"]),
        }]
    );
    let problems: i64 = store
        .conn
        .query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(problems, 0);
    // The rebuilt children point at `library_items`, not at the old name.
    let stale: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE sql LIKE '%library_items_new%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(stale, 0);
    // A kind the table does not list is refused.
    assert!(
        store
            .conn
            .execute(
                "INSERT INTO library_items (id, position, title, kind, runtime_ms, synced_at_ms)
                 VALUES ('x', 1, 'X', 'podcast', 1, 1)",
                [],
            )
            .is_err()
    );

    // A music video can now be cached, and replacing the cache still removes
    // the old item's chapters, streams and labels through their foreign keys.
    let video = LibraryEntry {
        item: MediaItem {
            artists: vec!["Artist".into()],
            album: Some("Album".into()),
            ..MediaItem::new("v", "Song", MediaKind::MusicVideo, 240_000)
        },
        path: None,
    };
    store
        .replace_library(std::slice::from_ref(&video), &[], 8)
        .unwrap();
    assert_eq!(store.library_items().unwrap(), [video]);
    for table in ["library_chapters", "library_streams"] {
        let rows: i64 = store
            .conn
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }
    let labels: i64 = store
        .conn
        .query_row(
            "SELECT COUNT(*) FROM library_item_labels WHERE item_id = 'a'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(labels, 0);
}

#[test]
fn migration_8_keeps_the_sync_time_and_adds_songs() {
    use crate::library::{MediaItem, MediaKind};

    // Databases a version-7 build left behind, one with a cached item and
    // one with none.
    let v7 = |items: &str| {
        let db = TempDb::new();
        let conn = Connection::open(db.path()).unwrap();
        conn.execute_batch(&format!(
            "{}{}{}{}{}{}{}{}{items}",
            "CREATE TABLE schema_migrations (
                version       INTEGER PRIMARY KEY,
                name          TEXT    NOT NULL,
                applied_at_ms INTEGER NOT NULL
            );
            INSERT INTO schema_migrations VALUES
                 (1, 'schedule', 1), (2, 'library_cache', 2), (3, 'drop_cursors', 3),
                 (4, 'library_sources', 4), (5, 'substitutions', 5),
                 (6, 'viewer_settings', 6), (7, 'music_videos', 7);",
            include_str!("../../migrations/0001_schedule.sql"),
            include_str!("../../migrations/0002_library_cache.sql"),
            include_str!("../../migrations/0003_drop_cursors.sql"),
            include_str!("../../migrations/0004_library_sources.sql"),
            include_str!("../../migrations/0005_substitutions.sql"),
            include_str!("../../migrations/0006_viewer_settings.sql"),
            include_str!("../../migrations/0007_music_videos.sql"),
        ))
        .unwrap();
        db
    };
    let empty = v7("");
    assert_eq!(
        Store::open(empty.path())
            .unwrap()
            .library_synced_at_ms()
            .unwrap(),
        None
    );
    let cached = v7(
        "INSERT INTO library_items (id, position, title, kind, runtime_ms, synced_at_ms)
         VALUES ('a', 0, 'A', 'movie', 1000, 7), ('b', 1, 'B', 'movie', 1000, 9);",
    );
    let mut store = Store::open(cached.path()).unwrap();
    assert_eq!(store.library_synced_at_ms().unwrap(), Some(9));
    assert_eq!(store.library_items().unwrap().len(), 2);

    let song = LibraryEntry {
        item: MediaItem {
            album_id: Some(MediaId::new("album")),
            ..MediaItem::new("s", "Song", MediaKind::Song, 200_000)
        },
        path: None,
    };
    store
        .replace_library(std::slice::from_ref(&song), &[], 10)
        .unwrap();
    assert_eq!(store.library_items().unwrap(), [song]);
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
fn a_collection_pool_resolves_from_the_library_cache() {
    use crate::config::Source;
    use crate::library::{Collection, MediaItem, MediaKind, Plays, resolve_pool};

    let mut store = Store::open_in_memory().unwrap();
    let entries: Vec<_> = ["a", "b", "c"]
        .into_iter()
        .map(|id| LibraryEntry {
            item: MediaItem::new(id, id, MediaKind::Episode, 1_320_000),
            path: None,
        })
        .collect();
    let cartoons = Collection {
        name: "Cartoons".into(),
        members: ids(&["c", "a"]),
    };
    store.replace_library(&entries, &[cartoons], 1).unwrap();

    let pool = resolve_pool(
        3,
        Plays::Videos,
        &Source::JellyfinCollection("Cartoons".into()),
        &store,
    )
    .unwrap();

    assert_eq!(pool, vec![entries[2].item.clone(), entries[0].item.clone()]);
}

#[test]
fn a_new_id_for_the_same_path_keeps_the_item_and_its_events() {
    use crate::library::{MediaItem, MediaKind, MediaSource};

    let entry = |id: &str, path: &str| LibraryEntry {
        item: MediaItem::new(id, id, MediaKind::Episode, 1_320_000),
        path: Some(path.into()),
    };
    let mut store = Store::open_in_memory().unwrap();
    store
        .replace_library(&[entry("a", "/tv/a.mkv"), entry("b", "/tv/b.mkv")], &[], 1)
        .unwrap();
    store
        .write_events(1, &[event(0, 10, "a", EventKind::Program)])
        .unwrap();
    store
        .write_events(2, &[event(5, 15, "a", EventKind::Program)])
        .unwrap();

    // Jellyfin gave the file a new id; `b` is gone.
    let changes = store
        .replace_library(&[entry("a2", "/tv/a.mkv")], &[], 2)
        .unwrap();

    assert_eq!(changes.renamed, [(MediaId::new("a"), MediaId::new("a2"))]);
    assert_eq!(changes.vanished, ids(&["b"]));
    assert!(changes.resized.is_empty());
    for channel in [1, 2] {
        let events = store.events_between(channel, 0, 100).unwrap();
        assert_eq!(events[0].media_id.as_str(), "a2");
        store.resolve(&events[0].media_id).unwrap();
    }
}

#[test]
fn substitutions_read_back_by_channel_in_scheduled_order() {
    let mut store = Store::open_in_memory().unwrap();
    let substitution = |channel, media: &str, at, replacement: Option<&str>| Substitution {
        channel,
        media_id: MediaId::new(media),
        scheduled_ms: at,
        replacement: replacement.map(MediaId::new),
    };
    store
        .record_substitution(&substitution(1, "b", 200, None), 9)
        .unwrap();
    store
        .record_substitution(&substitution(1, "a", 100, Some("x")), 9)
        .unwrap();
    store
        .record_substitution(&substitution(2, "c", 50, Some("y")), 9)
        .unwrap();

    assert_eq!(
        store.substitutions(1).unwrap(),
        [
            substitution(1, "a", 100, Some("x")),
            substitution(1, "b", 200, None)
        ]
    );
    assert_eq!(store.substitutions(3).unwrap(), []);
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

#[test]
fn the_schedule_end_is_the_end_of_the_last_event() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.schedule_end_ms(1).unwrap(), None);

    store.write_events(1, &schedule()).unwrap();

    assert_eq!(store.schedule_end_ms(1).unwrap(), Some(40_000));
    assert_eq!(store.schedule_end_ms(2).unwrap(), None);
}

#[test]
fn the_schema_version_is_the_newest_migration() {
    let store = Store::open_in_memory().unwrap();
    assert_eq!(store.schema_version().unwrap(), migrate::latest_version());
    assert_eq!(Store::supported_schema_version(), migrate::latest_version());
}

#[test]
fn opening_read_only_needs_an_existing_file() {
    let db = TempDb::new();

    assert!(Store::open_read_only(db.path()).is_err());
    assert!(!db.path().exists());
}

#[test]
fn a_read_only_store_sees_the_writer_and_cannot_write() {
    let db = TempDb::new();
    let mut writer = Store::open(db.path()).unwrap();
    let mut reader = Store::open_read_only(db.path()).unwrap();

    writer.write_events(1, &schedule()).unwrap();

    assert_eq!(reader.schedule_end_ms(1).unwrap(), Some(40_000));
    assert!(reader.write_events(2, &schedule()).is_err());
}

#[test]
fn a_read_only_store_does_not_migrate() {
    let db = TempDb::new();
    let store = Store::open(db.path()).unwrap();
    store
        .conn
        .execute(
            "DELETE FROM schema_migrations WHERE version = ?1",
            [migrate::latest_version()],
        )
        .unwrap();
    drop(store);

    let reader = Store::open_read_only(db.path()).unwrap();

    assert_eq!(
        reader.schema_version().unwrap(),
        migrate::latest_version() - 1
    );
}

#[test]
fn a_viewer_setting_is_absent_until_chosen_and_the_last_choice_wins() {
    let mut store = Store::open_in_memory().unwrap();
    assert_eq!(store.viewer_setting("crt").unwrap(), None);
    store.set_viewer_setting("crt", "light").unwrap();
    store.set_viewer_setting("crt", "off").unwrap();
    assert_eq!(store.viewer_setting("crt").unwrap().as_deref(), Some("off"));
    assert_eq!(store.viewer_setting("other").unwrap(), None);
}
