use chrono::{TimeZone, Utc};

use super::*;
use crate::config::{Channel, Mode, Plan, Source, Tz};
use crate::library::{FakeMediaSource, MediaItem, MediaKind};
use crate::store::Store;
use crate::timeline::{Event, EventKind};

const MINUTE: i64 = 60_000;
const DAY: i64 = 24 * 60 * MINUTE;
const CHICAGO: Tz = chrono_tz::America::Chicago;

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

fn source(items: &[MediaItem]) -> FakeMediaSource {
    items
        .iter()
        .cloned()
        .fold(FakeMediaSource::new(), FakeMediaSource::with_item)
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

fn params(mode: Mode) -> Params {
    Params {
        channel: 3,
        mode,
        tz: CHICAGO,
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
        cursor: Some(cursor.into()),
        played: Vec::new(),
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
fn a_shuffle_differs_by_channel_and_by_local_date() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let start = at(2026, 6, 1, 12, 0);
    let window = 8 * 60 * MINUTE;
    let order = |channel: u32, start: i64| {
        let params = Params {
            channel,
            ..params(Mode::Shuffle)
        };
        let out = generate(&params, &items, &Resume::default(), start, start + window).unwrap();
        out.events
            .iter()
            .take(8)
            .map(|e| e.media_id.clone())
            .collect::<Vec<_>>()
    };

    assert_ne!(order(3, start), order(4, start));
    assert_ne!(order(3, start), order(3, start + DAY));
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
    assert_eq!(out.cursor.as_deref(), Some(last.media_id.as_str()));
}

#[test]
fn sequential_starts_at_the_beginning_without_a_cursor() {
    let items = pool(&["a", "b", "c"]);
    let out = generate(&params(Mode::Sequential), &items, &Resume::default(), 0, 1).unwrap();
    assert_eq!(ids(&out.events), ["a"]);
    assert_eq!(out.cursor.as_deref(), Some("a"));
}

#[test]
fn sequential_restarts_and_says_so_when_the_cursor_item_vanished() {
    let items = pool(&["a", "b", "c"]);

    let out = generate(&params(Mode::Sequential), &items, &resume("gone"), 0, 1).unwrap();

    assert_eq!(ids(&out.events), ["a"]);
    assert_eq!(
        out.notes,
        [Note::CursorNotInPool {
            media_id: "gone".into()
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
        assert_eq!(none.cursor.as_deref(), Some("x"));
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
    assert_eq!(out.cursor.as_deref(), Some("a"));
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

#[test]
fn a_shuffle_does_not_repeat_an_item_until_the_pool_is_used_up() {
    let items = pool(&["a", "b", "c", "d", "e", "f"]);
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &params(Mode::Shuffle),
        &items,
        &Resume::default(),
        start,
        start + 7 * DAY,
    )
    .unwrap();

    assert_passes_are_permutations(&ids(&out.events), 6);
}

/// Each run of `n` programs from the start of the list uses an item at most
/// once, and a pass never opens with the item that closed the one before.
fn assert_passes_are_permutations(played: &[&str], n: usize) {
    for (i, pass) in played.chunks(n).enumerate() {
        let mut unique = pass.to_vec();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            pass.len(),
            "pass {i} repeats an item: {pass:?}"
        );
    }
    // The first item of a pass differs from the last of the one before.
    for pair in played.chunks(n).collect::<Vec<_>>().windows(2) {
        assert_ne!(pair[0].last(), pair[1].first());
    }
}

#[test]
fn a_shuffle_resumes_inside_its_pass_without_a_repeat() {
    let items = pool(&["a", "b", "c", "d", "e", "f"]);
    let start = at(2026, 6, 1, 12, 0);
    let first = generate(
        &params(Mode::Shuffle),
        &items,
        &Resume::default(),
        start,
        start + 70 * MINUTE,
    )
    .unwrap();
    // Cut so the run ends partway through a pass.
    assert!(!first.events.len().is_multiple_of(6));
    let played: Vec<String> = first.events.iter().map(|e| e.media_id.clone()).collect();

    let second = generate(
        &params(Mode::Shuffle),
        &items,
        &Resume {
            cursor: first.cursor.clone(),
            played: played.clone(),
        },
        first.events.last().unwrap().end_ms,
        start + 7 * DAY,
    )
    .unwrap();

    let all: Vec<&str> = played
        .iter()
        .map(String::as_str)
        .chain(ids(&second.events))
        .collect();
    assert_passes_are_permutations(&all, 6);
}

#[test]
fn a_shuffle_resumed_on_the_same_date_matches_an_unbroken_run() {
    let items = pool(&["a", "b", "c", "d", "e", "f"]);
    let start = at(2026, 6, 1, 17, 0);
    let whole = generate(
        &params(Mode::Shuffle),
        &items,
        &Resume::default(),
        start,
        start + 3 * 60 * MINUTE,
    )
    .unwrap();
    let cut = 2;
    let played: Vec<String> = whole.events[..cut]
        .iter()
        .map(|e| e.media_id.clone())
        .collect();

    let rest = generate(
        &params(Mode::Shuffle),
        &items,
        &Resume {
            cursor: played.last().cloned(),
            played,
        },
        whole.events[cut - 1].end_ms,
        start + 3 * 60 * MINUTE,
    )
    .unwrap();

    // The first pass is the same; later passes number from zero again.
    assert_eq!(ids(&rest.events)[..4], ids(&whole.events)[cut..6]);
}

#[test]
fn a_shuffle_with_only_a_cursor_does_not_open_with_it() {
    let items = pool(&["a", "b", "c"]);
    for cursor in ["a", "b", "c"] {
        let out = generate(&params(Mode::Shuffle), &items, &resume(cursor), 0, 1).unwrap();
        assert_ne!(out.events[0].media_id, cursor);
    }
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
fn local_dates_follow_the_calendar_on_days_of_23_and_25_hours() {
    let day = |ms| local_date_days(CHICAGO, ms).unwrap();

    // 2026-11-01: clocks go back, the day has 25 hours.
    let start = day(at(2026, 11, 1, 5, 0)); // 00:00 CDT
    assert_eq!(day(at(2026, 11, 1, 4, 59)), start - 1);
    assert_eq!(day(at(2026, 11, 2, 5, 59)), start); // 23:59 CST
    assert_eq!(day(at(2026, 11, 2, 6, 0)), start + 1);

    // 2026-03-08: clocks go forward, the day has 23 hours.
    let start = day(at(2026, 3, 8, 6, 0)); // 00:00 CST
    assert_eq!(day(at(2026, 3, 8, 5, 59)), start - 1);
    assert_eq!(day(at(2026, 3, 9, 4, 59)), start); // 23:59 CDT
    assert_eq!(day(at(2026, 3, 9, 5, 0)), start + 1);
}

#[test]
fn the_shuffle_seed_uses_the_local_date_not_the_utc_date() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let first_pass = |start: i64| {
        let pass = generate(
            &params(Mode::Shuffle),
            &items,
            &Resume::default(),
            start,
            start + 8 * 60 * MINUTE,
        )
        .unwrap();
        ids(&pass.events)[..8]
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    };

    // 00:30 CDT and 23:30 CST on 2026-11-01: the same local date, though they
    // fall on two UTC dates and are 24 hours apart.
    assert_eq!(
        first_pass(at(2026, 11, 1, 5, 30)),
        first_pass(at(2026, 11, 2, 5, 30))
    );
    // 00:30 CST the next day is a new date.
    assert_ne!(
        first_pass(at(2026, 11, 2, 6, 30)),
        first_pass(at(2026, 11, 2, 5, 30))
    );
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
                assert_passes_are_permutations(&ids(&out.events), 5);
            }
        }
    }
}

// The wrapper.

fn setup(items: &[MediaItem]) -> (Store, FakeMediaSource) {
    (Store::open_in_memory().unwrap(), source(items))
}

#[test]
fn nothing_is_generated_while_the_clock_is_not_trusted() {
    let items = pool(&["a", "b"]);
    let (mut store, media) = setup(&items);
    let channel = channel(3, Mode::Sequential);

    let result = refill_channel(
        &mut store,
        &media,
        &channel,
        CHICAGO,
        at(1970, 1, 1, 0, 0),
        false,
    )
    .unwrap();

    assert_eq!(result, Refill::ClockNotTrusted);
    assert!(
        store
            .events_between(3, i64::MIN, i64::MAX)
            .unwrap()
            .is_empty()
    );
    assert_eq!(store.cursor(3).unwrap(), None);
}

#[test]
fn a_refill_stores_seven_days_with_no_gaps_and_sets_the_cursor() {
    let items = pool(&["a", "b", "c", "d"]);
    let (mut store, media) = setup(&items);
    let channel = channel(3, Mode::Sequential);
    let now = at(2026, 6, 1, 12, 0);

    let result = refill_channel(&mut store, &media, &channel, CHICAGO, now, true).unwrap();

    let Refill::Generated {
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
        store.cursor(3).unwrap().as_deref(),
        Some(stored.last().unwrap().media_id.as_str())
    );
}

#[test]
fn a_sequential_refill_continues_from_the_stored_cursor() {
    let items = pool(&["a", "b", "c", "d"]);
    let (mut store, media) = setup(&items);
    store.set_cursor(3, "b").unwrap();
    let now = at(2026, 6, 1, 12, 0);

    refill_channel(
        &mut store,
        &media,
        &channel(3, Mode::Sequential),
        CHICAGO,
        now,
        true,
    )
    .unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_eq!(&ids(&stored)[..5], ["c", "d", "a", "b", "c"]);
}

#[test]
fn later_refills_extend_the_schedule_without_a_gap_or_a_repeat() {
    let items = pool(&["a", "b", "c", "d", "e", "f"]);
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let (mut store, media) = setup(&items);
        let channel = channel(3, mode);
        let now = at(2026, 6, 1, 12, 0);

        refill_channel(&mut store, &media, &channel, CHICAGO, now, true).unwrap();
        // Up to date: the same moment again has nothing to add.
        let again = refill_channel(&mut store, &media, &channel, CHICAGO, now, true).unwrap();
        assert!(matches!(again, Refill::UpToDate { .. }), "{mode:?}");
        // Three days on, the next run fills the gap at the far end.
        let later = now + 3 * DAY + 37 * MINUTE;
        refill_channel(&mut store, &media, &channel, CHICAGO, later, true).unwrap();

        let stored = store.events_between(3, now, later + HORIZON_MS).unwrap();
        assert_covers(&stored, now, later + HORIZON_MS);
        let played = ids(&stored);
        match mode {
            Mode::Shuffle => assert_passes_are_permutations(&played, 6),
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
fn a_refill_after_a_long_gap_starts_at_now() {
    let items = pool(&["a", "b", "c"]);
    let (mut store, media) = setup(&items);
    let channel = channel(3, Mode::Sequential);
    let then = at(2026, 6, 1, 12, 0);
    refill_channel(&mut store, &media, &channel, CHICAGO, then, true).unwrap();
    let now = then + 30 * DAY;

    refill_channel(&mut store, &media, &channel, CHICAGO, now, true).unwrap();

    let stored = store.events_between(3, now, now + HORIZON_MS).unwrap();
    assert_covers(&stored, now, now + HORIZON_MS);
}

#[test]
fn an_unplayable_library_writes_nothing_and_reports_it() {
    let (mut store, media) = setup(&[item("a", 0)]);
    let now = at(2026, 6, 1, 12, 0);

    let result = refill_channel(
        &mut store,
        &media,
        &channel(3, Mode::Shuffle),
        CHICAGO,
        now,
        true,
    )
    .unwrap();

    assert_eq!(
        result,
        Refill::Generated {
            from_ms: now,
            to_ms: now + HORIZON_MS,
            written: 0,
            notes: vec![Note::SkippedZeroRuntime { count: 1 }, Note::NoPlayableItems],
        }
    );
    assert_eq!(store.cursor(3).unwrap(), None);
}

#[test]
fn a_blocks_channel_is_refused() {
    let (mut store, media) = setup(&pool(&["a"]));
    let mut channel = channel(3, Mode::Sequential);
    channel.plan = Plan::Blocks(Vec::new());

    let result = refill_channel(&mut store, &media, &channel, CHICAGO, 0, true);

    assert!(matches!(result, Err(ScheduleError::UnsupportedPlan)));
}
