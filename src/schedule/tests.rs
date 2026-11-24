use std::path::PathBuf;
use std::time::Duration;

use chrono::{TimeZone, Utc};

use super::breaks::cut_points;
use super::*;
use crate::clips::{Clip, Pool as ClipPool};
use crate::config::{Breaks, Channel, Mode, Plan, Source};
use crate::library::{Chapter, FakeMediaSource, MediaId, MediaItem, MediaKind, resolve_pool};
use crate::store::Store;
use crate::timeline::{Event, EventKind};

const MINUTE: i64 = 60_000;
const DAY: i64 = 24 * 60 * MINUTE;

fn item(id: &str, minutes: u64) -> MediaItem {
    MediaItem::new(id, id, MediaKind::Episode, minutes * 60_000)
}

fn pool(ids: &[&str]) -> Vec<MediaItem> {
    // Different runtimes, so a mixed-up order shows in the times too.
    ids.iter()
        .enumerate()
        .map(|(i, id)| item(id, 20 + 7 * i as u64))
        .collect()
}

fn channel(number: u32, mode: Mode) -> Channel {
    Channel {
        number,
        name: format!("Channel {number}"),
        plan: Plan::Fixed {
            source: Source::JellyfinLibrary("All".into()),
            mode,
        },
        grid: None,
        breaks: None,
        filler: None,
    }
}

fn params(mode: Mode) -> Params<'static> {
    Params {
        channel: 3,
        mode,
        breaks: None,
    }
}

fn at(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    Utc.with_ymd_and_hms(year, month, day, hour, minute, 0)
        .unwrap()
        .timestamp_millis()
}

fn ids(events: &[Event]) -> Vec<&str> {
    events.iter().map(|e| e.media_id.as_str()).collect()
}

fn resume(cursor: &str) -> Resume {
    Resume {
        recent: vec![MediaId::new(cursor)],
        last_break: None,
    }
}

/// Events join up exactly and cover `[from, to)`.
fn assert_covers(events: &[Event], from: i64, to: i64) {
    assert!(!events.is_empty());
    assert_eq!(events[0].start_ms, from);
    for pair in events.windows(2) {
        assert_eq!(pair[0].end_ms, pair[1].start_ms, "gap or overlap");
    }
    let last = events.last().unwrap();
    assert!(last.end_ms >= to, "ends before the window does");
    assert!(last.start_ms < to, "an event starts after the window");
}

#[test]
fn programs_are_whole_items_back_to_back() {
    let items = pool(&["a", "b", "c"]);
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &params(Mode::Sequential),
        &items,
        &Resume::default(),
        start,
        start + 3 * 60 * MINUTE,
    )
    .unwrap();

    let first = &out.events[0];
    assert_eq!(first.kind, EventKind::Program);
    assert_eq!(first.media_offset_ms, 0);
    assert_eq!(first.media_duration_ms, 20 * 60_000);
    assert_eq!(first.end_ms - first.start_ms, 20 * MINUTE);
    assert_eq!(out.events[1].start_ms, first.end_ms);
    assert!(out.notes.is_empty());
}

#[test]
fn the_same_inputs_give_the_same_events() {
    let items = pool(&["a", "b", "c", "d", "e", "f"]);
    let start = at(2026, 6, 1, 12, 0);
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let run = || generate(&params(mode), &items, &resume("b"), start, start + 7 * DAY).unwrap();
        assert_eq!(run(), run(), "{mode:?}");
    }
}

#[test]
fn sequential_continues_after_the_cursor_and_wraps() {
    let items = pool(&["a", "b", "c", "d"]);
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &params(Mode::Sequential),
        &items,
        &resume("c"),
        start,
        start + 6 * 60 * MINUTE,
    )
    .unwrap();

    assert_eq!(&ids(&out.events)[..6], ["d", "a", "b", "c", "d", "a"]);
    let last = out.events.last().unwrap();
    assert_eq!(
        out.cursor.as_ref().map(MediaId::as_str),
        Some(last.media_id.as_str())
    );
}

#[test]
fn sequential_starts_at_the_beginning_without_a_cursor() {
    let items = pool(&["a", "b", "c"]);
    let out = generate(&params(Mode::Sequential), &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(ids(&out.events), ["a"]);
    assert_eq!(out.cursor.as_ref().map(MediaId::as_str), Some("a"));
}

#[test]
fn sequential_restarts_and_says_so_when_the_cursor_item_vanished() {
    let items = pool(&["a", "b", "c"]);

    let out = generate(&params(Mode::Sequential), &items, &resume("gone"), 0, 1).unwrap();

    assert_eq!(ids(&out.events), ["a"]);
    assert_eq!(
        out.notes,
        [Note::CursorNotInPool {
            media_id: MediaId::new("gone")
        }]
    );
}

#[test]
fn sequential_skips_zero_runtime_items_and_continues_past_a_zero_cursor() {
    let items = vec![item("a", 20), item("zero", 0), item("c", 20)];

    let out = generate(
        &params(Mode::Sequential),
        &items,
        &resume("a"),
        0,
        60 * MINUTE,
    )
    .unwrap();

    assert_eq!(ids(&out.events), ["c", "a", "c"]);
    assert_eq!(out.notes, [Note::SkippedZeroRuntime { count: 1 }]);

    let after_zero = generate(&params(Mode::Sequential), &items, &resume("zero"), 0, 1).unwrap();
    assert_eq!(ids(&after_zero.events), ["c"]);
    assert!(
        after_zero
            .notes
            .contains(&Note::SkippedZeroRuntime { count: 1 })
    );
}

#[test]
fn an_empty_or_unplayable_pool_gives_no_events_and_a_note() {
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let none = generate(&params(mode), &[], &resume("x"), 0, DAY).unwrap();
        assert!(none.events.is_empty());
        assert_eq!(none.cursor.as_ref().map(MediaId::as_str), Some("x"));
        assert_eq!(none.notes, [Note::NoPlayableItems]);

        let zeros = vec![item("a", 0), item("b", 0)];
        let out = generate(&params(mode), &zeros, &Resume::default(), 0, DAY).unwrap();
        assert!(out.events.is_empty());
        assert_eq!(
            out.notes,
            [Note::SkippedZeroRuntime { count: 2 }, Note::NoPlayableItems]
        );
    }
}

