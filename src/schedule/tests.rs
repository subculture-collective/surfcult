use std::path::PathBuf;
use std::time::Duration;

use chrono::{TimeZone, Utc};

use super::blocks::{Span, spans};
use super::breaks::cut_points;
use super::*;
use crate::clips::{Clip, Pool as ClipPool};
use crate::config::{
    Block, Breaks, Channel, Day, Lineup, Mode, Plan, Rotation, Source, Tz, WeightedSource,
};
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
            sources: vec![WeightedSource::new(Source::JellyfinLibrary("All".into()))],
            mode,
        },
        grid: None,
        breaks: None,
        filler: None,
        sign_off: None,
    }
}

static NO_SOURCE: Source = Source::JellyfinLibrary(String::new());

/// `items` as the one source of a single-source lineup.
fn single(items: &[MediaItem]) -> [SourcePool<'_>; 1] {
    [SourcePool {
        source: &NO_SOURCE,
        weight: 1,
        items,
    }]
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
        ..Resume::default()
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
        &single(&items),
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
        let run = || {
            generate(
                &params(mode),
                &single(&items),
                &resume("b"),
                start,
                start + 7 * DAY,
            )
            .unwrap()
        };
        assert_eq!(run(), run(), "{mode:?}");
    }
}

#[test]
fn sequential_continues_after_the_cursor_and_wraps() {
    let items = pool(&["a", "b", "c", "d"]);
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &params(Mode::Sequential),
        &single(&items),
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
    let out = generate(
        &params(Mode::Sequential),
        &single(&items),
        &Resume::default(),
        0,
        1,
    )
    .unwrap();
    assert_eq!(ids(&out.events), ["a"]);
    assert_eq!(out.cursor.as_ref().map(MediaId::as_str), Some("a"));
}

#[test]
fn sequential_restarts_and_says_so_when_the_cursor_item_vanished() {
    let items = pool(&["a", "b", "c"]);

    let out = generate(
        &params(Mode::Sequential),
        &single(&items),
        &resume("gone"),
        0,
        1,
    )
    .unwrap();

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
        &single(&items),
        &resume("a"),
        0,
        60 * MINUTE,
    )
    .unwrap();

    assert_eq!(ids(&out.events), ["c", "a", "c"]);
    assert_eq!(out.notes, [Note::SkippedZeroRuntime { count: 1 }]);

    let after_zero = generate(
        &params(Mode::Sequential),
        &single(&items),
        &resume("zero"),
        0,
        1,
    )
    .unwrap();
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
        let none = generate(&params(mode), &single(&[]), &resume("x"), 0, DAY).unwrap();
        assert!(none.events.is_empty());
        assert_eq!(none.cursor.as_ref().map(MediaId::as_str), Some("x"));
        assert_eq!(none.notes, [Note::NoPlayableItems]);

        let zeros = vec![item("a", 0), item("b", 0)];
        let out = generate(&params(mode), &single(&zeros), &Resume::default(), 0, DAY).unwrap();
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
    let out = generate(
        &params(Mode::Sequential),
        &single(&items),
        &resume("a"),
        100,
        100,
    )
    .unwrap();
    assert!(out.events.is_empty());
    assert_eq!(out.cursor.as_ref().map(MediaId::as_str), Some("a"));
}

#[test]
fn a_duplicate_id_in_the_pool_is_used_once() {
    let items = vec![item("a", 20), item("b", 20), item("a", 45)];
    let out = generate(
        &params(Mode::Sequential),
        &single(&items),
        &Resume::default(),
        0,
        80 * MINUTE,
    )
    .unwrap();
    assert_eq!(ids(&out.events), ["a", "b", "a", "b"]);
    assert_eq!(out.events[0].media_duration_ms, 20 * 60_000);
}

/// `n` items of 30 minutes each, named `i0`, `i1`, ...
fn uniform(n: usize) -> Vec<MediaItem> {
    (0..n).map(|i| item(&format!("i{i}"), 30)).collect()
}

