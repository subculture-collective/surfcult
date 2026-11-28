//! Interstitial kinds: weighted break pools, opening and closing clips, the
//! event kind each pool's clips are scheduled as, and `between`.

use super::*;
use crate::config::{Between, SignOff};
use crate::schedule::breaks::BetweenPlan;
use crate::schedule::extend::fill;

/// `name` in pool order, with no repeat limit, of `kind`.
fn named_pool(name: &str, kind: ClipKind, rotation: Rotation) -> PoolUse {
    PoolUse {
        rotation,
        ..pool_use(name, kind)
    }
}

/// The owner's clip pools: 16 bumps of 11 s, 6 stingers of 4 s, 5 ads of
/// 30 s, 3 trailers of 50 s, 2 idents of 20 s and a filler slate.
fn tv_clips() -> Vec<ClipPool> {
    let many = |prefix: &str, n: usize, seconds: u64| -> Vec<Clip> {
        (0..n)
            .map(|i| clip(&format!("{prefix}{i:02}"), seconds))
            .collect()
    };
    vec![
        ClipPool::new("bumps", "/clips", many("bump", 16, 11), Vec::new()),
        ClipPool::new("stingers", "/clips", many("sting", 6, 4), Vec::new()),
        ClipPool::new("ads", "/clips", many("ad", 5, 30), Vec::new()),
        ClipPool::new("trailers", "/clips", many("trailer", 3, 50), Vec::new()),
        ClipPool::new("idents", "/clips", many("ident", 2, 20), Vec::new()),
        ClipPool::new("filler", "/clips", vec![clip("slate", 90)], Vec::new()),
    ]
}

/// A cartoon channel: bump, commercials and trailers by weight 3 to 1,
/// padded from `idents`, then a stinger; a bump between programs.
fn toons(mode: Mode) -> Channel {
    let mut channel = channel(3, mode);
    channel.breaks = Some(Breaks {
        every: Duration::from_secs(11 * 60),
        length: Duration::from_secs(120),
        fill: Interlude {
            pools: vec![
                WeightedPool {
                    pool: PoolUse {
                        repeat_after: Some(Duration::from_secs(30 * 60)),
                        ..pool_use("ads", ClipKind::Commercial)
                    },
                    weight: 3,
                },
                WeightedPool {
                    pool: named_pool("trailers", ClipKind::Trailer, Rotation::Shuffle),
                    weight: 1,
                },
            ],
            open: Some(named_pool("bumps", ClipKind::Bump, Rotation::Shuffle)),
            close: Some(pool_use("stingers", ClipKind::Stinger)),
        },
        pad: Some("idents".into()),
    });
    channel.between = Some(Between {
        length: None,
        fill: Interlude {
            pools: vec![WeightedPool {
                pool: named_pool("bumps", ClipKind::Bump, Rotation::Shuffle),
                weight: 1,
            }],
            open: None,
            close: None,
        },
    });
    channel.filler = Some("filler".into());
    channel
}

fn sources(channel: &Channel, items: &[MediaItem]) -> Vec<(Source, Vec<MediaItem>)> {
    channel
        .plan
        .sources()
        .into_iter()
        .map(|source| (source.clone(), items.to_vec()))
        .collect()
}

fn is_bump(event: &Event) -> bool {
    event.media_id.as_str().starts_with("/clips/bump")
}

/// The clips of each break and each `between`: the runs of events that are
/// not programs.
fn interludes(events: &[Event]) -> Vec<&[Event]> {
    events
        .split(|e| e.kind == EventKind::Program)
        .filter(|run| !run.is_empty())
        .collect()
}

