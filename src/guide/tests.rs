use super::*;
use crate::library::{FakeMediaSource, MediaItem, MediaKind};

const MINUTE: i64 = 60_000;
const HOUR: i64 = 60 * MINUTE;
/// A start time well away from zero, so offsets and times cannot be confused.
const T: i64 = 1_000 * HOUR;

fn media() -> FakeMediaSource {
    FakeMediaSource::new()
        .with_item(MediaItem::new(
            "a",
            "Show A",
            MediaKind::Episode,
            30 * MINUTE as u64,
        ))
        .with_item(MediaItem::new(
            "b",
            "Show B",
            MediaKind::Episode,
            20 * MINUTE as u64,
        ))
}

/// A program segment of `media` from `start` to `end` minutes after `T`,
/// starting `offset` minutes into the media.
fn program(media: &str, start: i64, end: i64, offset: i64, duration: i64) -> Event {
    Event {
        start_ms: T + start * MINUTE,
        end_ms: T + end * MINUTE,
        media_id: MediaId::new(media),
        media_offset_ms: (offset * MINUTE) as u64,
        media_duration_ms: (duration * MINUTE) as u64,
        kind: EventKind::Program,
    }
}

fn other(kind: EventKind, start: i64, end: i64) -> Event {
    Event {
        kind,
        ..program("clip", start, end, 0, end - start)
    }
}

/// Show A (30 min) cut by two breaks, then an ident, then Show B whole:
///
/// ```text
/// 0-10 a@0   10-12 break   12-22 a@10   22-24 break   24-34 a@20
/// 34-35 ident   35-55 b@0
/// ```
fn schedule() -> Vec<Event> {
    vec![
        program("a", 0, 10, 0, 30),
        other(EventKind::Break, 10, 12),
        program("a", 12, 22, 10, 30),
        other(EventKind::Break, 22, 24),
        program("a", 24, 34, 20, 30),
        other(EventKind::Ident, 34, 35),
        program("b", 35, 55, 0, 20),
    ]
}

fn window(from: i64, to: i64) -> Window {
    Window {
        from_ms: T + from * MINUTE,
        to_ms: T + to * MINUTE,
    }
}

/// Each cell as (media id, start minute, end minute).
fn spans(cells: &[Cell]) -> Vec<(&str, i64, i64)> {
    cells
        .iter()
        .map(|cell| {
            (
                cell.media_id.as_str(),
                (cell.start_ms - T) / MINUTE,
                (cell.end_ms - T) / MINUTE,
            )
        })
        .collect()
}

#[test]
fn a_program_split_by_breaks_is_one_cell_from_first_start_to_last_end() {
    let cells = cells(&schedule(), &media(), window(0, 180));
    assert_eq!(spans(&cells), vec![("a", 0, 34), ("b", 35, 55)]);
}

#[test]
fn breaks_filler_and_idents_are_not_cells() {
    let events = vec![
        other(EventKind::Filler, 0, 5),
        other(EventKind::Ident, 5, 6),
        program("b", 6, 26, 0, 20),
        other(EventKind::Break, 26, 28),
    ];
    let cells = cells(&events, &media(), window(0, 180));
    assert_eq!(spans(&cells), vec![("b", 6, 26)]);
    assert!(super::cells(&events[..2], &media(), window(0, 180)).is_empty());
}

#[test]
fn cells_carry_the_title_and_no_description() {
    let cells = cells(&schedule(), &media(), window(0, 180));
    assert_eq!(cells[0].title.as_deref(), Some("Show A"));
    assert_eq!(cells[1].title.as_deref(), Some("Show B"));
    assert!(cells.iter().all(|cell| cell.description.is_none()));
}

#[test]
fn an_item_the_source_cannot_resolve_has_no_title() {
    let events = vec![program("gone", 0, 10, 0, 10)];
    let cells = cells(&events, &media(), window(0, 180));
    assert_eq!(spans(&cells), vec![("gone", 0, 10)]);
    assert_eq!(cells[0].title, None);
}

#[test]
fn the_same_item_played_again_from_the_start_is_a_new_cell() {
    let events = vec![program("b", 0, 20, 0, 20), program("b", 20, 40, 0, 20)];
    let cells = cells(&events, &media(), window(0, 180));
    assert_eq!(spans(&cells), vec![("b", 0, 20), ("b", 20, 40)]);
}

#[test]
fn segments_do_not_join_across_a_hole_or_another_program() {
    let hole = vec![program("a", 0, 10, 0, 30), program("a", 15, 35, 10, 30)];
    assert_eq!(
        spans(&cells(&hole, &media(), window(0, 180))),
        vec![("a", 0, 10), ("a", 15, 35)]
    );

    let between = vec![
        program("a", 0, 10, 0, 30),
        program("b", 10, 30, 0, 20),
        program("a", 30, 50, 10, 30),
    ];
    assert_eq!(
        spans(&cells(&between, &media(), window(0, 180))),
        vec![("a", 0, 10), ("b", 10, 30), ("a", 30, 50)]
    );
}