fn shuffle(items: &[MediaItem], recent: &[&str], start: i64, end: i64) -> Generated {
    let resume = Resume {
        recent: recent.iter().map(|id| MediaId::new(*id)).collect(),
        ..Resume::default()
    };
    generate(&params(Mode::Shuffle), &single(items), &resume, start, end).unwrap()
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
        let out = generate(
            &params,
            &single(&items),
            &Resume::default(),
            start,
            start + DAY,
        )
        .unwrap();
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
        &single(&items),
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
                &single(&items),
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

/// `extend` for a channel with one source, whose pool is `items`, in UTC.
fn extend_fixed(
    store: &mut Store,
    channel: &Channel,
    items: &[MediaItem],
    clips: &[ClipPool],
    now_ms: i64,
    clock_trusted: bool,
) -> Result<Extended, ScheduleError> {
    let pools: Vec<(Source, Vec<MediaItem>)> = channel
        .plan
        .sources()
        .into_iter()
        .map(|source| (source.clone(), items.to_vec()))
        .collect();
    extend(
        store,
        channel,
        &pools,
        clips,
        Tz::UTC,
        now_ms,
        clock_trusted,
    )
}

#[test]
fn nothing_is_generated_while_the_clock_is_not_trusted() {
    let items = pool(&["a", "b"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);

    let result = extend_fixed(
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

    let result = extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();

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

    extend_fixed(
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

    extend_fixed(
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
    extend_fixed(&mut store, &channel, &items, &[], then, true).unwrap();
    let last_old = store.last_program_before(3, i64::MAX).unwrap().unwrap();
    let now = then + 30 * DAY;

    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();

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
    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
    drop(store);
    let mut store = Store::open(&path).unwrap();
    extend_fixed(&mut store, &channel, &items, &[], later, true).unwrap();

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

        extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
        // Up to date: the same moment again has nothing to add.
        let again = extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
        assert!(matches!(again, Extended::UpToDate { .. }), "{mode:?}");
        // Three days on, the next run fills the gap at the far end.
        let later = now + 3 * DAY + 37 * MINUTE;
        extend_fixed(&mut store, &channel, &items, &[], later, true).unwrap();

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

    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
    extend_fixed(&mut store, &channel, &items, &[], later, true).unwrap();

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

    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
    let first_end = store
        .events_between(3, now, i64::MAX)
        .unwrap()
        .last()
        .unwrap()
        .end_ms;
    extend_fixed(
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
    extend_fixed(&mut store, &channel, &items, &[], then, true).unwrap();
    let now = then + 30 * DAY;

    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_covers(&stored, now, now + HORIZON_MS);
}

#[test]
fn an_unplayable_library_writes_nothing_and_reports_it() {
    let items = [item("a", 0)];
    let mut store = setup();
    let now = at(2026, 6, 1, 12, 0);

    let result = extend_fixed(
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
fn a_resolved_collection_pool_schedules_only_its_items() {
    let media = pool(&["a", "b", "c", "d"])
        .into_iter()
        .fold(FakeMediaSource::new(), FakeMediaSource::with_item)
        .with_collection("Cartoons", &["d", "b"]);
    let mut channel = channel(3, Mode::Sequential);
    channel.plan = Plan::Fixed {
        sources: vec![WeightedSource::new(Source::JellyfinCollection(
            "Cartoons".into(),
        ))],
        mode: Mode::Sequential,
    };
    let Plan::Fixed { sources, .. } = &channel.plan else {
        unreachable!()
    };
    let items = resolve_pool(channel.number, &sources[0].source, &media).unwrap();
    let mut store = setup();
    let now = at(2026, 6, 1, 12, 0);

    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();

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
        rotation: Rotation::Order,
        repeat_after_ms: None,
        idents: &[],
        filler: pad,
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

    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();

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

    let out = generate(&params, &single(&[show]), &Resume::default(), 0, 1).unwrap();

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

    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();

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

    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();

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

    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();

    assert_eq!(out.events[1].end_ms - out.events[1].start_ms, 29_971);
}

#[test]
fn without_filler_a_break_is_short_and_reported() {
    let items = [item("show", 20)];
    let ads = [clip("a", 50)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &ads, &[]));

    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_eq!(out.events.len(), 3);
    assert_eq!(out.events[1].end_ms - out.events[1].start_ms, 50_000);
    assert_eq!(out.notes, [Note::ShortBreaks { count: 1 }]);

    // With nothing to show, the cut is left out and the program runs whole.
    let params = with_breaks(Mode::Sequential, plan(10, 120, &[], &[]));
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
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

    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
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
    let out = generate(&params, &single(&items), &resume(&b), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), c);
    let out = generate(&params, &single(&items), &resume("/clips/gone.mp4"), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), a);

    // One 60 s clip does not play twice in a 120 s break; filler pads it.
    let one = [clip("a", 60)];
    let pad = [clip("slate", 60)];
    let params = with_breaks(Mode::Sequential, plan(10, 120, &one, &pad));
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
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
    let whole = generate(&params, &single(&items), &Resume::default(), start, end).unwrap();
    assert_cooldown_holds(&programs(&whole.events), 7);

    let starts: Vec<usize> = (0..whole.events.len())
        .filter(|&i| {
            let e = &whole.events[i];
            e.kind == EventKind::Program && e.media_offset_ms == 0
        })
        .collect();
    for cut in [starts[1], starts[4], starts[13]] {
        let boundary = whole.events[cut].start_ms;
        let first = generate(
            &params,
            &single(&items),
            &Resume::default(),
            start,
            boundary,
        )
        .unwrap();
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
            ..Resume::default()
        };
        let rest = generate(&params, &single(&items), &resume, boundary, end).unwrap();
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
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_eq!(breaks_of(&out.events)[0], [&a, &b, &c]);

    // One clip per break: the rotation is a, b, c, with the duplicate left
    // out, and a run resumed after any break matches an unbroken one.
    let params = with_breaks(Mode::Sequential, plan(10, 60, &ads, &[]));
    let items = pool(&["p", "q", "r", "s"]);
    let end = 6 * 60 * MINUTE;
    let whole = generate(&params, &single(&items), &Resume::default(), 0, end).unwrap();
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
            ..Resume::default()
        };
        let rest = generate(&params, &single(&items), &resume, boundary, end).unwrap();
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
    let out = generate(&params, &single(&items), &resume(first_break - DAY), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), clip_id("b"));
    // Any longer and the rotation starts again at the first clip, which is
    // what `extend` gets when it finds no break in the last day.
    let out = generate(
        &params,
        &single(&items),
        &resume(first_break - DAY - 1),
        0,
        1,
    )
    .unwrap();
    assert_eq!(out.events[1].media_id.as_str(), clip_id("a"));
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_eq!(out.events[1].media_id.as_str(), clip_id("a"));
    assert_eq!(ROTATION_MEMORY_MS, DAY);
}

fn channel_with_breaks(mode: Mode) -> Channel {
    let mut channel = channel(3, mode);
    channel.breaks = Some(Breaks {
        every: Duration::from_secs(11 * 60),
        length: Duration::from_secs(120),
        pool: "ads".into(),
        rotation: Rotation::Order,
        repeat_after: None,
        pad: None,
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

        extend_fixed(&mut store, &channel, &items, &pools, now, true).unwrap();
        extend_fixed(&mut store, &channel, &items, &pools, later, true).unwrap();

        let params = with_breaks(mode, plan(11, 120, pools[0].clips(), pools[1].clips()));
        let end = later + HORIZON_MS;
        let whole = generate(&params, &single(&items), &Resume::default(), now, end).unwrap();
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

    let result = extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();

    let Extended::Generated { notes, .. } = result else {
        panic!("expected events, got {result:?}");
    };
    assert!(matches!(notes[..], [Note::ShortBreaks { .. }]), "{notes:?}");
    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert!(stored.iter().all(|e| e.kind == EventKind::Program));
}

// Break settings: rotation, repeat_after and the pad pool.

fn with_rotation(
    rotation: Rotation,
    repeat_after_ms: Option<i64>,
    plan: BreakPlan<'_>,
) -> BreakPlan<'_> {
    BreakPlan {
        rotation,
        repeat_after_ms,
        ..plan
    }
}

/// Checks that no break clip in `events` starts again less than `limit_ms`
/// after its previous airing started.
fn assert_repeat_limit_holds(events: &[Event], limit_ms: i64) {
    let mut last: std::collections::HashMap<&str, i64> = std::collections::HashMap::new();
    for e in events.iter().filter(|e| e.kind == EventKind::Break) {
        if let Some(previous) = last.insert(e.media_id.as_str(), e.start_ms) {
            assert!(
                e.start_ms - previous >= limit_ms,
                "{} again after {} ms",
                e.media_id,
                e.start_ms - previous
            );
        }
    }
}

#[test]
fn order_rotation_walks_the_pool_and_shuffle_draws_a_new_order_per_break() {
    // Ten hours of program and five 30 s clips that fill each 150 s break.
    let items = [item("show", 600)];
    let ads = ["a", "b", "c", "d", "e"].map(|name| clip(name, 30));
    let all: Vec<String> = ["a", "b", "c", "d", "e"].map(clip_id).to_vec();

    let order = with_breaks(Mode::Sequential, plan(10, 150, &ads, &[]));
    let out = generate(&order, &single(&items), &Resume::default(), 0, 1).unwrap();
    for brk in breaks_of(&out.events) {
        assert_eq!(brk, all);
    }

    let shuffle = with_breaks(
        Mode::Sequential,
        with_rotation(Rotation::Shuffle, None, plan(10, 150, &ads, &[])),
    );
    let out = generate(&shuffle, &single(&items), &Resume::default(), 0, 1).unwrap();
    let breaks = breaks_of(&out.events);
    assert!(breaks.len() > 50);
    // Each break still plays every clip once.
    for brk in &breaks {
        let mut sorted = brk.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, all);
    }
    let orders: std::collections::HashSet<&Vec<&str>> = breaks.iter().collect();
    assert!(orders.len() > 10, "only {} orders", orders.len());

    // The same channel and break starts draw the same orders; another
    // channel draws others.
    assert_eq!(
        generate(&shuffle, &single(&items), &Resume::default(), 0, 1).unwrap(),
        out
    );
    let other = Params {
        channel: 4,
        ..shuffle
    };
    let elsewhere = generate(&other, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_ne!(breaks_of(&elsewhere.events), breaks);
}

#[test]
fn repeat_after_holds_a_clip_back_and_pads_a_break_none_can_fill() {
    // Breaks of one 60 s clip start every 11 minutes: at 10, 21, 32, 43...
    let items = [item("show", 600)];
    let ads = [clip("a", 60), clip("b", 60)];
    let slate = [clip("slate", 60)];
    let (a, b, s) = (clip_id("a"), clip_id("b"), clip_id("slate"));
    let run = |rotation, limit_ms| {
        let plan = with_rotation(rotation, Some(limit_ms), plan(10, 60, &ads, &slate));
        generate(
            &with_breaks(Mode::Sequential, plan),
            &single(&items),
            &Resume::default(),
            0,
            1,
        )
        .unwrap()
    };
    let breaks = |events: &[Event]| -> Vec<(EventKind, String)> {
        events
            .iter()
            .filter(|e| e.kind != EventKind::Program)
            .map(|e| (e.kind, e.media_id.to_string()))
            .collect()
    };
    use EventKind::{Break, Filler};

    // At 32 minutes `a` played 22 minutes ago and `b` 11, so the break is
    // padded rather than break the 25-minute limit.
    let out = run(Rotation::Order, 25 * MINUTE);
    assert_eq!(
        breaks(&out.events)[..6],
        [
            (Break, a.clone()),
            (Break, b.clone()),
            (Filler, s.clone()),
            (Break, a.clone()),
            (Break, b.clone()),
            (Filler, s),
        ]
    );
    assert_repeat_limit_holds(&out.events, 25 * MINUTE);
    assert_eq!(out.notes, []);

    // A clip may play again exactly `repeat_after` after its last airing.
    let out = run(Rotation::Order, 22 * MINUTE);
    assert_eq!(
        breaks(&out.events)[..4],
        [
            (Break, a.clone()),
            (Break, b.clone()),
            (Break, a),
            (Break, b)
        ]
    );

    for limit in [25 * MINUTE, 2 * 60 * MINUTE] {
        let out = run(Rotation::Shuffle, limit);
        assert_repeat_limit_holds(&out.events, limit);
        assert!(out.events.iter().any(|e| e.kind == Filler));
    }
}

#[test]
fn repeat_after_counts_from_the_airings_before_the_run() {
    let items = [item("show", 20)];
    let ads = [clip("a", 60), clip("b", 60)];
    let plan = with_rotation(Rotation::Order, Some(25 * MINUTE), plan(10, 60, &ads, &[]));
    let params = with_breaks(Mode::Sequential, plan);
    // `a` aired 15 minutes before the first break at 10 minutes.
    let resume = Resume {
        aired: vec![(-5 * MINUTE, MediaId::new(clip_id("a")))],
        ..Resume::default()
    };

    let out = generate(&params, &single(&items), &resume, 0, 1).unwrap();

    assert_eq!(out.events[1].media_id.as_str(), clip_id("b"));
}

#[test]
fn the_pad_pool_pads_a_break_before_the_filler_pool_with_idents() {
    let items = [item("show", 20)];
    let ads = [clip("a", 50)];
    let filler = [clip("filler", 70)];
    let pad = |idents| BreakPlan {
        idents,
        ..plan(10, 120, &ads, &filler)
    };
    let gap = |out: &Generated| {
        let e = &out.events[2];
        (
            e.kind,
            e.media_id.to_string(),
            e.end_ms - e.start_ms,
            e.media_duration_ms,
        )
    };

    // The shortest ident at least as long as the 70 s gap.
    let idents = [clip("ident", 5), clip("slate", 90), clip("card", 120)];
    let params = with_breaks(Mode::Sequential, pad(&idents));
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_eq!(out.events[1].kind, EventKind::Break);
    assert_eq!(
        gap(&out),
        (EventKind::Ident, clip_id("slate"), 70_000, 90_000)
    );

    // Else the longest ident, still exactly as long as the gap.
    let idents = [clip("ident", 5), clip("bumper", 10)];
    let params = with_breaks(Mode::Sequential, pad(&idents));
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_eq!(
        gap(&out),
        (EventKind::Ident, clip_id("bumper"), 70_000, 10_000)
    );

    // With no ident the filler pool pads, as filler.
    let params = with_breaks(Mode::Sequential, pad(&[]));
    let out = generate(&params, &single(&items), &Resume::default(), 0, 1).unwrap();
    assert_eq!(
        gap(&out),
        (EventKind::Filler, clip_id("filler"), 70_000, 70_000)
    );
}

/// `channel_with_breaks` with the break pool's rotation and repeat limit set
/// and an `idents` pad pool.
fn channel_with_break_settings(mode: Mode, rotation: Rotation, repeat_after: Duration) -> Channel {
    let mut channel = channel_with_breaks(mode);
    let breaks = channel.breaks.as_mut().unwrap();
    breaks.rotation = rotation;
    breaks.repeat_after = Some(repeat_after);
    breaks.pad = Some("idents".into());
    channel
}

/// `clip_pools` and an `idents` pool.
fn clip_pools_with_idents() -> Vec<ClipPool> {
    let mut pools = clip_pools();
    pools.push(ClipPool::new(
        "idents",
        "/clips",
        vec![clip("ident", 20), clip("card", 45)],
        Vec::new(),
    ));
    pools
}

#[test]
fn break_settings_survive_an_extend_restart_and_a_regeneration() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let pools = clip_pools_with_idents();
    let now = at(2026, 6, 1, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    // With a 60-hour limit the clips air together about every 60 hours, so
    // the last airings before the second extend (about 168 hours in) and
    // before the regeneration's boundary (about 48 hours in) are more than a
    // day old: both must read back past ROTATION_MEMORY_MS.
    for repeat_after in [40 * 60, 60 * 3600] {
        for rotation in [Rotation::Order, Rotation::Shuffle] {
            let limit = Duration::from_secs(repeat_after);
            let channel = channel_with_break_settings(Mode::Shuffle, rotation, limit);
            let label = format!("{rotation:?} {repeat_after} s");
            let mut store = setup();

            extend_fixed(&mut store, &channel, &items, &pools, now, true).unwrap();
            extend_fixed(&mut store, &channel, &items, &pools, later, true).unwrap();

            let plan = BreakPlan {
                idents: pools[2].clips(),
                ..with_rotation(
                    rotation,
                    Some(repeat_after as i64 * 1000),
                    plan(11, 120, pools[0].clips(), pools[1].clips()),
                )
            };
            let params = with_breaks(Mode::Shuffle, plan);
            let end = later + HORIZON_MS;
            let whole = generate(&params, &single(&items), &Resume::default(), now, end).unwrap();
            let stored = all_events(&store);
            assert_eq!(stored, whole.events, "{label}");
            assert_repeat_limit_holds(&stored, repeat_after as i64 * 1000);
            assert!(stored.iter().any(|e| e.kind == EventKind::Break), "{label}");
            assert!(stored.iter().any(|e| e.kind == EventKind::Ident), "{label}");

            // Regenerating two days into the stored schedule with unchanged
            // inputs resumes from the same history and changes nothing.
            let result = regenerate_fixed(&mut store, &channel, &items, &pools, later);
            regenerated_from(&result);
            assert_eq!(all_events(&store), stored, "{label}");
        }
    }
}

// Weighted.

fn tag(name: &str) -> Source {
    Source::JellyfinTag(name.into())
}

fn weighted(source: Source, weight: u32) -> WeightedSource {
    WeightedSource { source, weight }
}

/// Weighted sources over disjoint pools: `a0`..`a2` weigh 1, `b0`..`b3` weigh
/// 3 and `c0`..`c4` weigh 6, all one minute long, so a window of 20,000
/// minutes holds 20,000 picks.
struct Trio {
    sources: [Source; 3],
    pools: [Vec<MediaItem>; 3],
}

impl Trio {
    fn new() -> Self {
        Self {
            sources: [tag("a"), tag("b"), tag("c")],
            pools: [named("a", 3, 1), named("b", 4, 1), named("c", 5, 1)],
        }
    }

    fn source_pools(&self) -> Vec<SourcePool<'_>> {
        [1, 3, 6]
            .into_iter()
            .zip(&self.sources)
            .zip(&self.pools)
            .map(|((weight, source), items)| SourcePool {
                source,
                weight,
                items,
            })
            .collect()
    }
}

fn weighted_params(channel: u32) -> Params<'static> {
    Params {
        channel,
        mode: Mode::Weighted,
        breaks: None,
    }
}

/// How often events of items whose id starts with `prefix` play, as a share of
/// all events.
fn share(events: &[Event], prefix: &str) -> f64 {
    let count = events
        .iter()
        .filter(|e| e.media_id.as_str().starts_with(prefix))
        .count();
    count as f64 / events.len() as f64
}

#[test]
fn weighted_picks_approach_the_source_weights_and_repeat_for_a_fixed_seed() {
    let trio = Trio::new();
    let sources = trio.source_pools();
    let start = at(2026, 6, 1, 0, 0);
    let end = start + 20_000 * MINUTE;
    let run = |channel| {
        generate(
            &weighted_params(channel),
            &sources,
            &Resume::default(),
            start,
            end,
        )
        .unwrap()
    };

    let out = run(3);
    assert_eq!(out.events.len(), 20_000);
    for (prefix, expected) in [("a", 0.1), ("b", 0.3), ("c", 0.6)] {
        let share = share(&out.events, prefix);
        assert!((share - expected).abs() < 0.015, "{prefix}: {share}");
    }
    assert_eq!(out, run(3));
    assert_ne!(out.events, run(4).events);
}

#[test]
fn a_source_with_more_items_does_not_play_more_often() {
    // The first draw picks the source by weight alone: sources of 1 and 40
    // items with equal weights play equally often.
    let few = [item("f0", 1)];
    let many: Vec<MediaItem> = (0..40).map(|i| item(&format!("m{i}"), 1)).collect();
    let (few_source, many_source) = (tag("few"), tag("many"));
    let sources = [
        SourcePool {
            source: &few_source,
            weight: 1,
            items: &few,
        },
        SourcePool {
            source: &many_source,
            weight: 1,
            items: &many,
        },
    ];
    let start = at(2026, 6, 1, 0, 0);
    let out = generate(
        &weighted_params(3),
        &sources,
        &Resume::default(),
        start,
        start + 10_000 * MINUTE,
    )
    .unwrap();
    assert!((share(&out.events, "f") - 0.5).abs() < 0.02);
}

/// The picks of the items whose ids start with `prefix`, in order.
fn picks_of<'a>(events: &'a [Event], prefix: &str) -> Vec<&'a str> {
    events
        .iter()
        .map(|e| e.media_id.as_str())
        .filter(|id| id.starts_with(prefix))
        .collect()
}