#[test]
fn a_break_opens_fills_by_weight_pads_and_closes_with_each_kind() {
    let items = pool(&["a", "b", "c", "d"]);
    let channel = toons(Mode::Sequential);
    let start = at(2026, 6, 1, 12, 0);
    let out = fill(
        &setup(),
        &channel,
        &sources(&channel, &items),
        &tv_clips(),
        Tz::UTC,
        start,
        start + DAY,
    )
    .unwrap();
    assert!(out.notes.is_empty(), "{:?}", out.notes);
    assert_eq!(out.events[0].start_ms, start);
    for pair in out.events.windows(2) {
        assert_eq!(pair[0].end_ms, pair[1].start_ms, "gap or overlap");
    }

    let mut breaks = 0;
    let mut betweens = 0;
    for run in interludes(&out.events) {
        let length: i64 = run.iter().map(|e| e.end_ms - e.start_ms).sum();
        if run.len() == 1 {
            // Between two programs: one bump, as an ident.
            betweens += 1;
            assert!(is_bump(&run[0]), "{run:?}");
            assert_eq!(run[0].kind, EventKind::Ident);
            assert_eq!(length, 11_000);
            continue;
        }
        breaks += 1;
        assert_eq!(length, 120_000, "{run:?}");
        let (first, last) = (&run[0], run.last().unwrap());
        assert!(is_bump(first) && first.kind == EventKind::Ident, "{run:?}");
        assert!(last.media_id.as_str().starts_with("/clips/sting"));
        assert_eq!(last.kind, EventKind::Ident);
        for event in &run[1..run.len() - 1] {
            let id = event.media_id.as_str();
            let expected = if id.starts_with("/clips/ad") || id.starts_with("/clips/trailer") {
                EventKind::Break
            } else {
                assert!(id.starts_with("/clips/ident"), "{run:?}");
                EventKind::Ident
            };
            assert_eq!(event.kind, expected, "{id}");
        }
        // No clip plays twice in one break.
        let mut ids: Vec<&str> = run.iter().map(|e| e.media_id.as_str()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), run.len(), "{run:?}");
    }
    assert!(
        breaks > 50 && betweens > 20,
        "{breaks} breaks, {betweens} betweens"
    );
    // The first program of the run has nothing before it.
    assert_eq!(out.events[0].kind, EventKind::Program);
    // Trailers do play, as breaks.
    assert!(
        out.events.iter().any(
            |e| e.media_id.as_str().starts_with("/clips/trailer") && e.kind == EventKind::Break
        )
    );
}

#[test]
fn weighted_break_pools_share_the_draws_by_weight() {
    let items = [item("show", 60)];
    // Each break has room for one 10 s clip, so each break is one draw.
    let a: Vec<Clip> = (0..40).map(|i| clip(&format!("a{i}"), 10)).collect();
    let b: Vec<Clip> = (0..40).map(|i| clip(&format!("b{i}"), 10)).collect();
    let mut breaks = plan(1, 10, &a, &[]);
    breaks.fill.pools = vec![(ads_pool("a", &a), 3), (ads_pool("b", &b), 1)];
    let params = with_breaks(Mode::Sequential, breaks);
    let end = 30 * DAY;

    let out = generate(&params, &single(&items), &Resume::default(), 0, end).unwrap();
    let drawn: Vec<&str> = out
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Break)
        .map(|e| e.media_id.as_str())
        .collect();
    let from_a = drawn.iter().filter(|id| id.starts_with("/clips/a")).count();
    let share = from_a as f64 / drawn.len() as f64;
    assert!(drawn.len() > 10_000, "{}", drawn.len());
    assert!((share - 0.75).abs() < 0.02, "share of a: {share}");
    // Each pool still rotates in its own order.
    let a_order: Vec<&str> = drawn
        .iter()
        .copied()
        .filter(|id| id.starts_with("/clips/a"))
        .take(41)
        .collect();
    assert_eq!(a_order[0], clip_id("a0"));
    assert_eq!(a_order[1], clip_id("a1"));
    assert_eq!(a_order[40], clip_id("a0"));
    // The same inputs draw the same breaks.
    let again = generate(&params, &single(&items), &Resume::default(), 0, end).unwrap();
    assert_eq!(again.events, out.events);
}

#[test]
fn between_plays_one_clip_or_fills_a_length_between_programs() {
    let items = pool(&["a", "b", "c"]);
    let bumps = [clip("bump1", 11), clip("bump2", 11)];
    let slate = [clip("slate", 60)];
    let between = |length_ms| BetweenPlan {
        length_ms,
        fill: breaks::Interlude::of(breaks::ClipPool {
            kind: EventKind::Ident,
            ..ads_pool("bumps", &bumps)
        }),
    };
    let params = Params {
        between: Some(between(None)),
        filler: &slate,
        ..params(Mode::Sequential)
    };
    let out = generate(
        &params,
        &single(&items),
        &Resume::default(),
        0,
        3 * 60 * MINUTE,
    )
    .unwrap();
    use EventKind::{Filler, Ident, Program};
    let s = 1_000;
    assert_eq!(
        shape(&out.events[..6]),
        [
            (Program, "a".into(), 0, 20 * MINUTE),
            (Ident, clip_id("bump1"), 0, 11 * s),
            (Program, "b".into(), 0, 27 * MINUTE),
            (Ident, clip_id("bump2"), 0, 11 * s),
            (Program, "c".into(), 0, 34 * MINUTE),
            (Ident, clip_id("bump1"), 0, 11 * s),
        ]
    );

    // With a length the bumps fill it and the channel's filler pads it.
    let params = Params {
        between: Some(between(Some(30 * SECOND))),
        ..params
    };
    let out = generate(&params, &single(&items), &Resume::default(), 0, 60 * MINUTE).unwrap();
    assert_eq!(
        shape(&out.events[1..4]),
        [
            (Ident, clip_id("bump1"), 0, 11 * s),
            (Ident, clip_id("bump2"), 0, 11 * s),
            (Filler, clip_id("slate"), 0, 8 * s),
        ]
    );
}