#[test]
fn an_empty_window_gives_no_events_and_keeps_the_cursor() {
    let items = pool(&["a", "b"]);
    let out = generate(&params(Mode::Sequential), &items, &resume("a"), 100, 100).unwrap();
    assert!(out.events.is_empty());
    assert_eq!(out.cursor.as_ref().map(MediaId::as_str), Some("a"));
}

#[test]
fn a_duplicate_id_in_the_pool_is_used_once() {
    let items = vec![item("a", 20), item("b", 20), item("a", 45)];
    let out = generate(
        &params(Mode::Sequential),
        &items,
        &Resume::default(),
        0,
        80 * MINUTE,
    )
    .unwrap();
    assert_eq!(ids(&out.events), ["a", "b", "a", "b"]);
    assert_eq!(out.events[0].media_duration_ms, 20 * 60_000);
}

#[test]
fn weighted_is_not_generated_yet() {
    let result = generate(
        &params(Mode::Weighted),
        &pool(&["a"]),
        &Resume::default(),
        0,
        1,
    );
    assert!(matches!(
        result,
        Err(ScheduleError::UnsupportedMode(Mode::Weighted))
    ));
}

/// `n` items of 30 minutes each, named `i0`, `i1`, ...
fn uniform(n: usize) -> Vec<MediaItem> {
    (0..n).map(|i| item(&format!("i{i}"), 30)).collect()
}

fn shuffle(items: &[MediaItem], recent: &[&str], start: i64, end: i64) -> Generated {
    let resume = Resume {
        recent: recent.iter().map(|id| MediaId::new(*id)).collect(),
        last_break: None,
    };
    generate(&params(Mode::Shuffle), items, &resume, start, end).unwrap()
}

/// With `n` playable items, N - k of them are cooling down at each pick, so
/// every run of N - k + 1 consecutive programs has no repeat.
fn assert_cooldown_holds(played: &[&str], n: usize) {
    let window = n - n.div_ceil(4).max(1) + 1;
    for (i, run) in played.windows(window.min(played.len())).enumerate() {
        let mut unique = run.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(unique.len(), run.len(), "repeat at {i}: {run:?}");
    }
}

#[test]
fn a_shuffle_never_repeats_an_item_inside_its_cooldown() {
    let start = at(2026, 6, 1, 12, 0);
    for n in [2, 3, 4, 5, 8, 13, 40] {
        let out = shuffle(&uniform(n), &[], start, start + 7 * DAY);
        assert_cooldown_holds(&ids(&out.events), n);
    }
}

#[test]
fn every_item_of_a_shuffle_plays_within_a_week() {
    let start = at(2026, 6, 1, 12, 0);
    for n in [5, 13, 40] {
        let items = uniform(n);
        let out = shuffle(&items, &[], start, start + 7 * DAY);
        let played = ids(&out.events);
        for item in &items {
            assert!(
                played.contains(&item.id.as_str()),
                "{} never played",
                item.id
            );
        }
    }
}

#[test]
fn a_shuffle_differs_by_channel() {
    let items = uniform(8);
    let start = at(2026, 6, 1, 12, 0);
    let order = |channel: u32| {
        let params = Params {
            channel,
            mode: Mode::Shuffle,
            breaks: None,
        };
        let out = generate(&params, &items, &Resume::default(), start, start + DAY).unwrap();
        out.events
            .into_iter()
            .map(|e| e.media_id)
            .collect::<Vec<_>>()
    };
    assert_eq!(order(3), order(3));
    assert_ne!(order(3), order(4));
}

#[test]
fn a_shuffle_resumed_at_any_boundary_matches_an_unbroken_run() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h", "i"]);
    let start = at(2026, 6, 1, 12, 0);
    let end = start + 7 * DAY;
    let history = ["c", "x", "a", "b"];
    let whole = shuffle(&items, &history, start, end);

    for cut in [1, 2, 5, 9, 17, 40] {
        let boundary = whole.events[cut].start_ms;
        let first = shuffle(&items, &history, start, boundary);
        assert_eq!(first.events, whole.events[..cut]);
        // The stored history, newest first: this run's programs, then the old.
        let mut recent: Vec<&str> = ids(&first.events);
        recent.reverse();
        recent.extend(history);
        let rest = shuffle(&items, &recent, boundary, end);
        assert_eq!(rest.events, whole.events[cut..], "cut at {cut}");
    }
}

