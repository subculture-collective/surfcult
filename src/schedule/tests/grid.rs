//! Grids: programs start on the grid and the gap before each is filled.

use super::*;

const HALF_HOUR: u64 = 30 * 60 * SECOND;

fn grid_params<'a>(breaks: Option<BreakPlan<'a>>, filler: &'a [Clip]) -> Params<'a> {
    Params {
        grid_ms: Some(HALF_HOUR),
        filler,
        breaks,
        ..params(Mode::Sequential)
    }
}

fn gap_plan<'a>(clips: &'a [Clip], idents: &'a [Clip], filler: &'a [Clip]) -> BreakPlan<'a> {
    BreakPlan {
        idents,
        ..plan(600, 0, clips, filler)
    }
}

#[test]
fn every_program_starts_on_a_half_hour() {
    let items = [item("a", 22), item("b", 47), item("c", 30), item("d", 5)];
    let filler = [clip("f", 60)];
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &grid_params(None, &filler),
        &single(&items),
        &Resume::default(),
        start,
        start + 2 * DAY,
    )
    .unwrap();

    let starts: Vec<_> = out
        .events
        .iter()
        .filter(|e| e.kind == EventKind::Program)
        .map(|e| e.start_ms)
        .collect();
    assert!(starts.len() > 50);
    assert!(starts.iter().all(|ms| ms % (30 * MINUTE) == 0));
    assert_covers(&out.events, start, start + 2 * DAY);
    // The 22-minute program is followed by filler up to the half hour.
    assert_eq!(out.events[1].kind, EventKind::Filler);
    assert_eq!(out.events[1].start_ms, start + 22 * MINUTE);
}

#[test]
fn a_gap_is_filled_from_the_break_pool_then_idents_then_filler() {
    // An 8-minute gap after a 22-minute program.
    let items = [item("a", 22)];
    let ads = [clip("ad1", 120), clip("ad2", 100), clip("long", 900)];
    let idents = [clip("id", 90)];
    let filler = [clip("fl", 60)];
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &grid_params(Some(gap_plan(&ads, &idents, &filler)), &filler),
        &single(&items),
        &Resume::default(),
        start,
        start + 30 * MINUTE + 1,
    )
    .unwrap();

    let gap: Vec<_> = shape(&out.events[1..6]);
    assert_eq!(
        gap,
        [
            (EventKind::Break, clip_id("ad1"), 0, 120_000),
            (EventKind::Break, clip_id("ad2"), 0, 100_000),
            (EventKind::Ident, clip_id("id"), 0, 90_000),
            (EventKind::Filler, clip_id("fl"), 0, 60_000),
            // The last resort: one ident covers what is left.
            (EventKind::Ident, clip_id("id"), 0, 110_000),
        ]
    );
    assert_eq!(out.events[6].kind, EventKind::Program);
    assert_eq!(out.events[6].start_ms, start + 30 * MINUTE);
}

#[test]
fn a_run_that_starts_off_the_grid_fills_up_to_it() {
    let items = [item("a", 22)];
    let filler = [clip("fl", 300)];
    let start = at(2026, 6, 1, 12, 10);

    let out = generate(
        &grid_params(None, &filler),
        &single(&items),
        &Resume::default(),
        start,
        start + 30 * MINUTE,
    )
    .unwrap();

    assert_eq!(out.events[0].kind, EventKind::Filler);
    assert_eq!(out.events[0].start_ms, start);
    assert_eq!(out.events[0].end_ms, start + 5 * MINUTE);
    let program = out.events.iter().find(|e| e.kind == EventKind::Program);
    assert_eq!(program.unwrap().start_ms, at(2026, 6, 1, 12, 30));
}

#[test]
fn a_gap_with_no_clip_is_dead_air() {
    let items = [item("a", 22)];
    let start = at(2026, 6, 1, 12, 0);

    let out = generate(
        &grid_params(None, &[]),
        &single(&items),
        &Resume::default(),
        start,
        start + 65 * MINUTE,
    )
    .unwrap();

    let starts: Vec<_> = out.events.iter().map(|e| e.start_ms).collect();
    assert_eq!(starts, [start, start + 30 * MINUTE, start + 60 * MINUTE]);
    assert_eq!(out.notes, [Note::DeadAir { ms: 2 * 8 * 60_000 }]);
}