#[test]
fn between_skips_the_first_program_of_a_block() {
    let shows = named("s", 3, 25);
    let bumps = [clip("bump", 10)];
    let lane = solo(Mode::Sequential, &shows);
    let cut = 60 * MINUTE;
    let layout = Layout {
        channel: 3,
        grid_ms: None,
        breaks: None,
        between: Some(BetweenPlan {
            length_ms: None,
            fill: breaks::Interlude::of(breaks::ClipPool {
                kind: EventKind::Ident,
                ..ads_pool("bumps", &bumps)
            }),
        }),
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
    let out = generate_blocks(&layout, &Resume::default(), 0, 3 * cut).unwrap();
    let at_cut = out.events.iter().find(|e| e.start_ms == cut).unwrap();
    assert_eq!(at_cut.kind, EventKind::Program);
    assert_eq!(at_cut.media_offset_ms, 0);
    // Inside each block the bump plays between programs: 25 + 10 + 25 = 60
    // minutes and seconds, so the second program of the first block is cut.
    assert!(
        out.events
            .iter()
            .filter(|e| e.kind == EventKind::Ident)
            .count()
            >= 3
    );
    assert_eq!(out.events[1].kind, EventKind::Ident);
}

#[test]
fn between_does_not_cross_a_sign_off() {
    let items = pool(&["a", "b", "c", "d"]);
    let mut channel = toons(Mode::Sequential);
    channel.breaks = None;
    channel.sign_off = Some(SignOff {
        at_minute: 23 * 60,
        until_minute: 6 * 60,
        pool: "idents".into(),
        clip: None,
    });
    let start = at(2026, 6, 1, 12, 0);
    let out = fill(
        &setup(),
        &channel,
        &sources(&channel, &items),
        &tv_clips(),
        Tz::UTC,
        start,
        start + 3 * DAY,
    )
    .unwrap();
    for day in 1..3 {
        let back = at(2026, 6, 1 + day, 6, 0);
        let first = out.events.iter().find(|e| e.start_ms == back).unwrap();
        assert_eq!(first.kind, EventKind::Program, "day {day}");
        let sign_off = at(2026, 6, day, 23, 0);
        let i = out
            .events
            .iter()
            .position(|e| e.start_ms == sign_off)
            .unwrap();
        assert!(
            !is_bump(&out.events[i - 1]),
            "a bump right before the sign-off"
        );
    }
    assert!(out.events.iter().any(is_bump));
}

/// Runs resumed at every boundary of an unbroken run of `params`: at each
/// program's start (after its `between`) and at each `between`'s start (after
/// the program before it).
fn assert_resumes_match(params: &Params<'_>, items: &[MediaItem], start: i64, end: i64) {
    let whole = generate(params, &single(items), &Resume::default(), start, end).unwrap();
    let boundaries: Vec<usize> = (1..whole.events.len())
        .filter(|&i| {
            let (before, e) = (&whole.events[i - 1], &whole.events[i]);
            let ends_program = before.kind == EventKind::Program
                && before.media_offset_ms + before.end_ms.abs_diff(before.start_ms)
                    == before.media_duration_ms;
            (e.kind == EventKind::Program && e.media_offset_ms == 0)
                || (e.kind != EventKind::Program && ends_program)
        })
        .collect();
    assert!(boundaries.len() > 40, "{}", boundaries.len());
    for &cut in boundaries.iter().step_by(3) {
        let boundary = whole.events[cut].start_ms;
        let resume = resume_after(&whole.events[..cut]);
        let rest = generate(params, &single(items), &resume, boundary, end).unwrap();
        if let Some(i) =
            (0..rest.events.len()).find(|&i| whole.events.get(cut + i) != Some(&rest.events[i]))
        {
            panic!(
                "cut at {cut}, differs at +{i}:\nbefore {:?}\nwhole {:?}\nrest  {:?}",
                &whole.events[(cut + i).saturating_sub(4)..cut + i],
                &whole.events[cut + i..(cut + i + 3).min(whole.events.len())],
                &rest.events[i..(i + 3).min(rest.events.len())]
            );
        }
        assert_eq!(rest.events, whole.events[cut..], "cut at {cut}");
    }
}

#[test]
fn a_run_resumed_at_a_program_or_between_matches_an_unbroken_run() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g"]);
    let clips = tv_clips();
    let pool_of = |i: usize, kind, rotation, repeat_after_ms| breaks::ClipPool {
        name: clips[i].name(),
        clips: clips[i].clips(),
        rotation,
        repeat_after_ms,
        kind,
    };
    let bumps = pool_of(0, EventKind::Ident, Rotation::Shuffle, Some(40 * MINUTE));
    let plan = BreakPlan {
        every_ms: 11 * 60 * SECOND,
        length_ms: 120 * SECOND,
        fill: breaks::Interlude {
            pools: vec![
                (
                    pool_of(2, EventKind::Break, Rotation::Order, Some(30 * MINUTE)),
                    3,
                ),
                (pool_of(3, EventKind::Break, Rotation::Shuffle, None), 1),
            ],
            open: Some(bumps),
            close: Some(pool_of(1, EventKind::Ident, Rotation::Order, None)),
        },
        idents: clips[4].clips(),
        filler: clips[5].clips(),
    };
    let start = at(2026, 6, 1, 12, 0);
    for length_ms in [None, Some(45 * SECOND)] {
        let params = Params {
            between: Some(BetweenPlan {
                length_ms,
                fill: breaks::Interlude::of(bumps),
            }),
            filler: clips[5].clips(),
            ..with_breaks(Mode::Shuffle, plan.clone())
        };
        assert_resumes_match(&params, &items, start, start + 2 * DAY);
    }
}

