//! The nightly sign-off: the gap before it, its clip, the time off the air
//! and the return, on fixed and blocks channels, across clock changes, and
//! across restarts and regenerations.

use super::*;
use crate::config::SignOff;
use crate::schedule::extend::fill;
use crate::schedule::sign_off::{Night, clip as sign_off_clip, nights};

/// A local wall-clock time in Chicago that occurs exactly once.
fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> i64 {
    CHICAGO
        .with_ymd_and_hms(year, month, day, hour, minute, 0)
        .single()
        .unwrap()
        .timestamp_millis()
}

fn sign_off(at: (u16, u16), until: (u16, u16), clip: Option<&str>) -> SignOff {
    SignOff {
        at_minute: at.0 * 60 + at.1,
        until_minute: until.0 * 60 + until.1,
        pool: "anthem".into(),
        clip: clip.map(str::to_owned),
    }
}

/// A channel in `mode` that signs off at 23:00 with `anthem.mp4` and is back
/// on the air at 06:00, with `filler` for the gap.
fn signed_off(mode: Mode) -> Channel {
    let mut channel = channel(3, mode);
    channel.filler = Some("filler".into());
    channel.sign_off = Some(sign_off((23, 0), (6, 0), Some("anthem.mp4")));
    channel
}

/// The anthem pool (`bars` 60 s first, then `anthem` 120 s) and a filler pool
/// of two clips.
fn sign_off_pools() -> Vec<ClipPool> {
    vec![
        ClipPool::new(
            "anthem",
            "/clips",
            vec![clip("bars", 60), clip("anthem", 120)],
            Vec::new(),
        ),
        ClipPool::new(
            "filler",
            "/clips",
            vec![clip("slate", 90), clip("loop", 150)],
            Vec::new(),
        ),
    ]
}

fn pools_for(channel: &Channel, items: &[MediaItem]) -> Vec<(Source, Vec<MediaItem>)> {
    channel
        .plan
        .sources()
        .into_iter()
        .map(|source| (source.clone(), items.to_vec()))
        .collect()
}

/// What one unbroken run from an empty store gives for `[from, to)`.
fn unbroken(
    channel: &Channel,
    pools: &[(Source, Vec<MediaItem>)],
    clips: &[ClipPool],
    from: i64,
    to: i64,
) -> Generated {
    fill(&setup(), channel, pools, clips, CHICAGO, from, to).unwrap()
}

/// Checks one night off the air in `events`: no event is on at `night.start`
/// unless it starts there, the event at that moment is `clip` for
/// `clip_ms`, nothing plays from its end until `night.end`, and the time
/// before it has no gap.
fn assert_night(events: &[Event], night: Night, clip: Option<&str>, clip_ms: i64) {
    let (a, u) = (night.start_ms, night.end_ms);
    assert!(
        !events.iter().any(|e| e.start_ms < a && e.end_ms > a),
        "an event is still on at the sign-off"
    );
    let on = events.iter().filter(|e| e.end_ms > a && e.start_ms < u);
    match clip {
        Some(id) => {
            let on: Vec<&Event> = on.collect();
            assert_eq!(on.len(), 1, "only the sign-off clip plays off the air");
            assert_eq!(
                (on[0].start_ms, on[0].end_ms, on[0].kind),
                (a, a + clip_ms, EventKind::Ident)
            );
            assert_eq!(on[0].media_id.as_str(), id);
            assert_eq!(on[0].media_offset_ms, 0);
        }
        None => assert_eq!(on.count(), 0, "nothing plays off the air"),
    }
    let first = events.iter().find(|e| e.start_ms >= u).unwrap();
    assert_eq!(first.start_ms, u, "the channel is back on the air at once");
    assert_eq!(first.kind, EventKind::Program);
}