#[test]
fn no_item_repeats_inside_its_sources_cooldown() {
    // Items of different lengths, over a week through the 25-hour day and one
    // through the 23-hour day.
    let sources = [tag("a"), tag("b"), tag("c")];
    let pools = [named("a", 2, 21), named("b", 7, 33), named("c", 13, 47)];
    let pooled: Vec<SourcePool<'_>> = [2, 1, 5]
        .into_iter()
        .zip(&sources)
        .zip(&pools)
        .map(|((weight, source), items)| SourcePool {
            source,
            weight,
            items,
        })
        .collect();
    for start in [at(2026, 10, 30, 5, 0), at(2026, 3, 6, 6, 0)] {
        let out = generate(
            &weighted_params(3),
            &pooled,
            &Resume::default(),
            start,
            start + HORIZON_MS,
        )
        .unwrap();
        assert_covers(&out.events, start, start + HORIZON_MS);
        for (prefix, n) in [("a", 2), ("b", 7), ("c", 13)] {
            let picks = picks_of(&out.events, prefix);
            assert!(picks.len() > 2 * n, "{prefix}: {}", picks.len());
            assert_cooldown_holds(&picks, n);
        }
    }
}

#[test]
fn every_item_of_a_weighted_source_plays_within_a_week() {
    let trio = Trio::new();
    let sources = trio.source_pools();
    let start = at(2026, 6, 1, 0, 0);
    let out = generate(
        &weighted_params(3),
        &sources,
        &Resume::default(),
        start,
        start + 7 * DAY,
    )
    .unwrap();
    for item in trio.pools.iter().flatten() {
        assert!(
            out.events.iter().any(|e| e.media_id == item.id),
            "{} never played",
            item.id.as_str()
        );
    }
}

#[test]
fn a_sources_cooldown_counts_the_history_before_the_run() {
    // `a` has four items, so three are cooling down: after a2, a1 and a0 only
    // a3 can play from it. `b` has one item and no cooldown.
    let (a, b) = (tag("a"), tag("b"));
    let (a_items, b_items) = (named("a", 4, 1), named("b", 1, 1));
    let sources = [
        SourcePool {
            source: &a,
            weight: 1,
            items: &a_items,
        },
        SourcePool {
            source: &b,
            weight: 1,
            items: &b_items,
        },
    ];
    let resume = Resume {
        recent: ["a2", "x", "a1", "a0"]
            .into_iter()
            .map(MediaId::new)
            .collect(),
        ..Resume::default()
    };
    // One pick for each of many channels, which seed differently: whenever it
    // is from `a`, it is a3.
    let start = at(2026, 6, 1, 0, 0);
    let mut from_a = 0;
    for channel in 1..=40 {
        let out = generate(
            &weighted_params(channel),
            &sources,
            &resume,
            start,
            start + MINUTE,
        )
        .unwrap();
        let id = out.events[0].media_id.as_str();
        if id.starts_with('a') {
            assert_eq!(id, "a3", "channel {channel}");
            from_a += 1;
        }
    }
    assert!(from_a > 0, "the check never met a pick from a");
}

#[test]
fn a_weighted_run_resumed_at_a_program_boundary_matches_an_unbroken_run() {
    let trio = Trio::new();
    let sources = trio.source_pools();
    let start = at(2026, 6, 1, 0, 0);
    let end = start + 400 * MINUTE;
    let history = ["old", "c1", "b3"];
    let run = |recent: &[&str], from, to| {
        let resume = Resume {
            recent: recent.iter().map(|id| MediaId::new(*id)).collect(),
            ..Resume::default()
        };
        generate(&weighted_params(3), &sources, &resume, from, to).unwrap()
    };
    let whole = run(&history, start, end);
    for cut in [1, 2, 7, 50, 200, 399] {
        let boundary = whole.events[cut].start_ms;
        let first = run(&history, start, boundary);
        assert_eq!(first.events, whole.events[..cut]);
        // The stored history, newest first: this run's programs, then the old.
        let mut recent: Vec<&str> = ids(&first.events);
        recent.reverse();
        recent.extend(history);
        let rest = run(&recent, boundary, end);
        assert_eq!(rest.events, whole.events[cut..], "cut at {cut}");
    }
}