#[test]
fn a_new_item_is_eligible_at_once() {
    // N = 4, k = 1: the three old items are cooling down, so only the new one
    // can play next.
    let items = pool(&["a", "b", "c", "new"]);
    let out = shuffle(&items, &["c", "b", "a"], 0, 1);
    assert_eq!(ids(&out.events), ["new"]);
}

#[test]
fn ids_no_longer_in_the_pool_are_ignored() {
    // `gone` and `old` do not count toward the three items cooling down.
    let items = pool(&["a", "b", "c", "d"]);
    let out = shuffle(&items, &["gone", "c", "old", "b", "gone", "a"], 0, 1);
    assert_eq!(ids(&out.events), ["d"]);

    // History made only of unknown ids is no history.
    let start = at(2026, 6, 1, 12, 0);
    assert_eq!(
        shuffle(&items, &["gone", "old"], start, start + DAY),
        shuffle(&items, &[], start, start + DAY)
    );
}

#[test]
fn a_two_item_shuffle_alternates() {
    let items = pool(&["a", "b"]);
    let out = shuffle(&items, &["b"], 0, 4 * 60 * MINUTE);
    assert_eq!(&ids(&out.events)[..4], ["a", "b", "a", "b"]);
}

#[test]
fn a_three_item_shuffle_rotates_once_its_history_is_full() {
    // N = 3, k = 1: two items are cooling down, so one is eligible.
    let items = pool(&["a", "b", "c"]);
    let out = shuffle(&items, &["b", "a"], 0, 6 * 60 * MINUTE);
    assert_eq!(&ids(&out.events)[..6], ["c", "a", "b", "c", "a", "b"]);
}

#[test]
fn a_shuffle_skips_zero_runtime_items() {
    // `zero` does not count toward N: four playable items rotate.
    let items = vec![
        item("a", 20),
        item("zero", 0),
        item("b", 25),
        item("c", 30),
        item("d", 35),
    ];
    let start = at(2026, 6, 1, 12, 0);
    let out = shuffle(&items, &[], start, start + DAY);
    let played = ids(&out.events);
    assert!(!played.contains(&"zero"));
    assert_cooldown_holds(&played, 4);
    assert_eq!(out.notes, [Note::SkippedZeroRuntime { count: 1 }]);
}

#[test]
fn a_one_item_shuffle_repeats_that_item() {
    let items = pool(&["only"]);
    let out = generate(
        &params(Mode::Shuffle),
        &items,
        &resume("only"),
        0,
        60 * MINUTE,
    )
    .unwrap();
    assert_eq!(ids(&out.events), ["only", "only", "only"]);
}

#[test]
fn seven_days_across_a_daylight_saving_change_have_no_gaps() {
    let items = pool(&["a", "b", "c", "d", "e"]);
    // A week through the 25-hour day, and one through the 23-hour day.
    for start in [at(2026, 10, 30, 5, 0), at(2026, 3, 6, 6, 0)] {
        for mode in [Mode::Sequential, Mode::Shuffle] {
            let out = generate(
                &params(mode),
                &items,
                &Resume::default(),
                start,
                start + HORIZON_MS,
            )
            .unwrap();
            assert_covers(&out.events, start, start + HORIZON_MS);
            if mode == Mode::Shuffle {
                assert_cooldown_holds(&ids(&out.events), 5);
            }
        }
    }
}

// The wrapper.

fn setup() -> Store {
    Store::open_in_memory().unwrap()
}