/// The nights of `channel` that lie wholly in `[from, to)`.
fn nights_in(channel: &Channel, from: i64, to: i64) -> Vec<Night> {
    nights(channel.sign_off.as_ref().unwrap(), CHICAGO, from, to)
        .unwrap()
        .into_iter()
        .filter(|night| night.start_ms >= from && night.end_ms < to)
        .collect()
}

#[test]
fn nights_are_local_times_and_end_the_next_morning_when_until_is_earlier() {
    let night = sign_off((23, 0), (6, 0), None);
    let found = nights(
        &night,
        CHICAGO,
        local(2026, 6, 1, 12, 0),
        local(2026, 6, 2, 12, 0),
    )
    .unwrap();
    assert!(found.contains(&Night {
        start_ms: local(2026, 6, 1, 23, 0),
        end_ms: local(2026, 6, 2, 6, 0),
    }));
    for pair in found.windows(2) {
        assert!(pair[0].end_ms <= pair[1].start_ms);
    }

    // `until` after `at` is the same day.
    let early = sign_off((1, 0), (5, 30), None);
    let found = nights(
        &early,
        CHICAGO,
        local(2026, 6, 1, 12, 0),
        local(2026, 6, 2, 12, 0),
    )
    .unwrap();
    assert!(found.contains(&Night {
        start_ms: local(2026, 6, 2, 1, 0),
        end_ms: local(2026, 6, 2, 5, 30),
    }));
}

#[test]
fn nights_follow_the_clocks_on_both_change_days() {
    let late = sign_off((23, 0), (6, 0), None);
    // Spring: 23:00 CST to 06:00 CDT is six hours.
    let spring = nights(&late, CHICAGO, at(2026, 3, 7, 0, 0), at(2026, 3, 9, 0, 0)).unwrap();
    assert!(spring.contains(&Night {
        start_ms: at(2026, 3, 8, 5, 0),
        end_ms: at(2026, 3, 8, 11, 0),
    }));
    // Autumn: 23:00 CDT to 06:00 CST is eight hours.
    let autumn = nights(
        &late,
        CHICAGO,
        at(2026, 10, 31, 0, 0),
        at(2026, 11, 2, 0, 0),
    )
    .unwrap();
    assert!(autumn.contains(&Night {
        start_ms: at(2026, 11, 1, 4, 0),
        end_ms: at(2026, 11, 1, 12, 0),
    }));

    // 02:30 does not occur on March 8: it is 03:00 CDT, when the clocks jump.
    let skipped = sign_off((2, 30), (5, 0), None);
    let spring = nights(
        &skipped,
        CHICAGO,
        at(2026, 3, 7, 0, 0),
        at(2026, 3, 9, 0, 0),
    )
    .unwrap();
    assert!(spring.contains(&Night {
        start_ms: at(2026, 3, 8, 8, 0),
        end_ms: at(2026, 3, 8, 10, 0),
    }));
    // 01:30 occurs twice on November 1: it is the first, 01:30 CDT.
    let twice = sign_off((1, 30), (5, 0), None);
    let autumn = nights(
        &twice,
        CHICAGO,
        at(2026, 10, 31, 0, 0),
        at(2026, 11, 2, 0, 0),
    )
    .unwrap();
    assert!(autumn.contains(&Night {
        start_ms: at(2026, 11, 1, 6, 30),
        end_ms: at(2026, 11, 1, 11, 0),
    }));
}

