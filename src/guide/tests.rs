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

// Music channels: songs grouped into hour blocks.

use crate::config::{Channel, Mode, Picture, Plan, Tz};
use chrono::TimeZone;

/// A song by `artists`, `minutes` long.
fn song(id: &str, artists: &[&str], minutes: i64) -> MediaItem {
    let mut item = MediaItem::new(id, id, MediaKind::Song, (minutes * MINUTE) as u64);
    item.artists = artists.iter().map(|a| a.to_string()).collect();
    item.genres = artists
        .first()
        .map(|a| format!("{a} genre"))
        .into_iter()
        .collect();
    item
}

fn songs() -> FakeMediaSource {
    FakeMediaSource::new()
        .with_item(song("santana", &["Santana"], 5))
        .with_item(song("who", &["The Who"], 5))
        .with_item(song("cream", &["Cream"], 5))
        .with_item(song("doors", &["The Doors"], 5))
        .with_item(song("duet", &["Santana", "Cream"], 5))
        .with_item(song("nobody", &[], 5))
}

/// A whole song from `start` to `end` milliseconds.
fn play(media: &str, start: i64, end: i64) -> Event {
    Event {
        start_ms: start,
        end_ms: end,
        media_id: MediaId::new(media),
        media_offset_ms: 0,
        media_duration_ms: (end - start) as u64,
        kind: EventKind::Program,
    }
}

fn bump(start: i64, end: i64) -> Event {
    Event {
        kind: EventKind::Ident,
        ..play("bump", start, end)
    }
}

/// `count` songs from `start`, each `length` long with a `gap` bump after
/// it, cycling through `ids`.
fn music(ids: &[&str], start: i64, count: usize, length: i64, gap: i64) -> Vec<Event> {
    let mut events = Vec::new();
    let mut at = start;
    for i in 0..count {
        events.push(play(ids[i % ids.len()], at, at + length));
        at += length;
        if gap > 0 {
            events.push(bump(at, at + gap));
            at += gap;
        }
    }
    events
}

fn local(tz: Tz, y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    tz.with_ymd_and_hms(y, mo, d, h, mi, 0)
        .earliest()
        .unwrap()
        .timestamp_millis()
}

fn utc(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
    local(Tz::UTC, y, mo, d, h, mi)
}

fn everything() -> Window {
    Window {
        from_ms: i64::MIN,
        to_ms: i64::MAX,
    }
}

/// Each block as (start, end, song count).
fn block_spans(cells: &[Cell]) -> Vec<(i64, i64, usize)> {
    cells
        .iter()
        .map(|c| (c.start_ms, c.end_ms, c.songs.len()))
        .collect()
}

fn channel(number: u32, picture: Option<Picture>) -> Channel {
    Channel {
        number,
        name: "Rock".into(),
        plan: Plan::Fixed {
            sources: Vec::new(),
            mode: Mode::Shuffle,
        },
        grid: None,
        breaks: None,
        between: None,
        filler: None,
        sign_off: None,
        picture,
    }
}

#[test]
fn blocks_start_on_the_local_hour() {
    // Kolkata is UTC+5:30, so its hours start on UTC half hours.
    let kolkata: Tz = "Asia/Kolkata".parse().unwrap();
    let at = local(kolkata, 2026, 11, 28, 13, 15);
    assert_eq!(
        block_start(at, kolkata),
        local(kolkata, 2026, 11, 28, 13, 0)
    );
    assert_eq!(block_start(at, Tz::UTC), utc(2026, 11, 28, 7, 0));
}

#[test]
fn blocks_follow_the_clock_across_daylight_saving_changes() {
    let chicago: Tz = "America/Chicago".parse().unwrap();
    // 2026-11-01: 01:00 CDT is 06:00 UTC, and 01:00 CST an hour later.
    // The repeated hour is two blocks.
    let events = music(&["who"], utc(2026, 11, 1, 5, 30), 6, 30 * MINUTE, 0);
    let cells = blocks(&events, &songs(), "Rock", chicago, everything());
    let starts: Vec<i64> = cells.iter().map(|c| c.start_ms).collect();
    assert_eq!(
        starts,
        vec![
            utc(2026, 11, 1, 5, 30),
            utc(2026, 11, 1, 6, 0),
            utc(2026, 11, 1, 7, 0),
            utc(2026, 11, 1, 8, 0),
        ]
    );
    // 2026-03-08: 02:00 CST is skipped; 03:00 CDT is 08:00 UTC.
    assert_eq!(
        block_start(utc(2026, 3, 8, 8, 30), chicago),
        utc(2026, 3, 8, 8, 0)
    );
    assert_eq!(
        block_start(utc(2026, 3, 8, 7, 59), chicago),
        utc(2026, 3, 8, 7, 0)
    );
}

#[test]
fn a_song_belongs_to_the_hour_it_starts_in() {
    let hour = utc(2026, 11, 28, 13, 0);
    let events = vec![
        play("santana", hour, hour + 29 * MINUTE),
        play("who", hour + 29 * MINUTE, hour + 59 * MINUTE),
        // Starts at 13:59 and ends at 14:04: still the 13:00 block.
        play("cream", hour + 59 * MINUTE, hour + 64 * MINUTE),
        play("doors", hour + 64 * MINUTE, hour + 69 * MINUTE),
    ];
    let cells = blocks(&events, &songs(), "Rock", Tz::UTC, everything());
    assert_eq!(
        block_spans(&cells),
        vec![
            (hour, hour + 64 * MINUTE, 3),
            (hour + 64 * MINUTE, hour + 69 * MINUTE, 1)
        ]
    );
}