#[test]
fn nothing_is_generated_while_the_clock_is_not_trusted() {
    let items = pool(&["a", "b"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);

    let result = extend(
        &mut store,
        &channel,
        &items,
        &[],
        at(1970, 1, 1, 0, 0),
        false,
    )
    .unwrap();

    assert_eq!(result, Extended::ClockNotTrusted);
    assert!(
        store
            .events_between(3, i64::MIN, i64::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.last_program_before(3, i64::MAX).unwrap(), None);
}

#[test]
fn an_extend_stores_seven_days_with_no_gaps_and_ends_on_the_last_program() {
    let items = pool(&["a", "b", "c", "d"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);

    let result = extend(&mut store, &channel, &items, &[], now, true).unwrap();

    let Extended::Generated {
        from_ms,
        to_ms,
        written,
        notes,
    } = result
    else {
        panic!("expected events, got {result:?}");
    };
    assert_eq!((from_ms, to_ms), (now, now + HORIZON_MS));
    assert!(notes.is_empty());
    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_eq!(stored.len(), written);
    assert_covers(&stored, now, now + HORIZON_MS);
    assert_eq!(
        store.last_program_before(3, i64::MAX).unwrap().as_ref(),
        Some(&stored.last().unwrap().media_id)
    );
}

/// A stored event of `kind` for `media` covering `[start_ms, end_ms)`.
fn stored(start_ms: i64, end_ms: i64, media: &str, kind: EventKind) -> Event {
    Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media),
        media_offset_ms: 0,
        media_duration_ms: (end_ms - start_ms) as u64,
        kind,
    }
}

#[test]
fn a_sequential_extend_continues_after_the_last_stored_program() {
    let items = pool(&["a", "b", "c", "d"]);
    let mut store = setup();
    let now = at(2026, 6, 1, 12, 0);
    store
        .write_events(
            3,
            &[stored(
                now - 120 * MINUTE,
                now - 60 * MINUTE,
                "b",
                EventKind::Program,
            )],
        )
        .unwrap();

    extend(
        &mut store,
        &channel(3, Mode::Sequential),
        &items,
        &[],
        now,
        true,
    )
    .unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_eq!(&ids(&stored)[..5], ["c", "d", "a", "b", "c"]);
}

#[test]
fn the_resume_point_ignores_a_break_after_the_last_program() {
    let items = pool(&["a", "b", "c", "d"]);
    let mut store = setup();
    let now = at(2026, 6, 1, 12, 0);
    store
        .write_events(
            3,
            &[
                stored(
                    now - 120 * MINUTE,
                    now - 60 * MINUTE,
                    "b",
                    EventKind::Program,
                ),
                stored(now - 60 * MINUTE, now - 58 * MINUTE, "ad", EventKind::Break),
            ],
        )
        .unwrap();

    extend(
        &mut store,
        &channel(3, Mode::Sequential),
        &items,
        &[],
        now,
        true,
    )
    .unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_eq!(ids(&stored)[0], "c");
}

#[test]
fn a_sequential_channel_resumes_where_it_stopped_after_a_long_gap() {
    let items = pool(&["a", "b", "c"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let then = at(2026, 6, 1, 12, 0);
    extend(&mut store, &channel, &items, &[], then, true).unwrap();
    let last_old = store.last_program_before(3, i64::MAX).unwrap().unwrap();
    let now = then + 30 * DAY;

    extend(&mut store, &channel, &items, &[], now, true).unwrap();

    let first_new = &store.events_between(3, now, now + HORIZON_MS).unwrap()[0];
    let after = items.iter().position(|i| i.id == last_old).unwrap();
    assert_eq!(first_new.media_id, items[(after + 1) % 3].id);
}

#[test]
fn a_reopened_store_continues_where_it_stopped() {
    let items = pool(&["a", "b", "c", "d", "e"]);
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("schedule.db");
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);
    let later = now + 3 * DAY + 37 * MINUTE;

    let mut store = Store::open(&path).unwrap();
    extend(&mut store, &channel, &items, &[], now, true).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    extend(&mut store, &channel, &items, &[], later, true).unwrap();

    let stored = store.events_between(3, now, later + HORIZON_MS).unwrap();
    assert_covers(&stored, now, later + HORIZON_MS);
    let played = ids(&stored);
    for pair in played.windows(2) {
        let next = items.iter().position(|i| i.id.as_str() == pair[0]).unwrap();
        assert_eq!(items[(next + 1) % 5].id.as_str(), pair[1]);
    }
}

#[test]
fn later_extends_continue_the_schedule_without_a_gap_or_a_repeat() {
    let items = pool(&["a", "b", "c", "d", "e", "f"]);
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let mut store = setup();
        let channel = channel(3, mode);
        let now = at(2026, 6, 1, 12, 0);

        extend(&mut store, &channel, &items, &[], now, true).unwrap();
        // Up to date: the same moment again has nothing to add.
        let again = extend(&mut store, &channel, &items, &[], now, true).unwrap();
        assert!(matches!(again, Extended::UpToDate { .. }), "{mode:?}");
        // Three days on, the next run fills the gap at the far end.
        let later = now + 3 * DAY + 37 * MINUTE;
        extend(&mut store, &channel, &items, &[], later, true).unwrap();

        let stored = store.events_between(3, now, later + HORIZON_MS).unwrap();
        assert_covers(&stored, now, later + HORIZON_MS);
        let played = ids(&stored);
        match mode {
            Mode::Shuffle => assert_cooldown_holds(&played, 6),
            _ => {
                for pair in played.windows(2) {
                    let next = items.iter().position(|i| i.id.as_str() == pair[0]).unwrap();
                    assert_eq!(items[(next + 1) % 6].id.as_str(), pair[1]);
                }
            }
        }
    }
}

#[test]
fn a_shuffle_extended_twice_matches_one_unbroken_run() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"]);
    let mut store = setup();
    let channel = channel(3, Mode::Shuffle);
    let now = at(2026, 6, 1, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;

    extend(&mut store, &channel, &items, &[], now, true).unwrap();
    extend(&mut store, &channel, &items, &[], later, true).unwrap();

    let whole = shuffle(&items, &[], now, later + HORIZON_MS);
    let stored = store.events_between(3, now, later + HORIZON_MS).unwrap();
    assert_eq!(stored, whole.events);
}

#[test]
fn a_shuffle_extend_reads_back_past_ids_no_longer_in_the_pool() {
    // 40 items of 3 days: N - k = 30, and one page is 80 rows. The newest 60
    // stored programs are no longer in the pool, so the 30 cooling items lie
    // beyond the first page.
    let items: Vec<MediaItem> = (0..40)
        .map(|i| item(&format!("p{i}"), 3 * 24 * 60))
        .collect();
    let mut history: Vec<String> = (0..30).map(|i| format!("p{i}")).collect();
    history.extend((0..60).map(|i| format!("x{i}")));
    let mut store = setup();
    let channel = channel(3, Mode::Shuffle);
    let now = at(2026, 6, 1, 12, 0);
    let count = i64::try_from(history.len()).unwrap();
    let old: Vec<Event> = (0..count)
        .zip(&history)
        .map(|(i, id)| {
            let start = now - (count - i) * 30 * MINUTE;
            stored(start, start + 30 * MINUTE, id, EventKind::Program)
        })
        .collect();
    store.write_events(3, &old).unwrap();

    extend(&mut store, &channel, &items, &[], now, true).unwrap();
    let first_end = store
        .events_between(3, now, i64::MAX)
        .unwrap()
        .last()
        .unwrap()
        .end_ms;
    extend(
        &mut store,
        &channel,
        &items,
        &[],
        first_end - HORIZON_MS + 1,
        true,
    )
    .unwrap();
    let stored_events = store.events_between(3, now, i64::MAX).unwrap();
    let end = stored_events.last().unwrap().end_ms;

    let recent: Vec<&str> = history.iter().rev().map(String::as_str).collect();
    let whole = shuffle(&items, &recent, now, end);
    assert_eq!(stored_events, whole.events);
    // p0..p29 are all cooling down, so the first pick is one of p30..p39.
    let first: usize = whole.events[0].media_id.as_str()[1..].parse().unwrap();
    assert!(first >= 30, "{first}");
}

#[test]
fn an_extend_after_a_long_gap_starts_at_now() {
    let items = pool(&["a", "b", "c"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let then = at(2026, 6, 1, 12, 0);
    extend(&mut store, &channel, &items, &[], then, true).unwrap();
    let now = then + 30 * DAY;

    extend(&mut store, &channel, &items, &[], now, true).unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_covers(&stored, now, now + HORIZON_MS);
}

#[test]
fn an_unplayable_library_writes_nothing_and_reports_it() {
    let items = [item("a", 0)];
    let mut store = setup();
    let now = at(2026, 6, 1, 12, 0);

    let result = extend(
        &mut store,
        &channel(3, Mode::Shuffle),
        &items,
        &[],
        now,
        true,
    )
    .unwrap();

    assert_eq!(
        result,
        Extended::Generated {
            from_ms: now,
            to_ms: now + HORIZON_MS,
            written: 0,
            notes: vec![Note::SkippedZeroRuntime { count: 1 }, Note::NoPlayableItems],
        }
    );
    assert_eq!(store.last_program_before(3, i64::MAX).unwrap(), None);
    assert!(
        store
            .events_between(3, i64::MIN, i64::MAX)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_blocks_channel_is_refused() {
    let mut store = setup();
    let mut channel = channel(3, Mode::Sequential);
    channel.plan = Plan::Blocks(Vec::new());

    let result = extend(&mut store, &channel, &pool(&["a"]), &[], 0, true);

    assert!(matches!(result, Err(ScheduleError::UnsupportedPlan)));
}

#[test]
fn a_resolved_collection_pool_schedules_only_its_items() {
    let media = pool(&["a", "b", "c", "d"])
        .into_iter()
        .fold(FakeMediaSource::new(), FakeMediaSource::with_item)
        .with_collection("Cartoons", &["d", "b"]);
    let mut channel = channel(3, Mode::Sequential);
    channel.plan = Plan::Fixed {
        source: Source::JellyfinCollection("Cartoons".into()),
        mode: Mode::Sequential,
    };
    let Plan::Fixed { source, .. } = &channel.plan else {
        unreachable!()
    };
    let items = resolve_pool(channel.number, source, &media).unwrap();
    let mut store = setup();
    let now = at(2026, 6, 1, 12, 0);

    extend(&mut store, &channel, &items, &[], now, true).unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_eq!(ids(&stored[..4]), ["d", "b", "d", "b"]);
    assert!(ids(&stored).iter().all(|id| ["d", "b"].contains(id)));
}

// Breaks.

const SECOND: u64 = 1_000;

fn clip(name: &str, seconds: u64) -> Clip {
    Clip {
        path: PathBuf::from(format!("/clips/{name}.mp4")),
        duration: Duration::from_secs(seconds),
    }
}

fn clip_id(name: &str) -> String {
    format!("/clips/{name}.mp4")
}

fn plan<'a>(every_min: u64, length_s: u64, clips: &'a [Clip], pad: &'a [Clip]) -> BreakPlan<'a> {
    BreakPlan {
        every_ms: every_min * 60 * SECOND,
        length_ms: length_s * SECOND,
        clips,
        pad,
    }
}

fn with_breaks(mode: Mode, breaks: BreakPlan<'_>) -> Params<'_> {
    Params {
        channel: 3,
        mode,
        breaks: Some(breaks),
    }
}

/// The program events that start a program: a cut program counts once.
fn programs(events: &[Event]) -> Vec<&str> {
    events
        .iter()
        .filter(|e| e.kind == EventKind::Program && e.media_offset_ms == 0)
        .map(|e| e.media_id.as_str())
        .collect()
}

/// `(kind, media, offset, length)` of each event, for compact assertions.
fn shape(events: &[Event]) -> Vec<(EventKind, String, u64, i64)> {
    events
        .iter()
        .map(|e| {
            (
                e.kind,
                e.media_id.to_string(),
                e.media_offset_ms,
                e.end_ms - e.start_ms,
            )
        })
        .collect()
}

#[test]
fn a_22_minute_program_with_two_2_minute_breaks_occupies_26_minutes() {
    let items = [item("show", 22)];
    let ads = [clip("ad1", 60), clip("ad2", 60)];
    let params = with_breaks(Mode::Sequential, plan(8, 120, &ads, &[]));

    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();

    use EventKind::{Break, Program};
    let m = MINUTE;
    let (a1, a2) = (clip_id("ad1"), clip_id("ad2"));
    let show = "show".to_string();
    assert_eq!(
        shape(&out.events),
        [
            (Program, show.clone(), 0, 8 * m),
            (Break, a1.clone(), 0, m),
            (Break, a2.clone(), 0, m),
            (Program, show.clone(), 8 * 60_000, 8 * m),
            (Break, a1, 0, m),
            (Break, a2, 0, m),
            (Program, show, 16 * 60_000, 6 * m),
        ]
    );
    assert_eq!(out.events.last().unwrap().end_ms, 26 * MINUTE);
    for event in out.events.iter().filter(|e| e.kind == Program) {
        assert_eq!(event.media_duration_ms, 22 * 60_000);
    }
    assert!(out.notes.is_empty());
}

#[test]
fn a_cut_snaps_to_the_nearest_chapter_within_60_seconds() {
    let chapter = |s: u64| Chapter {
        start_ms: s * SECOND,
        name: None,
    };
    let mut show = item("show", 30);
    // 8:40 and 9:30 are near 9:00; 8:40 is nearer. 19:01 is 61 s from 18:00,
    // too far. 27:05 is the only mark near the third cut.
    show.chapters = vec![
        chapter(0),
        chapter(520),
        chapter(570),
        chapter(1141),
        chapter(1625),
    ];
    assert_eq!(
        cut_points(&show, 9 * 60 * SECOND),
        [520 * SECOND, 18 * 60 * SECOND, 1625 * SECOND]
    );

    // A tie goes to the earlier mark.
    show.chapters = vec![chapter(510), chapter(570)];
    assert_eq!(cut_points(&show, 9 * 60 * SECOND)[0], 510 * SECOND);

    // Exactly 60 s away still snaps; a mark at the end does not.
    show.chapters = vec![chapter(600), chapter(1800)];
    assert_eq!(
        cut_points(&show, 9 * 60 * SECOND),
        [600 * SECOND, 18 * 60 * SECOND, 27 * 60 * SECOND]
    );

    // No chapters: the cuts stay at the interval, and none at the end.
    show.chapters.clear();
    assert_eq!(
        cut_points(&show, 10 * 60 * SECOND),
        [10 * 60 * SECOND, 20 * 60 * SECOND]
    );
    assert!(cut_points(&item("short", 9), 10 * 60 * SECOND).is_empty());
}

#[test]
fn snapped_cuts_move_the_program_offsets() {
    let mut show = item("show", 22);
    show.chapters = vec![Chapter {
        start_ms: 7 * 60_000 + 30_000,
        name: Some("Act 2".into()),
    }];
    let ads = [clip("ad", 120)];
    let params = with_breaks(Mode::Sequential, plan(8, 120, &ads, &[]));

    let out = generate(&params, &[show], &Resume::default(), 0, 1).unwrap();

    let segments: Vec<(u64, i64)> = out
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Program)
        .map(|e| (e.media_offset_ms, e.end_ms - e.start_ms))
        .collect();
    assert_eq!(
        segments,
        [
            (0, 7 * MINUTE + 30_000),
            (450_000, 8 * MINUTE + 30_000),
            (960_000, 6 * MINUTE)
        ]
    );
}

#[test]
fn a_break_uses_whole_clips_and_pads_the_rest() {
    let items = [item("show", 20)];
    // 50 + 50 = 100 s; a third 50 s clip would overrun the 120 s break.
    let ads = [clip("a", 50), clip("b", 50), clip("c", 50)];
    let pad = [clip("ident", 5), clip("slate", 30), clip("long", 300)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &ads, &pad));

    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();

    let brk: Vec<&Event> = out
        .events
        .iter()
        .filter(|e| e.kind != EventKind::Program)
        .collect();
    let lengths: Vec<i64> = brk.iter().map(|e| e.end_ms - e.start_ms).collect();
    assert_eq!(lengths, [50_000, 50_000, 20_000]);
    for ad in &brk[..2] {
        assert_eq!(ad.kind, EventKind::Break);
        assert_eq!(ad.media_duration_ms, 50_000, "an ad is never cut");
    }
    // The pad is the shortest filler clip that covers the 20 s left.
    assert_eq!(brk[2].kind, EventKind::Filler);
    assert_eq!(brk[2].media_id.as_str(), clip_id("slate"));
    assert_eq!(brk[2].media_duration_ms, 30_000);
    assert_eq!(out.events.last().unwrap().end_ms, 22 * MINUTE);
}

#[test]
fn a_short_filler_pool_pads_with_its_longest_clip() {
    let items = [item("show", 20)];
    let pad = [clip("ident", 5), clip("slate", 10), clip("other", 10)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &[], &pad));

    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();

    let brk = &out.events[1];
    assert_eq!(brk.kind, EventKind::Filler);
    assert_eq!(brk.media_id.as_str(), clip_id("slate"));
    assert_eq!(brk.end_ms - brk.start_ms, 120_000);
}

#[test]
fn a_clip_length_is_rounded_up_to_a_whole_millisecond() {
    let items = [item("show", 20)];
    let ads = [Clip {
        path: PathBuf::from("/clips/odd.mp4"),
        duration: Duration::from_micros(29_970_100),
    }];
    let params = with_breaks(Mode::Sequential, plan(10, 30, &ads, &[]));

    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();

    assert_eq!(out.events[1].end_ms - out.events[1].start_ms, 29_971);
}

#[test]
fn without_filler_a_break_is_short_and_reported() {
    let items = [item("show", 20)];
    let ads = [clip("a", 50)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &ads, &[]));

    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(out.events.len(), 3);
    assert_eq!(out.events[1].end_ms - out.events[1].start_ms, 50_000);
    assert_eq!(out.notes, [Note::ShortBreaks { count: 1 }]);

    // With nothing to show, the cut is left out and the program runs whole.
    let params = with_breaks(Mode::Sequential, plan(10, 120, &[], &[]));
    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(
        shape(&out.events),
        [(EventKind::Program, "show".into(), 0, 20 * MINUTE)]
    );
    assert_eq!(out.notes, [Note::ShortBreaks { count: 1 }]);
}

#[test]
fn break_clips_rotate_and_play_once_per_break() {
    let items = [item("show", 40)];
    let ads = [clip("a", 60), clip("b", 60), clip("c", 60)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &ads, &[]));

    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();
    let played: Vec<&str> = out
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Break)
        .map(|e| e.media_id.as_str())
        .collect();
    let (a, b, c) = (clip_id("a"), clip_id("b"), clip_id("c"));
    assert_eq!(played, [&a, &b, &c, &a, &b, &c]);

    // The rotation continues after the last stored break clip, and an id no
    // longer in the pool starts it again at the first clip.
    let resume = |last: &str| Resume {
        last_break: Some((-MINUTE, MediaId::new(last))),
        ..Resume::default()
    };
    let out = generate(&params, &items, &resume(&b), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), c);
    let out = generate(&params, &items, &resume("/clips/gone.mp4"), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), a);

    // One 60 s clip does not play twice in a 120 s break; filler pads it.
    let one = [clip("a", 60)];
    let pad = [clip("slate", 60)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &one, &pad));
    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(
        shape(&out.events[1..3]),
        [
            (EventKind::Break, a, 0, MINUTE),
            (EventKind::Filler, clip_id("slate"), 0, MINUTE)
        ]
    );
}