#[test]
fn a_source_with_nothing_playable_is_skipped_and_noted() {
    let (a, empty, zero, b) = (tag("a"), tag("empty"), tag("zero"), tag("b"));
    let a_items = named("a", 3, 1);
    let zero_items = [item("z0", 0)];
    let b_items = named("b", 3, 1);
    let sources = [
        SourcePool {
            source: &a,
            weight: 1,
            items: &a_items,
        },
        SourcePool {
            source: &empty,
            weight: 50,
            items: &[],
        },
        SourcePool {
            source: &zero,
            weight: 50,
            items: &zero_items,
        },
        SourcePool {
            source: &b,
            weight: 1,
            items: &b_items,
        },
    ];
    let start = at(2026, 6, 1, 0, 0);
    let out = generate(
        &weighted_params(3),
        &sources,
        &Resume::default(),
        start,
        start + 5_000 * MINUTE,
    )
    .unwrap();

    // The skipped sources' weight of 100 does not count: a and b split the
    // picks.
    assert!((share(&out.events, "a") - 0.5).abs() < 0.03);
    assert_eq!(share(&out.events, "z"), 0.0);
    assert_eq!(
        out.notes,
        [
            Note::SourceNotPlayable { source: empty },
            Note::SkippedZeroRuntime { count: 1 },
            Note::SourceNotPlayable { source: zero },
        ]
    );
}

#[test]
fn a_weighted_channel_with_nothing_playable_is_dead_air() {
    let (a, b) = (tag("a"), tag("b"));
    let sources = [
        SourcePool {
            source: &a,
            weight: 1,
            items: &[],
        },
        SourcePool {
            source: &b,
            weight: 2,
            items: &[],
        },
    ];
    let out = generate(&weighted_params(3), &sources, &Resume::default(), 0, DAY).unwrap();
    assert!(out.events.is_empty());
    assert_eq!(
        out.notes,
        [
            Note::SourceNotPlayable { source: a },
            Note::SourceNotPlayable { source: b },
            Note::NoPlayableItems,
        ]
    );
}

fn weighted_channel() -> Channel {
    let mut channel = channel_with_breaks(Mode::Weighted);
    channel.plan = Plan::Fixed {
        sources: vec![weighted(tag("x"), 3), weighted(tag("y"), 1)],
        mode: Mode::Weighted,
    };
    channel
}

fn weighted_pools() -> Vec<(Source, Vec<MediaItem>)> {
    vec![(tag("x"), named("x", 6, 22)), (tag("y"), named("y", 3, 41))]
}

/// The events of one unbroken run of `channel`'s weighted lineup over
/// `[from, to)`, with its breaks.
fn unbroken_weighted(
    channel: &Channel,
    pools: &[(Source, Vec<MediaItem>)],
    clips: &[ClipPool],
    from: i64,
    to: i64,
) -> Vec<Event> {
    let Plan::Fixed { sources, mode } = &channel.plan else {
        unreachable!()
    };
    let lane = extend::lane_of(pools, *mode, sources);
    let params = with_breaks(*mode, plan(11, 120, clips[0].clips(), clips[1].clips()));
    generate(&params, &lane.sources, &Resume::default(), from, to)
        .unwrap()
        .events
}

#[test]
fn a_weighted_extend_resumed_later_matches_one_unbroken_run() {
    let channel = weighted_channel();
    let pools = weighted_pools();
    let clips = clip_pools();
    let now = at(2026, 6, 1, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    let mut store = setup();

    extend(&mut store, &channel, &pools, &clips, Tz::UTC, now, true).unwrap();
    extend(&mut store, &channel, &pools, &clips, Tz::UTC, later, true).unwrap();

    let stored = store.events_between(3, now, i64::MAX).unwrap();
    assert_eq!(
        stored,
        unbroken_weighted(&channel, &pools, &clips, now, later + HORIZON_MS)
    );
    assert!(stored.iter().any(|e| e.kind == EventKind::Break));
    let played = programs(&stored);
    for (prefix, n) in [("x", 6), ("y", 3)] {
        let picks: Vec<&str> = played
            .iter()
            .copied()
            .filter(|id| id.starts_with(prefix))
            .collect();
        assert!(picks.len() > 2 * n, "{prefix}");
        assert_cooldown_holds(&picks, n);
    }
}

#[test]
fn a_weighted_extend_reads_back_past_ids_no_longer_in_any_pool() {
    // Items of 3 days: x has 12 (cooling 9) and y has 4 (cooling 3), and one
    // page is 32 rows. The newest 40 stored programs are in no pool, so the
    // items cooling down lie beyond the first page.
    let pools = vec![
        (tag("x"), named("x", 12, 3 * 24 * 60)),
        (tag("y"), named("y", 4, 3 * 24 * 60)),
    ];
    let mut channel = channel(3, Mode::Weighted);
    channel.plan = Plan::Fixed {
        sources: vec![weighted(tag("x"), 1), weighted(tag("y"), 1)],
        mode: Mode::Weighted,
    };
    let mut history: Vec<String> = (0..9).map(|i| format!("x{i}")).collect();
    history.extend((0..3).map(|i| format!("y{i}")));
    history.extend((0..40).map(|i| format!("gone{i}")));
    let now = at(2026, 6, 1, 12, 0);
    let count = i64::try_from(history.len()).unwrap();
    let old: Vec<Event> = (0..count)
        .zip(&history)
        .map(|(i, id)| {
            let start = now - (count - i) * 30 * MINUTE;
            stored(start, start + 30 * MINUTE, id, EventKind::Program)
        })
        .collect();
    let mut store = setup();
    store.write_events(3, &old).unwrap();

    extend(&mut store, &channel, &pools, &[], Tz::UTC, now, true).unwrap();

    let events = store.events_between(3, now, i64::MAX).unwrap();
    let recent: Vec<MediaId> = history
        .iter()
        .rev()
        .map(|id| MediaId::new(id.as_str()))
        .collect();
    let Plan::Fixed { sources, mode } = &channel.plan else {
        unreachable!()
    };
    let lane = extend::lane_of(&pools, *mode, sources);
    let resume = Resume {
        recent,
        ..Resume::default()
    };
    let whole = generate(
        &weighted_params(3),
        &lane.sources,
        &resume,
        now,
        now + HORIZON_MS,
    )
    .unwrap();
    assert_eq!(events, whole.events);
    // x0..x8 and y0..y2 are cooling down, so the first pick from each source
    // is one of the others.
    let first_x = picks_of(&events, "x").first().copied();
    let first_y = picks_of(&events, "y").first().copied();
    assert!(
        first_x.is_none_or(|id| ["x9", "x10", "x11"].contains(&id)),
        "{first_x:?}"
    );
    assert!(first_y.is_none_or(|id| id == "y3"), "{first_y:?}");
    assert!(first_x.is_some() || first_y.is_some());
}

#[test]
fn a_weighted_regeneration_with_unchanged_inputs_changes_nothing() {
    let channel = weighted_channel();
    let pools = weighted_pools();
    let clips = clip_pools();
    let now = at(2026, 6, 1, 12, 0);
    let mut store = setup();
    extend(&mut store, &channel, &pools, &clips, Tz::UTC, now, true).unwrap();
    let before = all_events(&store);

    for later in [now, now + 90 * MINUTE, now + 2 * DAY] {
        let result =
            regenerate(&mut store, &channel, &pools, &clips, Tz::UTC, later, true).unwrap();
        // Regenerating later also extends the schedule; the old part is the
        // same.
        regenerated_from(&result);
        let after = all_events(&store);
        assert_eq!(after[..before.len()], before[..], "{later}");
    }
}

/// A blocks channel with a weighted block: cartoons and shorts, 3 to 1, from
/// 18:00 to 23:00 every evening, and a shuffle block of drama 06:00 to 12:00.
fn weighted_block_channel() -> Channel {
    let mut channel = channel_with_breaks(Mode::Sequential);
    channel.plan = Plan::Blocks {
        blocks: vec![
            Block {
                sources: vec![weighted(tag("cartoons"), 3), weighted(tag("shorts"), 1)],
                ..block(&[], (18, 0), (23, 0), "cartoons", Mode::Weighted)
            },
            block(&[], (6, 0), (12, 0), "drama", Mode::Shuffle),
        ],
        default: None,
    };
    channel
}

fn weighted_block_pools() -> Vec<(Source, Vec<MediaItem>)> {
    vec![
        (tag("cartoons"), named("c", 6, 22)),
        (tag("shorts"), named("s", 3, 11)),
        (tag("drama"), named("d", 5, 44)),
    ]
}

#[test]
fn a_weighted_block_extended_twice_matches_one_unbroken_run_across_dst() {
    let channel = weighted_block_channel();
    let pools = weighted_block_pools();
    let clips = clip_pools();
    // Through the 25-hour day, Sunday November 1.
    let now = at(2026, 10, 28, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    let mut store = setup();

    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, later, true).unwrap();

    let Plan::Blocks { blocks, .. } = &channel.plan else {
        unreachable!()
    };
    let end = later + HORIZON_MS;
    let layout = Layout {
        channel: 3,
        breaks: Some(plan(11, 120, clips[0].clips(), clips[1].clips())),
        spans: spans(blocks, CHICAGO, now, end).unwrap(),
        lanes: blocks
            .iter()
            .map(|block| extend::lane_of(&pools, block.mode, &block.sources))
            .collect(),
        default: None,
        filler: clips[1].clips(),
        nights: Vec::new(),
        sign_off: None,
    };
    let whole = generate_blocks(&layout, &Resume::default(), now, end).unwrap();

    let stored = store.events_between(3, now, i64::MAX).unwrap();
    assert_eq!(stored, whole.events);
    // Both sources of the weighted block played, each inside its cooldown.
    let played = programs(&stored);
    for (prefix, n) in [("c", 6), ("s", 3)] {
        let picks: Vec<&str> = played
            .iter()
            .copied()
            .filter(|id| id.starts_with(prefix))
            .collect();
        assert!(picks.len() > 2 * n, "{prefix}");
        assert_cooldown_holds(&picks, n);
    }
}

#[test]
fn a_weighted_block_regeneration_with_unchanged_inputs_changes_nothing() {
    let channel = weighted_block_channel();
    let pools = weighted_block_pools();
    let clips = clip_pools();
    let now = at(2026, 10, 28, 12, 0);
    let mut store = setup();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    let before = all_events(&store);

    // Regenerating at the same time rewrites the same week; a later one also
    // extends it and keeps what was there.
    let result = regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    regenerated_from(&result);
    assert_eq!(all_events(&store), before);
    regenerate(
        &mut store,
        &channel,
        &pools,
        &clips,
        CHICAGO,
        now + DAY + 5 * MINUTE,
        true,
    )
    .unwrap();
    let after = all_events(&store);
    assert_eq!(after[..before.len()], before[..]);
}

// Blocks.

const CHICAGO: Tz = Tz::America__Chicago;

/// A block on `days`, or every day when `days` is empty, as in the config.
fn block(days: &[Day], start: (u16, u16), end: (u16, u16), tag: &str, mode: Mode) -> Block {
    let every = [
        Day::Mon,
        Day::Tue,
        Day::Wed,
        Day::Thu,
        Day::Fri,
        Day::Sat,
        Day::Sun,
    ];
    Block {
        days: if days.is_empty() { &every } else { days }.to_vec(),
        start_minute: start.0 * 60 + start.1,
        end_minute: end.0 * 60 + end.1,
        sources: vec![WeightedSource::new(Source::JellyfinTag(tag.into()))],
        mode,
    }
}

/// Items named `{prefix}0`, `{prefix}1`, ... of `minutes` each.
fn named(prefix: &str, n: usize, minutes: u64) -> Vec<MediaItem> {
    (0..n)
        .map(|i| item(&format!("{prefix}{i}"), minutes))
        .collect()
}

/// A lane with `pool` as its one source.
fn solo(mode: Mode, pool: &[MediaItem]) -> Lane<'_> {
    Lane {
        mode,
        sources: single(pool).to_vec(),
    }
}