#[test]
fn a_sequential_night_fills_the_gap_signs_off_and_resumes_the_order() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let channel = signed_off(Mode::Sequential);
    let clips = sign_off_pools();
    let now = local(2026, 6, 1, 12, 0);
    let mut store = setup();
    extend(
        &mut store,
        &channel,
        &pools_for(&channel, &items),
        &clips,
        CHICAGO,
        now,
        true,
    )
    .unwrap();
    let events = all_events(&store);

    let found = nights_in(&channel, now, now + HORIZON_MS);
    assert_eq!(found.len(), 7);
    for night in found {
        assert_night(&events, night, Some(&clip_id("anthem")), 120_000);
        // The last program ends before the sign-off and filler plays from
        // there to it, back to back, its last clip ending as it starts.
        let before: Vec<&Event> = events
            .iter()
            .filter(|e| e.end_ms <= night.start_ms)
            .collect();
        let last_program = before
            .iter()
            .rposition(|e| e.kind == EventKind::Program)
            .unwrap();
        let gap = &before[last_program + 1..];
        for pair in gap.windows(2) {
            assert_eq!(pair[0].end_ms, pair[1].start_ms);
        }
        if let Some(last) = gap.last() {
            assert_eq!(last.end_ms, night.start_ms);
            assert_eq!(last.media_offset_ms, 0);
            assert!(gap.iter().all(|e| e.kind == EventKind::Filler));
            assert_eq!(gap[0].start_ms, before[last_program].end_ms);
        }
        // The next program after the night is the next item in order.
        let order = ["a", "b", "c", "d", "e", "f", "g", "h"];
        let previous = before[last_program].media_id.as_str();
        let next = events
            .iter()
            .find(|e| e.start_ms == night.end_ms)
            .unwrap()
            .media_id
            .as_str();
        let p = order.iter().position(|id| *id == previous).unwrap();
        assert_eq!(next, order[(p + 1) % order.len()]);
    }
    // Every program that did not fit is still to come: no item is skipped.
    let programs = programs(&events);
    for pair in programs.windows(2) {
        let p = items.iter().position(|i| i.id.as_str() == pair[0]).unwrap();
        assert_eq!(pair[1], items[(p + 1) % items.len()].id.as_str());
    }
}

#[test]
fn the_sign_off_clip_is_the_named_one_else_the_first() {
    let clips = sign_off_pools();
    let anthem = clips[0].clips();
    assert_eq!(
        sign_off_clip(anthem, Some("anthem.mp4")),
        Some((MediaId::new(clip_id("anthem")), 120_000))
    );
    assert_eq!(
        sign_off_clip(anthem, None),
        Some((MediaId::new(clip_id("bars")), 60_000))
    );
    assert_eq!(
        sign_off_clip(anthem, Some("gone.mp4")),
        Some((MediaId::new(clip_id("bars")), 60_000))
    );
    assert_eq!(sign_off_clip(&[], Some("anthem.mp4")), None);

    // Through extend: without a name the first clip signs off every night.
    let items = pool(&["a", "b", "c"]);
    let mut channel = signed_off(Mode::Shuffle);
    channel.sign_off = Some(sign_off((23, 0), (6, 0), None));
    let now = local(2026, 6, 1, 12, 0);
    let events = unbroken(
        &channel,
        &pools_for(&channel, &items),
        &clips,
        now,
        now + 3 * DAY,
    )
    .events;
    for night in nights_in(&channel, now, now + 2 * DAY) {
        assert_night(&events, night, Some(&clip_id("bars")), 60_000);
    }
}

#[test]
fn without_a_usable_clip_the_channel_still_goes_off_the_air() {
    let items = pool(&["a", "b", "c"]);
    let channel = signed_off(Mode::Sequential);
    let clips = vec![
        ClipPool::new("anthem", "/clips", Vec::new(), Vec::new()),
        sign_off_pools().remove(1),
    ];
    let now = local(2026, 6, 1, 12, 0);
    let out = unbroken(
        &channel,
        &pools_for(&channel, &items),
        &clips,
        now,
        now + 3 * DAY,
    );

    assert!(out.notes.contains(&Note::NoSignOffClip));
    for night in nights_in(&channel, now, now + 2 * DAY) {
        assert_night(&out.events, night, None, 0);
    }
}