#[test]
fn a_shuffle_with_breaks_resumed_at_a_program_matches_an_unbroken_run() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g"]);
    let ads = [clip("x", 30), clip("y", 45), clip("z", 60)];
    let pad = [clip("slate", 90)];
    let params = with_breaks(Mode::Shuffle, plan(11, 120, &ads, &pad));
    let start = at(2026, 6, 1, 12, 0);
    let end = start + 2 * DAY;
    let whole = generate(&params, &items, &Resume::default(), start, end).unwrap();
    assert_cooldown_holds(&programs(&whole.events), 7);

    let starts: Vec<usize> = (0..whole.events.len())
        .filter(|&i| {
            let e = &whole.events[i];
            e.kind == EventKind::Program && e.media_offset_ms == 0
        })
        .collect();
    for cut in [starts[1], starts[4], starts[13]] {
        let boundary = whole.events[cut].start_ms;
        let first = generate(&params, &items, &Resume::default(), start, boundary).unwrap();
        assert_eq!(first.events, whole.events[..cut]);
        // What `extend` reads from the store: the program rows newest first,
        // and the last break clip.
        let resume = Resume {
            recent: first
                .events
                .iter()
                .rev()
                .filter(|e| e.kind == EventKind::Program)
                .map(|e| e.media_id.clone())
                .collect(),
            last_break: first
                .events
                .iter()
                .rev()
                .find(|e| e.kind == EventKind::Break)
                .map(|e| (e.start_ms, e.media_id.clone())),
        };
        let rest = generate(&params, &items, &resume, boundary, end).unwrap();
        assert_eq!(rest.events, whole.events[cut..], "cut at {cut}");
    }
}