#[test]
fn interstitials_survive_an_extend_restart_and_a_regeneration() {
    let items = pool(&["a", "b", "c", "d", "e", "f", "g", "h"]);
    let clips = tv_clips();
    let now = at(2026, 6, 1, 12, 0);
    let later = now + 2 * DAY + 11 * MINUTE;
    for mode in [Mode::Sequential, Mode::Shuffle] {
        let channel = toons(mode);
        let pools = sources(&channel, &items);
        let mut store = setup();

        extend(&mut store, &channel, &pools, &clips, Tz::UTC, now, true).unwrap();
        extend(&mut store, &channel, &pools, &clips, Tz::UTC, later, true).unwrap();

        let whole = fill(
            &setup(),
            &channel,
            &pools,
            &clips,
            Tz::UTC,
            now,
            later + HORIZON_MS,
        )
        .unwrap();
        let stored = all_events(&store);
        assert_eq!(stored, whole.events, "{mode:?}");

        let result =
            regenerate(&mut store, &channel, &pools, &clips, Tz::UTC, later, true).unwrap();
        regenerated_from(&result);
        assert_eq!(all_events(&store), stored, "{mode:?}");
    }
}

#[test]
fn a_blocks_channel_with_interstitials_extended_twice_matches_one_unbroken_run() {
    let pools = blocks_pools();
    let clips = tv_clips();
    let mut channel = blocks_channel_with_breaks();
    let interstitials = toons(Mode::Sequential);
    channel.breaks = interstitials.breaks;
    channel.between = interstitials.between;
    // A Saturday, so the cartoon block starts and ends inside the window.
    let now = at(2026, 6, 6, 3, 0);
    let later = now + DAY + 7 * MINUTE;
    let mut store = setup();

    extend(&mut store, &channel, &pools, &clips, CHICAGO, now, true).unwrap();
    extend(&mut store, &channel, &pools, &clips, CHICAGO, later, true).unwrap();

    let whole = fill(
        &setup(),
        &channel,
        &pools,
        &clips,
        CHICAGO,
        now,
        later + HORIZON_MS,
    )
    .unwrap();
    assert_eq!(all_events(&store), whole.events);
    assert!(whole.events.iter().any(is_bump));
}