fn lane<'a>(block: &'a Block, pool: &'a [MediaItem]) -> Lane<'a> {
    solo(block.mode, pool)
}

fn layout<'a>(
    blocks: &[Block],
    lanes: Vec<Lane<'a>>,
    default: Option<usize>,
    filler: &'a [Clip],
    start: i64,
    end: i64,
) -> Layout<'a> {
    Layout {
        channel: 3,
        breaks: None,
        spans: spans(blocks, CHICAGO, start, end).unwrap(),
        lanes,
        default,
        filler,
        nights: Vec::new(),
        sign_off: None,
    }
}

/// The spans of block `i` that start in `[from, to)`.
fn block_spans(spans: &[Span], i: usize, from: i64, to: i64) -> Vec<(i64, i64)> {
    spans
        .iter()
        .filter(|s| s.block == Some(i) && s.start_ms >= from && s.start_ms < to)
        .map(|s| (s.start_ms, s.end_ms))
        .collect()
}

#[test]
fn a_block_starts_and_ends_at_local_times_on_ordinary_and_dst_days() {
    let year = (at(2026, 1, 1, 0, 0), at(2027, 1, 1, 0, 0));
    // Chicago in 2026: CST (UTC-6) until 02:00 on Sunday March 8, the 23-hour
    // day, then CDT (UTC-5) until 02:00 on Sunday November 1, the 25-hour day.
    let morning = [block(&[Day::Sun], (6, 0), (12, 0), "m", Mode::Sequential)];
    let found = spans(&morning, CHICAGO, year.0, year.1).unwrap();
    for ((y, m, d), (from, to)) in [
        ((2026, 3, 1), (12, 18)),
        ((2026, 3, 8), (11, 17)),
        ((2026, 6, 7), (11, 17)),
        ((2026, 10, 25), (11, 17)),
        ((2026, 11, 1), (12, 18)),
    ] {
        let midnight = at(y, m, d, 0, 0);
        assert_eq!(
            block_spans(&found, 0, midnight, midnight + DAY),
            [(at(y, m, d, from, 0), at(y, m, d, to, 0))],
            "{y}-{m}-{d}"
        );
    }

    let all_day = [block(&[Day::Sun], (0, 0), (24, 0), "s", Mode::Sequential)];
    let found = spans(&all_day, CHICAGO, year.0, year.1).unwrap();
    let hours = |y, m, d| {
        let midnight = at(y, m, d, 0, 0);
        let [(start, end)] = block_spans(&found, 0, midnight, midnight + DAY)[..] else {
            panic!("one span on {y}-{m}-{d}");
        };
        (end - start) / (60 * MINUTE)
    };
    assert_eq!(hours(2026, 6, 7), 24);
    assert_eq!(hours(2026, 3, 8), 23);
    assert_eq!(hours(2026, 11, 1), 25);

    let small_hours = [block(&[Day::Sun], (1, 30), (3, 0), "n", Mode::Sequential)];
    let found = spans(&small_hours, CHICAGO, year.0, year.1).unwrap();
    let on = |y, m, d| {
        let midnight = at(y, m, d, 0, 0);
        block_spans(&found, 0, midnight, midnight + DAY)
    };
    // 01:30 happens twice on November 1; the block starts at the first, CDT,
    // and ends at 03:00 CST.
    assert_eq!(
        on(2026, 11, 1),
        [(at(2026, 11, 1, 6, 30), at(2026, 11, 1, 9, 0))]
    );
    // 02:00 to 03:00 does not happen on March 8: 01:30 CST to 03:00 CDT is
    // half an hour.
    assert_eq!(
        on(2026, 3, 8),
        [(at(2026, 3, 8, 7, 30), at(2026, 3, 8, 8, 0))]
    );
}

#[test]
fn a_block_time_that_does_not_occur_starts_when_the_clocks_jump() {
    let blocks = [block(&[Day::Sun], (2, 30), (4, 0), "x", Mode::Sequential)];
    let from = at(2026, 3, 7, 0, 0);
    let found = spans(&blocks, CHICAGO, from, from + 2 * DAY).unwrap();
    // 02:30 on March 8 does not exist; the block starts at 03:00 CDT.
    assert_eq!(
        block_spans(&found, 0, from, from + 2 * DAY),
        [(at(2026, 3, 8, 8, 0), at(2026, 3, 8, 9, 0))]
    );
}

#[test]
fn spans_cover_the_window_without_gaps() {
    let blocks = [
        block(&[Day::Sat, Day::Sun], (6, 0), (12, 0), "a", Mode::Shuffle),
        block(&[], (12, 0), (24, 0), "b", Mode::Sequential),
    ];
    let start = at(2026, 10, 30, 5, 0);
    let end = start + HORIZON_MS;
    let found = spans(&blocks, CHICAGO, start, end).unwrap();
    assert!(found[0].start_ms <= start);
    assert!(found.last().unwrap().end_ms >= end);
    for pair in found.windows(2) {
        assert_eq!(pair[0].end_ms, pair[1].start_ms);
        assert!(pair[0].start_ms < pair[0].end_ms);
    }
    assert!(spans(&blocks, CHICAGO, end, start).unwrap().is_empty());
}

#[test]
fn programs_start_at_a_block_start_and_are_cut_at_its_end() {
    // Saturday 06:00 to 12:00 is cartoons; the rest of the week the default.
    let blocks = [block(&[Day::Sat], (6, 0), (12, 0), "c", Mode::Sequential)];
    let cartoons = named("c", 3, 25);
    let movies = named("m", 4, 95);
    let lanes = vec![lane(&blocks[0], &cartoons), solo(Mode::Shuffle, &movies)];
    // A week that holds the 25-hour day, Sunday November 1.
    let start = at(2026, 10, 30, 5, 0);
    let end = start + HORIZON_MS;
    let layout = layout(&blocks, lanes, Some(1), &[], start, end);

    let out = generate_blocks(&layout, &Resume::default(), start, end).unwrap();

    assert_covers(&out.events, start, end);
    // Saturday October 31 is CDT: 06:00 is 11:00 UTC and 12:00 is 17:00 UTC.
    let (from, to) = (at(2026, 10, 31, 11, 0), at(2026, 10, 31, 17, 0));
    let first = out.events.iter().position(|e| e.start_ms == from).unwrap();
    let last = out.events.iter().position(|e| e.end_ms == to).unwrap();
    assert!(
        out.events[first..=last]
            .iter()
            .all(|e| e.media_id.as_str().starts_with('c'))
    );
    assert_eq!(ids(&out.events[first..first + 4]), ["c0", "c1", "c2", "c0"]);
    // Six hours do not divide into 25-minute cartoons, so the last is cut.
    let cut = &out.events[last];
    assert!(cut.end_ms - cut.start_ms < 25 * MINUTE);
    assert_eq!(cut.media_duration_ms, 25 * 60_000);
    let others = out.events[..first].iter().chain(&out.events[last + 1..]);
    assert!(others.clone().all(|e| e.media_id.as_str().starts_with('m')));
    assert!(out.notes.is_empty(), "{:?}", out.notes);
}

#[test]
fn a_sequential_block_continues_where_it_stopped_the_day_before() {
    let blocks = [
        block(&[Day::Mon, Day::Tue], (6, 0), (7, 0), "a", Mode::Sequential),
        block(&[Day::Mon, Day::Tue], (7, 0), (8, 0), "b", Mode::Sequential),
    ];
    let a = named("a", 5, 20);
    let b = named("b", 2, 30);
    let lanes = vec![lane(&blocks[0], &a), lane(&blocks[1], &b)];
    let start = at(2026, 6, 1, 0, 0); // a Monday
    let end = start + 2 * DAY;
    let layout = layout(&blocks, lanes, None, &[], start, end);

    let out = generate_blocks(&layout, &Resume::default(), start, end).unwrap();

    assert_eq!(
        ids(&out.events),
        ["a0", "a1", "a2", "b0", "b1", "a3", "a4", "a0", "b0", "b1"]
    );
}