/// The break clip ids of `events`, one list per break.
fn breaks_of(events: &[Event]) -> Vec<Vec<&str>> {
    let mut out: Vec<Vec<&str>> = Vec::new();
    let mut open = false;
    for e in events {
        if e.kind == EventKind::Break {
            if !open {
                out.push(Vec::new());
            }
            out.last_mut().unwrap().push(e.media_id.as_str());
        }
        open = e.kind != EventKind::Program;
    }
    out
}

#[test]
fn clips_sharing_a_media_id_count_once() {
    let items = [item("show", 40)];
    // `a` twice, as one file reached through two links.
    let ads = [clip("a", 60), clip("b", 60), clip("a", 60), clip("c", 60)];
    let (a, b, c) = (clip_id("a"), clip_id("b"), clip_id("c"));

    // A break long enough for every entry still plays `a` once.
    let params = with_breaks(Mode::Sequential, plan(10, 240, &ads, &[]));
    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(breaks_of(&out.events)[0], [&a, &b, &c]);

    // One clip per break: the rotation is a, b, c, with the duplicate left
    // out, and a run resumed after any break matches an unbroken one.
    let params = with_breaks(Mode::Sequential, plan(10, 60, &ads, &[]));
    let items = pool(&["p", "q", "r", "s"]);
    let end = 6 * 60 * MINUTE;
    let whole = generate(&params, &items, &Resume::default(), 0, end).unwrap();
    let played: Vec<&str> = breaks_of(&whole.events).concat();
    assert_eq!(played[..6], [&a, &b, &c, &a, &b, &c]);
    let starts: Vec<usize> = (0..whole.events.len())
        .filter(|&i| {
            let e = &whole.events[i];
            e.kind == EventKind::Program && e.media_offset_ms == 0
        })
        .collect();
    for &cut in &starts[1..5] {
        let boundary = whole.events[cut].start_ms;
        let first = &whole.events[..cut];
        let resume = Resume {
            recent: first
                .iter()
                .rev()
                .filter(|e| e.kind == EventKind::Program)
                .map(|e| e.media_id.clone())
                .collect(),
            last_break: first
                .iter()
                .rev()
                .find(|e| e.kind == EventKind::Break)
                .map(|e| (e.start_ms, e.media_id.clone())),
        };
        let rest = generate(&params, &items, &resume, boundary, end).unwrap();
        assert_eq!(rest.events, whole.events[cut..], "cut at {cut}");
    }
}