#[test]
fn bumps_between_songs_are_part_of_the_block_and_blocks_abut() {
    let hour = utc(2026, 11, 28, 13, 0);
    // Four-minute songs with a one-minute bump: twelve songs an hour.
    let events = music(&["santana", "who"], hour, 30, 4 * MINUTE, MINUTE);
    let cells = blocks(&events, &songs(), "Rock", Tz::UTC, everything());
    assert_eq!(
        block_spans(&cells),
        vec![
            (hour, hour + 60 * MINUTE, 12),
            (hour + 60 * MINUTE, hour + 120 * MINUTE, 12),
            // The last block ends with its last song, not the bump after it.
            (hour + 120 * MINUTE, hour + 149 * MINUTE, 6),
        ]
    );
    let row = Row { channel: 33, cells };
    // During a bump the block is still on.
    assert!(row.current(hour + 4 * MINUTE + 30_000).is_some());
}

#[test]
fn dead_air_or_a_sign_off_ends_a_block() {
    let hour = utc(2026, 11, 28, 22, 0);
    let events = vec![
        play("santana", hour, hour + 20 * MINUTE),
        // The sign-off clip, then off the air until 22:30.
        bump(hour + 20 * MINUTE, hour + 21 * MINUTE),
        play("who", hour + 30 * MINUTE, hour + 50 * MINUTE),
    ];
    let cells = blocks(&events, &songs(), "Rock", Tz::UTC, everything());
    assert_eq!(
        block_spans(&cells),
        vec![
            (hour, hour + 20 * MINUTE, 1),
            (hour + 30 * MINUTE, hour + 50 * MINUTE, 1)
        ]
    );
    assert_eq!(cells[0].title.as_deref(), Some("Santana"));
    assert_eq!(cells[1].title.as_deref(), Some("The Who"));
}

#[test]
fn the_title_names_the_three_artists_with_the_most_airtime() {
    let hour = utc(2026, 11, 28, 13, 0);
    let ids = ["who", "cream", "who", "doors", "cream", "cream"];
    let events = music(&ids, hour, 6, 5 * MINUTE, 0);
    let cells = blocks(&events, &songs(), "Rock", Tz::UTC, everything());
    assert_eq!(
        cells[0].title.as_deref(),
        Some("Cream · The Who · The Doors")
    );
    assert_eq!(
        cells[0].description.as_deref(),
        Some(
            "The Who – who\nCream – cream\nThe Who – who\nThe Doors – doors\nCream – cream\nCream – cream"
        )
    );
    assert_eq!(
        cells[0].genres,
        vec!["The Who genre", "Cream genre", "The Doors genre"]
    );
    assert_eq!(cells[0].media_id.as_str(), "who");

    // Ties go to the artist heard first: The Who 10 minutes, then Santana,
    // Cream (both from the duet) and The Doors at 5.
    let events = music(&["who", "duet", "who", "doors"], hour, 4, 5 * MINUTE, 0);
    let cells = blocks(&events, &songs(), "Rock", Tz::UTC, everything());
    assert_eq!(cells[0].title.as_deref(), Some("The Who · Santana · Cream"));

    // No known artists: the channel's name.
    let events = music(&["nobody", "gone"], hour, 2, 5 * MINUTE, 0);
    let cells = blocks(&events, &songs(), "Rock", Tz::UTC, everything());
    assert_eq!(cells[0].title.as_deref(), Some("Rock"));
}

#[test]
fn listings_reads_whole_blocks_across_the_window_edges() {
    let hour = utc(2026, 11, 28, 13, 0);
    let mut store = Store::open_in_memory().unwrap();
    // From 12:58: a song that crosses into 13:00, then 4-minute songs with
    // 1-minute bumps.
    let mut events = vec![play("doors", hour - 2 * MINUTE, hour + 3 * MINUTE)];
    events.extend(music(
        &["santana", "who"],
        hour + 3 * MINUTE,
        40,
        4 * MINUTE,
        MINUTE,
    ));
    store.write_events(33, &events).unwrap();
    let music_channel = channel(33, Some(Picture::Cover));
    // A window from 13:30 to 14:00 sees only the 13:00 block.
    let window = Window {
        from_ms: hour + 30 * MINUTE,
        to_ms: hour + 60 * MINUTE,
    };
    let rows = listings(&store, &songs(), &[&music_channel], Tz::UTC, window).unwrap();
    let cells = &rows[0].cells;
    // The 13:00 block starts with the song at 13:03 and runs to the first
    // song starting after 14:00, at 14:03.
    assert_eq!(
        block_spans(cells),
        vec![(hour + 3 * MINUTE, hour + 63 * MINUTE, 12)]
    );
    // A window from 13:01 also sees the 12:00 block's song running to 13:03.
    let window = Window {
        from_ms: hour + MINUTE,
        to_ms: hour + 2 * MINUTE,
    };
    let rows = listings(&store, &songs(), &[&music_channel], Tz::UTC, window).unwrap();
    assert_eq!(
        block_spans(&rows[0].cells),
        vec![(hour - 2 * MINUTE, hour + 3 * MINUTE, 1)]
    );
    assert_eq!(rows[0].cells[0].title.as_deref(), Some("The Doors"));
}

#[test]
fn listings_leaves_video_channels_as_build_gives_them() {
    let store = stored(&schedule());
    let video = channel(3, None);
    let rows = listings(&store, &media(), &[&video], Tz::UTC, window(0, 180)).unwrap();
    assert_eq!(rows, build(&store, &media(), &[3], window(0, 180)).unwrap());
    assert!(rows[0].cells.iter().all(|cell| cell.songs.is_empty()));
}