#[test]
fn time_without_a_block_or_default_plays_filler_clips() {
    let blocks = [block(&[Day::Mon], (6, 0), (7, 0), "a", Mode::Sequential)];
    let a = named("a", 1, 20);
    let clips = [clip("ident", 40), clip("slate", 50)];
    let six = at(2026, 6, 1, 11, 0); // 06:00 CDT
    let (start, end) = (six - 10 * MINUTE, six + 62 * MINUTE);
    let lanes = vec![lane(&blocks[0], &a)];
    let layout = layout(&blocks, lanes, None, &clips, start, end);

    let out = generate_blocks(&layout, &Resume::default(), start, end).unwrap();

    assert_covers(&out.events, start, end);
    let (before, rest): (Vec<&Event>, Vec<&Event>) =
        out.events.iter().partition(|e| e.start_ms < six);
    assert!(before.iter().all(|e| e.kind == EventKind::Filler));
    assert_eq!(before.last().unwrap().end_ms, six);
    assert!(rest[..3].iter().all(|e| e.kind == EventKind::Program));
    assert_eq!(rest[2].end_ms, six + 60 * MINUTE);
    // From 07:00 the clips play from the first again.
    let after = rest[3];
    assert_eq!(after.kind, EventKind::Filler);
    assert_eq!(
        (after.media_id.as_str(), after.media_offset_ms),
        (clip_id("ident").as_str(), 0)
    );
    assert!(out.notes.is_empty(), "{:?}", out.notes);
}

#[test]
fn filler_resumed_mid_gap_continues_the_same_cycle() {
    let blocks = [block(&[Day::Mon], (6, 0), (7, 0), "a", Mode::Sequential)];
    let a = named("a", 1, 20);
    let clips = [clip("ident", 40), clip("slate", 50)];
    let start = at(2026, 6, 1, 12, 0);
    let end = start + 3 * 60 * MINUTE;
    let run = |from| {
        let lanes = vec![lane(&blocks[0], &a)];
        let layout = layout(&blocks, lanes, None, &clips, from, end);
        generate_blocks(&layout, &Resume::default(), from, end).unwrap()
    };

    let whole = run(start);
    for cut in [7, 30, 101] {
        let rest = run(whole.events[cut].start_ms);
        assert_eq!(rest.events, whole.events[cut..], "resumed at {cut}");
    }
}

#[test]
fn time_with_nothing_to_play_is_dead_air_and_noted() {
    let blocks = [block(&[Day::Mon], (6, 0), (7, 0), "a", Mode::Sequential)];
    let a = named("a", 1, 20);
    let six = at(2026, 6, 1, 11, 0); // 06:00 CDT
    let (start, end) = (six - 60 * MINUTE, six + 120 * MINUTE);
    let layout = layout(&blocks, vec![lane(&blocks[0], &a)], None, &[], start, end);

    let out = generate_blocks(&layout, &Resume::default(), start, end).unwrap();

    assert_eq!(out.events.len(), 3);
    assert_eq!(out.events[0].start_ms, six);
    assert_eq!(out.events[2].end_ms, six + 60 * MINUTE);
    assert_eq!(
        out.notes,
        [Note::DeadAir {
            ms: 2 * 60 * 60_000
        }]
    );
}

#[test]
fn a_block_with_nothing_playable_falls_back_to_the_default() {
    let blocks = [block(&[Day::Mon], (6, 0), (7, 0), "a", Mode::Sequential)];
    let default = named("d", 2, 30);
    let lanes = vec![lane(&blocks[0], &[]), solo(Mode::Sequential, &default)];
    let six = at(2026, 6, 1, 11, 0);
    let layout = layout(&blocks, lanes, Some(1), &[], six, six + 60 * MINUTE);

    let out = generate_blocks(&layout, &Resume::default(), six, six + 60 * MINUTE).unwrap();

    assert_eq!(ids(&out.events), ["d0", "d1"]);
    assert_eq!(out.notes, [Note::NoPlayableItems]);
}

fn blocks_channel_with_breaks() -> Channel {
    let mut channel = channel_with_breaks(Mode::Sequential);
    channel.plan = Plan::Blocks {
        blocks: vec![
            block(
                &[Day::Sat, Day::Sun],
                (6, 0),
                (12, 0),
                "cartoons",
                Mode::Shuffle,
            ),
            block(
                &[Day::Mon, Day::Wed, Day::Fri],
                (18, 30),
                (23, 0),
                "drama",
                Mode::Sequential,
            ),
        ],
        default: Some(Lineup {
            sources: vec![WeightedSource::new(Source::JellyfinTag("movies".into()))],
            mode: Mode::Weighted,
        }),
    };
    channel
}

fn blocks_pools() -> Vec<(Source, Vec<MediaItem>)> {
    vec![
        (Source::JellyfinTag("cartoons".into()), named("c", 6, 22)),
        (Source::JellyfinTag("drama".into()), named("d", 5, 44)),
        (Source::JellyfinTag("movies".into()), named("m", 4, 97)),
    ]
}

#[test]
fn a_blocks_extend_twice_matches_one_unbroken_run_across_dst() {
    let channel = blocks_channel_with_breaks();
    let pools = blocks_pools();
    let clips = clip_pools();
    // Through the 25-hour day, Sunday November 1.
    let now = at(2026, 10, 28, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    let mut store = setup();

    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, later, true).unwrap();

    let Plan::Blocks { blocks, default } = &channel.plan else {
        unreachable!()
    };
    let default = default.as_ref().unwrap();
    let lanes = vec![
        lane(&blocks[0], &pools[0].1),
        lane(&blocks[1], &pools[1].1),
        solo(default.mode, &pools[2].1),
    ];
    let end = later + HORIZON_MS;
    let layout = Layout {
        channel: 3,
        breaks: Some(plan(11, 120, clips[0].clips(), clips[1].clips())),
        spans: spans(blocks, CHICAGO, now, end).unwrap(),
        lanes,
        default: Some(2),
        filler: clips[1].clips(),
        nights: Vec::new(),
        sign_off: None,
    };
    let whole = generate_blocks(&layout, &Resume::default(), now, end).unwrap();

    let stored = store.events_between(3, now, i64::MAX).unwrap();
    assert_eq!(stored, whole.events);
    assert!(stored.iter().any(|e| e.kind == EventKind::Break));
    // Saturday's cartoons start at 06:00 CDT and Sunday's at 06:00 CST, and
    // both run six hours.
    for start in [at(2026, 10, 31, 11, 0), at(2026, 11, 1, 12, 0)] {
        let first = stored.iter().find(|e| e.start_ms == start).unwrap();
        assert!(first.media_id.as_str().starts_with('c'), "{first:?}");
        assert!(stored.iter().any(|e| e.end_ms == start + 6 * 60 * MINUTE));
    }
}

#[test]
fn a_blocks_extend_with_break_settings_matches_one_unbroken_run() {
    // Break clips cut off at a block's end never air, so they do not count
    // toward the repeat limit in an unbroken run either.
    let pools = blocks_pools();
    let clips = clip_pools_with_idents();
    let now = at(2026, 10, 28, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    for rotation in [Rotation::Order, Rotation::Shuffle] {
        let mut channel = blocks_channel_with_breaks();
        let breaks = channel.breaks.as_mut().unwrap();
        breaks.rotation = rotation;
        breaks.repeat_after = Some(Duration::from_secs(25 * 60));
        breaks.pad = Some("idents".into());
        let mut store = setup();

        extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
        extend(&mut store, &channel, &pools, &clips, CHICAGO, later, true).unwrap();

        let Plan::Blocks { blocks, default } = &channel.plan else {
            unreachable!()
        };
        let default = default.as_ref().unwrap();
        let lanes = vec![
            lane(&blocks[0], &pools[0].1),
            lane(&blocks[1], &pools[1].1),
            solo(default.mode, &pools[2].1),
        ];
        let end = later + HORIZON_MS;
        let plan = BreakPlan {
            idents: clips[2].clips(),
            ..with_rotation(
                rotation,
                Some(25 * MINUTE),
                plan(11, 120, clips[0].clips(), clips[1].clips()),
            )
        };
        let layout = Layout {
            channel: 3,
            breaks: Some(plan),
            spans: spans(blocks, CHICAGO, now, end).unwrap(),
            lanes,
            default: Some(2),
            filler: clips[1].clips(),
            nights: Vec::new(),
            sign_off: None,
        };
        let whole = generate_blocks(&layout, &Resume::default(), now, end).unwrap();

        let stored = store.events_between(3, now, i64::MAX).unwrap();
        assert_eq!(stored, whole.events, "{rotation:?}");
        assert_repeat_limit_holds(&stored, 25 * MINUTE);
    }
}

#[test]
fn a_break_clip_cut_off_by_a_block_end_does_not_count_toward_repeat_after() {
    // The first block ends 30 s into the break at 11 minutes: `a` is cut
    // there and `b` never airs. The next block's first break, at 22:30,
    // continues with `b`, which a kept airing at 12:00 would hold back.
    let shows = [item("show", 22)];
    let ads = [clip("a", 60), clip("b", 60)];
    let lane = solo(Mode::Sequential, &shows);
    let cut = 11 * MINUTE + 30_000;
    let layout = Layout {
        channel: 3,
        breaks: Some(with_rotation(
            Rotation::Order,
            Some(15 * MINUTE),
            plan(11, 120, &ads, &[]),
        )),
        lanes: vec![lane.clone(), lane],
        default: None,
        spans: vec![
            Span {
                start_ms: 0,
                end_ms: cut,
                block: Some(0),
            },
            Span {
                start_ms: cut,
                end_ms: i64::MAX,
                block: Some(1),
            },
        ],
        filler: &[],
        nights: Vec::new(),
        sign_off: None,
    };

    let out = generate_blocks(&layout, &Resume::default(), 0, cut + 1).unwrap();

    let brk: Vec<(i64, &str)> = out
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Break)
        .map(|e| (e.start_ms, e.media_id.as_str()))
        .collect();
    let (a, b) = (clip_id("a"), clip_id("b"));
    assert_eq!(
        brk[..2],
        [(11 * MINUTE, a.as_str()), (cut + 11 * MINUTE, b.as_str())]
    );
}