#[test]
fn without_filler_the_gap_before_the_sign_off_is_dead_air() {
    let items = pool(&["a", "b", "c", "d"]);
    let mut channel = signed_off(Mode::Sequential);
    channel.filler = None;
    let clips = sign_off_pools();
    let now = local(2026, 6, 1, 12, 0);
    let out = unbroken(
        &channel,
        &pools_for(&channel, &items),
        &clips,
        now,
        now + 3 * DAY,
    );

    assert!(out.events.iter().all(|e| e.kind != EventKind::Filler));
    assert!(out.notes.iter().any(|n| matches!(n, Note::DeadAir { .. })));
    for night in nights_in(&channel, now, now + 2 * DAY) {
        assert_night(&out.events, night, Some(&clip_id("anthem")), 120_000);
    }
}

#[test]
fn sign_offs_hold_on_both_clock_change_nights() {
    let items = pool(&["a", "b", "c", "d", "e"]);
    let clips = sign_off_pools();
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let channel = signed_off(mode);
        let pools = pools_for(&channel, &items);
        for (from, night) in [
            (
                at(2026, 3, 6, 12, 0),
                Night {
                    start_ms: at(2026, 3, 8, 5, 0),
                    end_ms: at(2026, 3, 8, 11, 0),
                },
            ),
            (
                at(2026, 10, 30, 12, 0),
                Night {
                    start_ms: at(2026, 11, 1, 4, 0),
                    end_ms: at(2026, 11, 1, 12, 0),
                },
            ),
        ] {
            let out = unbroken(&channel, &pools, &clips, from, from + 4 * DAY);
            assert_night(&out.events, night, Some(&clip_id("anthem")), 120_000);
        }
    }
}

#[test]
fn a_blocks_channel_signs_off_over_its_blocks_and_default() {
    // Drama 18:30 to 23:00 on Monday, Wednesday and Friday and a movie
    // default the rest of the time; the sign-off at 22:00 cuts into both.
    let mut channel = blocks_channel_with_breaks();
    channel.sign_off = Some(sign_off((22, 0), (6, 0), Some("anthem.mp4")));
    let pools = blocks_pools();
    let mut clips = clip_pools();
    clips.push(sign_off_pools().remove(0));
    let now = local(2026, 10, 28, 12, 0);
    let mut store = setup();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    let events = all_events(&store);

    let found = nights_in(&channel, now, now + HORIZON_MS);
    assert_eq!(found.len(), 7);
    for night in found {
        assert_night(&events, night, Some(&clip_id("anthem")), 120_000);
    }
    // The drama block still starts on time on Friday, October 30.
    let drama = events
        .iter()
        .find(|e| e.start_ms == local(2026, 10, 30, 18, 30))
        .unwrap();
    assert!(drama.media_id.as_str().starts_with('d'));
}

/// The places where a run may stop and the next one start, around the
/// second night in `events`: the last program's end, inside the filler gap,
/// the sign-off, inside its clip, off the air, the return and just after it.
fn stops(events: &[Event], night: Night) -> Vec<i64> {
    let last_program_end = events
        .iter()
        .filter(|e| e.kind == EventKind::Program && e.end_ms <= night.start_ms)
        .map(|e| e.end_ms)
        .max()
        .unwrap();
    vec![
        last_program_end,
        last_program_end + 1,
        (last_program_end + night.start_ms) / 2,
        night.start_ms - 1,
        night.start_ms,
        night.start_ms + 30_000,
        night.start_ms + 3_600_000,
        night.end_ms - 1,
        night.end_ms,
        night.end_ms + 60_000,
    ]
}