#[test]
fn only_cells_overlapping_the_window_are_kept_and_are_not_clipped() {
    // From inside A's second break to just after B starts.
    let cells = cells(&schedule(), &media(), window(23, 36));
    assert_eq!(spans(&cells), vec![("a", 0, 34), ("b", 35, 55)]);
    // A window that ends exactly when B starts leaves B out; one starting
    // exactly when A ends leaves A out.
    assert_eq!(
        spans(&super::cells(&schedule(), &media(), window(0, 35))),
        vec![("a", 0, 34)]
    );
    assert_eq!(
        spans(&super::cells(&schedule(), &media(), window(34, 60))),
        vec![("b", 35, 55)]
    );
}

#[test]
fn the_default_window_is_now_through_three_hours() {
    assert_eq!(
        Window::starting_at(T),
        Window {
            from_ms: T,
            to_ms: T + 3 * HOUR
        }
    );
}

#[test]
fn progress_of_the_current_program_counts_its_breaks() {
    let cells = cells(&schedule(), &media(), window(0, 180));
    let a = &cells[0];
    // A runs 0 to 34 minutes.
    assert_eq!(a.progress(T), Some(0.0));
    assert_eq!(a.progress(T + 17 * MINUTE), Some(0.5));
    // Inside a break the program is still the current one.
    assert_eq!(a.progress(T + 11 * MINUTE), Some(11.0 / 34.0));
    assert_eq!(a.progress(T - 1), None);
    assert_eq!(a.progress(T + 34 * MINUTE), None);
}

#[test]
fn the_current_cell_is_the_one_on_at_the_time() {
    let row = Row {
        channel: 3,
        cells: cells(&schedule(), &media(), window(0, 180)),
    };
    assert_eq!(row.current(T + 23 * MINUTE).unwrap().media_id.as_str(), "a");
    assert_eq!(row.current(T + 40 * MINUTE).unwrap().media_id.as_str(), "b");
    // The ident between A and B belongs to neither.
    assert_eq!(row.current(T + 34 * MINUTE), None);
    assert_eq!(row.current(T + 55 * MINUTE), None);
}

fn stored(events: &[Event]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(3, events).unwrap();
    store
}

#[test]
fn build_reads_a_split_program_that_crosses_the_window_start() {
    let store = stored(&schedule());
    // The window starts in A's second break: the store returns that break
    // onwards, and build reads back to A's first segment.
    let rows = build(&store, &media(), &[3], window(23, 180)).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].channel, 3);
    assert_eq!(spans(&rows[0].cells), vec![("a", 0, 34), ("b", 35, 55)]);
    assert_eq!(rows[0].cells[0].title.as_deref(), Some("Show A"));
    // Progress at the window start is through the whole program.
    assert_eq!(
        rows[0]
            .current(T + 23 * MINUTE)
            .unwrap()
            .progress(T + 23 * MINUTE),
        Some(23.0 / 34.0)
    );
}

#[test]
fn build_reads_a_split_program_that_crosses_the_window_end() {
    let store = stored(&schedule());
    // The window ends in A's first segment: build reads on to its last one.
    let rows = build(&store, &media(), &[3], window(0, 5)).unwrap();
    assert_eq!(spans(&rows[0].cells), vec![("a", 0, 34)]);
}

#[test]
fn build_stops_reading_at_a_hole() {
    // A's later segments are missing, so its cell ends where the stored
    // events do.
    let store = stored(&[
        program("a", 0, 10, 0, 30),
        other(EventKind::Break, 10, 12),
        program("a", 50, 60, 10, 30),
    ]);
    let rows = build(&store, &media(), &[3], window(0, 5)).unwrap();
    assert_eq!(spans(&rows[0].cells), vec![("a", 0, 10)]);
}

#[test]
fn build_gives_one_row_per_channel_in_order() {
    let mut store = stored(&schedule());
    store
        .write_events(7, &[program("b", 0, 20, 0, 20)])
        .unwrap();
    let rows = build(&store, &media(), &[7, 9, 3], window(0, 180)).unwrap();
    let channels: Vec<u32> = rows.iter().map(|row| row.channel).collect();
    assert_eq!(channels, vec![7, 9, 3]);
    assert_eq!(spans(&rows[0].cells), vec![("b", 0, 20)]);
    assert!(rows[1].cells.is_empty());
    assert_eq!(spans(&rows[2].cells), vec![("a", 0, 34), ("b", 35, 55)]);
}