#[test]
fn a_blocks_extend_reads_back_to_each_sequential_block() {
    let channel = blocks_channel_with_breaks();
    let pools = blocks_pools();
    let now = at(2026, 6, 1, 12, 0); // a Monday
    let mut store = setup();

    extend(&mut store, &channel, &pools, &[], CHICAGO, now, true).unwrap();
    // A week later the drama block continues after its last episode, though
    // many movies and cartoons played since.
    extend(
        &mut store,
        &channel,
        &pools,
        &[],
        CHICAGO,
        now + 7 * DAY,
        true,
    )
    .unwrap();

    let stored = store.events_between(3, now, i64::MAX).unwrap();
    let drama: Vec<&str> = programs(&stored)
        .into_iter()
        .filter(|id| id.starts_with('d'))
        .collect();
    let expected: Vec<String> = (0..drama.len()).map(|i| format!("d{}", i % 5)).collect();
    assert_eq!(drama, expected);
    assert!(drama.len() > 20);
}

#[test]
fn break_clips_cut_off_at_a_block_end_play_in_the_next_break() {
    // Each day the block ends one minute into the break, so only its first
    // clip plays; the next day's break starts with the clip that did not.
    let blocks = [block(
        &[Day::Mon, Day::Tue],
        (6, 0),
        (6, 11),
        "p",
        Mode::Sequential,
    )];
    let show = [item("p", 20)];
    let clips = [clip("a", 60), clip("b", 60)];
    let start = at(2026, 6, 1, 0, 0); // a Monday
    let end = start + 2 * DAY;
    let mut layout = layout(
        &blocks,
        vec![lane(&blocks[0], &show)],
        None,
        &[],
        start,
        end,
    );
    layout.breaks = Some(plan(10, 120, &clips, &[]));

    let out = generate_blocks(&layout, &Resume::default(), start, end).unwrap();

    let breaks: Vec<&str> = out
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Break)
        .map(|e| e.media_id.as_str())
        .collect();
    assert_eq!(breaks, [clip_id("a"), clip_id("b")]);
}

#[test]
fn breaks_after_a_block_end_are_not_counted_as_short() {
    // `p` runs 60 minutes with a cut every 10 minutes, but the block ends at
    // 06:15. Without filler each break is short; only the one at 06:10 plays.
    let blocks = [block(&[Day::Mon], (6, 0), (6, 15), "p", Mode::Sequential)];
    let show = [item("p", 60)];
    let ads = [clip("a", 60)];
    let six = at(2026, 6, 1, 11, 0); // 06:00 CDT
    let end = six + 15 * MINUTE;
    let mut layout = layout(&blocks, vec![lane(&blocks[0], &show)], None, &[], six, end);
    layout.breaks = Some(plan(10, 120, &ads, &[]));

    let out = generate_blocks(&layout, &Resume::default(), six, end).unwrap();

    assert_eq!(out.events.last().unwrap().end_ms, end);
    assert_eq!(out.notes, [Note::ShortBreaks { count: 1 }]);
}

// Regeneration.

/// `regenerate` for a channel with one source, whose pool is `items`, in UTC.
fn regenerate_fixed(
    store: &mut Store,
    channel: &Channel,
    items: &[MediaItem],
    clips: &[ClipPool],
    now_ms: i64,
) -> Extended {
    let pools: Vec<(Source, Vec<MediaItem>)> = channel
        .plan
        .sources()
        .into_iter()
        .map(|source| (source.clone(), items.to_vec()))
        .collect();
    regenerate(store, channel, &pools, clips, Tz::UTC, now_ms, true).unwrap()
}

fn all_events(store: &Store) -> Vec<Event> {
    store.events_between(3, i64::MIN, i64::MAX).unwrap()
}

fn regenerated_from(result: &Extended) -> i64 {
    let Extended::Generated { from_ms, .. } = result else {
        panic!("expected events, got {result:?}");
    };
    *from_ms
}

#[test]
fn a_regeneration_keeps_every_event_that_starts_within_30_minutes() {
    // a 0-20, b 20-47, c 47-81 minutes. Regenerating at 5 minutes freezes
    // until 35, so c, the first program starting after that, is the boundary.
    let items = pool(&["a", "b", "c", "d"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);
    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
    let before = all_events(&store);

    let later = now + 5 * MINUTE;
    let result = regenerate_fixed(&mut store, &channel, &pool(&["x", "y", "z"]), &[], later);

    let boundary = regenerated_from(&result);
    assert_eq!(boundary, now + 47 * MINUTE);
    assert!(boundary >= later + FREEZE_MS);
    let after = all_events(&store);
    let kept = before.iter().take_while(|e| e.start_ms < boundary).count();
    assert_eq!(after[..kept], before[..kept]);
    assert_eq!(ids(&after[..kept]), ["a", "b"]);
    // The cursor `b` is not in the new pool, so the order restarts at `x`.
    assert_eq!(after[kept].media_id.as_str(), "x");
    assert_covers(&after, now, later + HORIZON_MS);
}

#[test]
fn a_regeneration_with_unchanged_inputs_changes_nothing() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let pools = clip_pools();
    let now = at(2026, 6, 1, 12, 0);
    for mode in [Mode::Sequential, Mode::Shuffle, Mode::Weighted] {
        let mut store = setup();
        let channel = channel_with_breaks(mode);
        extend_fixed(&mut store, &channel, &items, &pools, now, true).unwrap();
        let before = all_events(&store);

        let result = regenerate_fixed(&mut store, &channel, &items, &pools, now);

        regenerated_from(&result);
        assert_eq!(all_events(&store), before, "{mode:?}");
    }
}

#[test]
fn a_regeneration_starts_at_a_program_not_inside_its_breaks() {
    // With an 11-minute break interval, `a` (20 minutes) is cut once and `b`
    // (27 minutes) twice. The freeze point falls inside `b`, so all of `b`
    // and its breaks are kept and the new schedule starts at `c`.
    let items = pool(&["a", "b", "c", "d"]);
    let pools = clip_pools();
    let mut store = setup();
    let channel = channel_with_breaks(Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);
    extend_fixed(&mut store, &channel, &items, &pools, now, true).unwrap();
    let before = all_events(&store);

    let result = regenerate_fixed(&mut store, &channel, &pool(&["x"]), &pools, now);

    let boundary = regenerated_from(&result);
    let first = before.iter().find(|e| e.start_ms == boundary).unwrap();
    assert_eq!(
        (first.kind, first.media_id.as_str(), first.media_offset_ms),
        (EventKind::Program, "c", 0)
    );
    let after = all_events(&store);
    let kept = before.iter().take_while(|e| e.start_ms < boundary).count();
    assert_eq!(after[..kept], before[..kept]);
    assert_eq!(before[kept - 1].kind, EventKind::Program);
    assert_eq!(programs(&after[kept..])[0], "x");
    assert_covers(&after, now, now + HORIZON_MS);
}