#[test]
fn the_rotation_starts_over_after_a_day_without_breaks() {
    let items = [item("show", 20)];
    let ads = [clip("a", 60), clip("b", 60), clip("c", 60)];
    let params = with_breaks(Mode::Sequential, plan(10, 60, &ads, &[]));
    let first_break = 10 * MINUTE;
    let resume = |started: i64| Resume {
        last_break: Some((started, MediaId::new(clip_id("a")))),
        ..Resume::default()
    };

    // Exactly a day before the next break still continues after `a`.
    let out = generate(&params, &items, &resume(first_break - DAY), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), clip_id("b"));
    // Any longer and the rotation starts again at the first clip, which is
    // what `extend` gets when it finds no break in the last day.
    let out = generate(&params, &items, &resume(first_break - DAY - 1), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), clip_id("a"));
    let out = generate(&params, &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), clip_id("a"));
    assert_eq!(ROTATION_MEMORY_MS, DAY);
}

fn channel_with_breaks(mode: Mode) -> Channel {
    let mut channel = channel(3, mode);
    channel.breaks = Some(Breaks {
        every: Duration::from_secs(11 * 60),
        length: Duration::from_secs(120),
        pool: "ads".into(),
    });
    channel.filler = Some("filler".into());
    channel
}

fn clip_pools() -> Vec<ClipPool> {
    vec![
        ClipPool::new(
            "ads",
            "/clips",
            vec![clip("x", 30), clip("y", 45), clip("z", 60)],
            Vec::new(),
        ),
        ClipPool::new("filler", "/clips", vec![clip("slate", 90)], Vec::new()),
    ]
}