#[test]
fn an_extend_in_two_parts_matches_one_run_at_every_kind_of_stop() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let mut clips = clip_pools();
    clips.push(sign_off_pools().remove(0));
    let start = local(2026, 6, 1, 12, 0);
    for mode in [Mode::Sequential, Mode::Shuffle, Mode::Weighted] {
        for filler in [true, false] {
            let mut channel = channel_with_breaks(mode);
            channel.sign_off = Some(sign_off((23, 0), (6, 0), Some("anthem.mp4")));
            if !filler {
                channel.filler = None;
            }
            let pools = pools_for(&channel, &items);
            let reference = unbroken(&channel, &pools, &clips, start, start + 3 * DAY).events;
            let night = nights_in(&channel, start, start + 3 * DAY)[1];
            for stop in stops(&reference, night) {
                // The first run reaches `stop`; the second, an hour later,
                // continues from where it ended.
                let first = stop - HORIZON_MS;
                let second = first + 60 * MINUTE;
                let mut store = setup();
                extend(&mut store, &channel, &pools, &clips, CHICAGO, first, true).unwrap();
                extend(&mut store, &channel, &pools, &clips, CHICAGO, second, true).unwrap();
                let whole = unbroken(&channel, &pools, &clips, first, second + HORIZON_MS);
                assert_eq!(
                    all_events(&store),
                    whole.events,
                    "{mode:?}, filler {filler}, stop {}",
                    stop - night.start_ms
                );
            }
        }
    }
}

#[test]
fn a_run_after_an_outage_places_programs_when_they_fit_before_the_sign_off() {
    // The stored schedule runs out on a program in the afternoon and the Pi
    // is off for 20 minutes: the run then starts in the stretch before the
    // sign-off, after a program that ended earlier in it. Programs still fit,
    // so it is not the gap.
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let channel = signed_off(mode);
        let clips = sign_off_pools();
        let pools = pools_for(&channel, &items);
        let mut store = setup();
        let now = local(2026, 6, 1, 12, 0);
        extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
        let last = all_events(&store).pop().unwrap();
        assert_eq!(last.kind, EventKind::Program);
        let back = last.end_ms + 20 * MINUTE;
        let night = nights_in(&channel, now, back + DAY)
            .into_iter()
            .find(|night| night.start_ms > back)
            .unwrap();
        assert!(night.start_ms - back > 6 * 60 * MINUTE, "{mode:?}");

        extend(&mut store, &channel, &pools, &clips, CHICAGO, back, true).unwrap();
        let events = all_events(&store);
        let first = events.iter().find(|e| e.start_ms >= back).unwrap();
        assert_eq!((first.start_ms, first.kind), (back, EventKind::Program));
        assert_night(&events, night, Some(&clip_id("anthem")), 120_000);
        // The gap is shorter than the longest program, 69 minutes.
        let gap_start = events
            .iter()
            .filter(|e| e.kind == EventKind::Program && e.end_ms <= night.start_ms)
            .map(|e| e.end_ms)
            .max()
            .unwrap();
        assert!(night.start_ms - gap_start < 69 * MINUTE, "{mode:?}");
    }
}

#[test]
fn a_blocks_extend_in_two_parts_matches_one_run_around_a_sign_off() {
    let mut channel = blocks_channel_with_breaks();
    channel.sign_off = Some(sign_off((22, 0), (6, 0), None));
    let pools = blocks_pools();
    let mut clips = clip_pools();
    clips.push(sign_off_pools().remove(0));
    // Through the 25-hour day, Sunday November 1.
    let start = local(2026, 10, 29, 12, 0);
    let reference = unbroken(&channel, &pools, &clips, start, start + 4 * DAY).events;
    for night in nights_in(&channel, start, start + 4 * DAY) {
        for stop in stops(&reference, night) {
            let first = stop - HORIZON_MS;
            let second = first + 60 * MINUTE;
            let mut store = setup();
            extend(&mut store, &channel, &pools, &clips, CHICAGO, first, true).unwrap();
            extend(&mut store, &channel, &pools, &clips, CHICAGO, second, true).unwrap();
            let whole = unbroken(&channel, &pools, &clips, first, second + HORIZON_MS);
            assert_eq!(all_events(&store), whole.events, "stop {stop}");
        }
    }
}