#[test]
fn changing_an_item_duration_changes_only_later_events() {
    // a 0-20, b 20-47 and then c, d, a, b again from 47 minutes. `b` grows
    // from 27 to 30 minutes.
    let items = pool(&["a", "b", "c", "d"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);
    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
    let before = all_events(&store);
    let mut longer = items.clone();
    longer[1].runtime_ms = 30 * MINUTE as u64;

    let result = regenerate_fixed(&mut store, &channel, &longer, &[], now);

    let boundary = regenerated_from(&result);
    let after = all_events(&store);
    // The `b` in the frozen half hour keeps its scheduled length.
    assert_eq!(after[1], before[1]);
    assert_eq!(after[1].media_duration_ms, 27 * MINUTE as u64);
    // Everything up to the next `b` after the boundary is the same.
    let next_b = after
        .iter()
        .position(|e| e.start_ms >= boundary && e.media_id.as_str() == "b")
        .unwrap();
    assert_eq!(ids(&after[..next_b]), ["a", "b", "c", "d", "a"]);
    assert_eq!(after[..next_b], before[..next_b]);
    assert_eq!(after[next_b].media_duration_ms, 30 * MINUTE as u64);
    assert_eq!(after[next_b].end_ms - after[next_b].start_ms, 30 * MINUTE);
    assert_ne!(after[next_b + 1].start_ms, before[next_b + 1].start_ms);
    assert_covers(&after, now, now + HORIZON_MS);
}

#[test]
fn a_regeneration_past_the_end_of_a_short_schedule_extends_it() {
    let items = pool(&["a", "b"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);
    store
        .write_events(
            3,
            &[stored(now, now + 20 * MINUTE, "a", EventKind::Program)],
        )
        .unwrap();

    let result = regenerate_fixed(&mut store, &channel, &items, &[], now);

    assert_eq!(regenerated_from(&result), now + 20 * MINUTE);
    let after = all_events(&store);
    assert_eq!(ids(&after[..2]), ["a", "b"]);
    assert_covers(&after, now, now + HORIZON_MS);
}

/// A blocks channel with a 06:00 to 07:00 block of `a` every day, the
/// `filler` pool in the time no block covers when `filler` is true, and no
/// default.
fn morning_channel(filler: bool) -> Channel {
    let mut channel = channel(3, Mode::Sequential);
    channel.plan = Plan::Blocks {
        blocks: vec![block(&[], (6, 0), (7, 0), "a", Mode::Sequential)],
        default: None,
    };
    channel.filler = filler.then(|| "filler".into());
    channel
}

/// Adds an evening block of `b`, 20:00 to 21:00 every day, to `channel`.
fn add_evening_block(channel: &mut Channel) {
    let Plan::Blocks { blocks, .. } = &mut channel.plan else {
        unreachable!()
    };
    blocks.push(block(&[], (20, 0), (21, 0), "b", Mode::Sequential));
}

fn morning_pools() -> Vec<(Source, Vec<MediaItem>)> {
    vec![
        (Source::JellyfinTag("a".into()), named("a", 3, 20)),
        (Source::JellyfinTag("b".into()), named("b", 3, 30)),
    ]
}

#[test]
fn a_blocks_regeneration_reaches_the_filler_before_the_next_program() {
    // At 12:00 the channel plays filler until 06:00 tomorrow. A block added
    // at 20:00 airs tonight.
    let mut channel = morning_channel(true);
    let pools = morning_pools();
    let clips = [ClipPool::new(
        "filler",
        "/clips",
        vec![clip("slate", 90), clip("ident", 45)],
        Vec::new(),
    )];
    let now = at(2026, 6, 1, 17, 0); // 12:00 CDT
    let mut store = setup();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    let before = all_events(&store);

    // Unchanged inputs change nothing.
    let same = regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    regenerated_from(&same);
    assert_eq!(all_events(&store), before);

    add_evening_block(&mut channel);
    let result = regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();

    let boundary = regenerated_from(&result);
    assert!(boundary >= now + FREEZE_MS);
    assert!(boundary < now + FREEZE_MS + 2 * MINUTE, "{boundary}");
    let after = all_events(&store);
    let kept = before.iter().take_while(|e| e.start_ms < boundary).count();
    assert_eq!(after[..kept], before[..kept]);
    assert_eq!(after[kept - 1].kind, EventKind::Filler);
    let eight = at(2026, 6, 2, 1, 0); // 20:00 CDT
    let first = after.iter().find(|e| e.start_ms == eight).unwrap();
    assert_eq!(
        (first.kind, first.media_id.as_str()),
        (EventKind::Program, "b0")
    );
    assert_covers(&after, before[0].start_ms, now + HORIZON_MS);
}

#[test]
fn a_blocks_regeneration_reaches_dead_air_after_the_freeze_point() {
    // Without filler the channel has no events from 07:00 to 06:00. The
    // freeze point falls inside that time, so the new schedule starts there.
    let mut channel = morning_channel(false);
    let pools = morning_pools();
    let now = at(2026, 6, 1, 17, 0); // 12:00 CDT
    let mut store = setup();
    extend(&mut store, &channel, &pools, &[], CHICAGO, now, true).unwrap();
    let before = all_events(&store);

    add_evening_block(&mut channel);
    let result = regenerate(&mut store, &channel, &pools, &[], CHICAGO, now, true).unwrap();

    assert_eq!(regenerated_from(&result), now + FREEZE_MS);
    let after = all_events(&store);
    let eight = at(2026, 6, 2, 1, 0); // 20:00 CDT
    assert_eq!(after[0].start_ms, eight);
    assert_eq!(after[0].media_id.as_str(), "b0");
    assert!(after.len() > before.len());
}

#[test]
fn a_regeneration_with_nothing_playable_replaces_the_future() {
    // The source now matches no playable item: the frozen half hour stays and
    // nothing is scheduled from the boundary on.
    let items = pool(&["a", "b", "c"]);
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);
    extend_fixed(&mut store, &channel, &items, &[], now, true).unwrap();
    let before = all_events(&store);

    let result = regenerate_fixed(&mut store, &channel, &[], &[], now);

    let Extended::Generated {
        from_ms,
        written,
        notes,
        ..
    } = result
    else {
        panic!("expected events, got {result:?}");
    };
    assert!(from_ms >= now + FREEZE_MS);
    assert_eq!(written, 0);
    assert_eq!(notes, [Note::NoPlayableItems]);
    let after = all_events(&store);
    let kept = before.iter().take_while(|e| e.start_ms < from_ms).count();
    assert!(kept < before.len());
    assert_eq!(after, before[..kept]);
}

#[test]
fn a_regeneration_does_nothing_while_the_clock_is_not_trusted() {
    let mut store = setup();
    let channel = channel(3, Mode::Sequential);
    let pools = [(Source::JellyfinLibrary("All".into()), pool(&["a"]))];

    let result = regenerate(&mut store, &channel, &pools, &[], Tz::UTC, 0, false).unwrap();

    assert_eq!(result, Extended::ClockNotTrusted);
    assert!(all_events(&store).is_empty());
}

// Fitting a file to its event.

/// A program event for a 20-minute item `a`, `length_min` long, playing the
/// media from `offset_min`.
fn slot(offset_min: u64, length_min: i64) -> Event {
    let start = at(2026, 6, 1, 12, 0);
    Event {
        start_ms: start,
        end_ms: start + length_min * MINUTE,
        media_id: MediaId::new("a"),
        media_offset_ms: offset_min * 60 * SECOND,
        media_duration_ms: 20 * 60 * SECOND,
        kind: EventKind::Program,
    }
}

#[test]
fn a_short_file_is_padded_from_the_filler_pool_to_its_scheduled_end() {
    let event = slot(0, 20);
    let filler = [clip("slate", 90), clip("ident", 45)];

    let fitted = fit(&event, 18 * 60 * SECOND, &filler);

    assert_eq!(
        fitted.fit,
        Fit::Short {
            by_ms: 2 * 60 * SECOND,
            padded: true
        }
    );
    assert_eq!(
        shape(&fitted.events),
        [
            (EventKind::Program, "a".into(), 0, 18 * MINUTE),
            (EventKind::Filler, clip_id("slate"), 0, 90_000),
            (EventKind::Filler, clip_id("ident"), 0, 30_000),
        ]
    );
    assert_covers(&fitted.events, event.start_ms, event.end_ms);
    assert_eq!(fitted.events.last().unwrap().end_ms, event.end_ms);
}

#[test]
fn a_long_file_is_cut_at_its_scheduled_end() {
    let event = slot(0, 20);

    let fitted = fit(&event, 23 * 60 * SECOND, &[clip("slate", 90)]);

    assert_eq!(
        fitted.fit,
        Fit::Long {
            by_ms: 3 * 60 * SECOND
        }
    );
    assert_eq!(fitted.events, [event]);
}

#[test]
fn each_mismatch_is_recorded_in_playback_history() {
    let mut store = setup();
    let event = slot(0, 20);
    let filler = [clip("slate", 90)];
    let start = event.start_ms;

    for (real_min, at_ms) in [
        (20, start),
        (18, start + 1),
        (19, start + 2),
        (25, start + 3),
    ] {
        let fitted = fit(&event, real_min * 60 * SECOND, &filler);
        record_fit(&mut store, 3, &event, fitted.fit, at_ms).unwrap();
    }
    let short = fit(&event, 18 * 60 * SECOND, &[]);
    assert!(record_fit(&mut store, 3, &event, short.fit, start + 4).unwrap());

    let history: Vec<(i64, String, String)> = store
        .playback_history(3, 10)
        .unwrap()
        .into_iter()
        .map(|row| {
            (
                row.started_at_ms - start,
                row.media_id.to_string(),
                row.outcome,
            )
        })
        .collect();
    // The exact fit at `start` writes no row.
    let row = |at: i64, outcome: &str| (at, "a".to_string(), outcome.to_string());
    assert_eq!(
        history,
        [
            row(4, "short"),
            row(3, "cut"),
            row(2, "padded"),
            row(1, "padded")
        ]
    );
}

#[test]
fn a_short_file_without_filler_leaves_the_rest_of_the_event_empty() {
    let event = slot(0, 20);

    let fitted = fit(&event, 18 * 60 * SECOND, &[]);

    assert_eq!(fitted.fit.outcome(), Some("short"));
    assert_eq!(
        shape(&fitted.events),
        [(EventKind::Program, "a".into(), 0, 18 * MINUTE)]
    );
}

#[test]
fn a_segment_the_file_does_not_reach_is_all_filler() {
    // The segment plays 15..20 minutes; the file is 14 minutes long.
    let event = slot(15, 5);

    let fitted = fit(&event, 14 * 60 * SECOND, &[clip("slate", 240)]);

    assert_eq!(
        shape(&fitted.events),
        [
            (EventKind::Filler, clip_id("slate"), 0, 4 * MINUTE),
            (EventKind::Filler, clip_id("slate"), 0, MINUTE),
        ]
    );
    assert_eq!(fitted.fit.outcome(), Some("padded"));
}

#[test]
fn a_long_file_in_a_segment_before_a_break_is_not_a_mismatch() {
    // The first 11 minutes of a 20-minute item, cut for a break. The file is
    // longer than scheduled, but this event does not reach the item's end.
    let event = slot(0, 11);

    assert_eq!(fit(&event, 25 * 60 * SECOND, &[]).fit, Fit::Exact);
    assert_eq!(fit(&event, 11 * 60 * SECOND, &[]).fit, Fit::Exact);
    // The last segment, 11..20 minutes, is where the long file is cut.
    assert_eq!(
        fit(&slot(11, 9), 25 * 60 * SECOND, &[]).fit,
        Fit::Long {
            by_ms: 5 * 60 * SECOND
        }
    );
}

#[test]
fn a_length_within_half_a_second_of_the_event_is_exact() {
    let event = slot(0, 20);
    let filler = [clip("slate", 90)];
    let scheduled = 20 * 60 * SECOND;

    for real_ms in [scheduled - FIT_TOLERANCE_MS, scheduled + FIT_TOLERANCE_MS] {
        let fitted = fit(&event, real_ms, &filler);
        assert_eq!(
            (fitted.fit, &fitted.events),
            (Fit::Exact, &vec![event.clone()])
        );
    }
    assert_eq!(
        fit(&event, scheduled - FIT_TOLERANCE_MS - 1, &filler).fit,
        Fit::Short {
            by_ms: FIT_TOLERANCE_MS + 1,
            padded: true
        }
    );
    assert_eq!(
        fit(&event, scheduled + FIT_TOLERANCE_MS + 1, &filler).fit,
        Fit::Long {
            by_ms: FIT_TOLERANCE_MS + 1
        }
    );
}

#[test]
fn clip_events_are_never_fitted() {
    let mut event = slot(0, 2);
    event.kind = EventKind::Break;

    let fitted = fit(&event, 0, &[clip("slate", 90)]);

    assert_eq!((fitted.fit, fitted.events), (Fit::Exact, vec![event]));
}

mod sign_off;