#[test]
fn an_extend_with_breaks_twice_matches_one_unbroken_run() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let pools = clip_pools();
    let now = at(2026, 6, 1, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let mut store = setup();
        let channel = channel_with_breaks(mode);

        extend(&mut store, &channel, &items, &pools, now, true).unwrap();
        extend(&mut store, &channel, &items, &pools, later, true).unwrap();

        let params = with_breaks(mode, plan(11, 120, pools[0].clips(), pools[1].clips()));
        let end = later + HORIZON_MS;
        let whole = generate(&params, &items, &Resume::default(), now, end).unwrap();
        // A whole program is stored, so read past `end` to its last segment.
        let stored = store.events_between(3, now, i64::MAX).unwrap();
        assert_eq!(stored, whole.events, "{mode:?}");
        assert!(stored.iter().any(|e| e.kind == EventKind::Break));
        if mode == Mode::Shuffle {
            assert_cooldown_holds(&programs(&stored), 8);
        }
    }
}

#[test]
fn an_extend_without_its_clip_pools_reports_short_breaks() {
    let items = pool(&["a", "b"]);
    let mut store = setup();
    let channel = channel_with_breaks(Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);

    let result = extend(&mut store, &channel, &items, &[], now, true).unwrap();

    let Extended::Generated { notes, .. } = result else {
        panic!("expected events, got {result:?}");
    };
    assert!(matches!(notes[..], [Note::ShortBreaks { .. }]), "{notes:?}");
    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert!(stored.iter().all(|e| e.kind == EventKind::Program));
}