#[test]
fn a_regeneration_with_unchanged_inputs_changes_nothing_around_a_sign_off() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let mut clips = clip_pools();
    clips.push(sign_off_pools().remove(0));
    let start = local(2026, 6, 1, 12, 0);
    for mode in [Mode::Sequential, Mode::Shuffle, Mode::Weighted] {
        for filler in [true, false] {
            let mut channel = channel_with_breaks(mode);
            channel.sign_off = Some(sign_off((23, 0), (6, 0), Some("anthem.mp4")));
            if !filler {
                channel.filler = None;
            }
            let pools = pools_for(&channel, &items);
            let mut store = setup();
            extend(&mut store, &channel, &pools, &clips, CHICAGO, start, true).unwrap();
            let night = nights_in(&channel, start, start + 3 * DAY)[1];
            for stop in stops(&all_events(&store), night) {
                // The freeze point falls at `stop`.
                let now = stop - FREEZE_MS;
                extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
                let before = all_events(&store);
                let result =
                    regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
                let from = regenerated_from(&result);
                assert!(from >= stop, "{from} {stop}");
                assert_eq!(
                    all_events(&store),
                    before,
                    "{mode:?}, filler {filler}, stop {}",
                    stop - night.start_ms
                );
            }
        }
    }
}

#[test]
fn a_regeneration_starts_at_the_sign_off_clip_and_inside_the_gap() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let channel = signed_off(Mode::Shuffle);
    let clips = sign_off_pools();
    let pools = pools_for(&channel, &items);
    let start = local(2026, 6, 1, 12, 0);
    let mut store = setup();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, start, true).unwrap();
    let night = nights_in(&channel, start, start + 3 * DAY)[1];
    let gap: Vec<i64> = all_events(&store)
        .iter()
        .filter(|e| e.kind == EventKind::Filler && e.end_ms <= night.start_ms)
        .filter(|e| e.start_ms > night.start_ms - 2 * 60 * MINUTE)
        .map(|e| e.start_ms)
        .collect();
    assert!(
        gap.len() >= 2,
        "the second night has a gap of two clips or more"
    );

    // A freeze point in the gap, before its second filler clip, reaches it,
    // and the run there stays in the gap.
    let now = gap[1] - FREEZE_MS;
    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    let events = all_events(&store);
    let result = regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    assert_eq!(regenerated_from(&result), gap[1]);
    assert_eq!(all_events(&store), events);

    // A freeze point just before the sign-off reaches the clip itself.
    let now = night.start_ms - 1 - FREEZE_MS;
    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    let events = all_events(&store);
    let result = regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    assert_eq!(regenerated_from(&result), night.start_ms);
    assert_eq!(all_events(&store), events);
}

#[test]
fn a_regeneration_in_dead_air_from_the_start_of_the_stretch_changes_nothing() {
    // On the air only from 06:00 to 06:30. Without filler, when the first
    // pick does not fit, the stretch is dead air from its start, and the last
    // stored event is the sign-off clip of the night before. A run from a
    // freeze point in that dead air must not place a program because a
    // shuffle or weighted pick at that time would fit.
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let clips = sign_off_pools();
    let start = local(2026, 6, 1, 12, 0);
    for mode in [Mode::Sequential, Mode::Shuffle, Mode::Weighted] {
        for filler in [false, true] {
            let mut channel = signed_off(mode);
            channel.sign_off = Some(sign_off((6, 30), (6, 0), Some("anthem.mp4")));
            if !filler {
                channel.filler = None;
            }
            let pools = pools_for(&channel, &items);
            for day in 1..4 {
                for minute in (0..30).step_by(3) {
                    let now = local(2026, 6, 1 + day, 6, minute) - FREEZE_MS;
                    let mut store = setup();
                    extend(&mut store, &channel, &pools, &clips, CHICAGO, start, true).unwrap();
                    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
                    let before = all_events(&store);
                    regenerate(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
                    assert_eq!(
                        all_events(&store),
                        before,
                        "{mode:?}, filler {filler}, day {day}, 06:{minute:02}"
                    );
                }
            }
        }
    }
}
