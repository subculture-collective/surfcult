use chrono::TimeZone;

use super::*;
use crate::config::Config;
use crate::library::{MediaId, MediaItem, MediaKind, SeriesEpisode};
use crate::store::LibraryEntry;
use crate::timeline::{Event, EventKind};

const MIN: i64 = 60_000;

fn at(hour: u32, minute: u32) -> i64 {
    Tz::UTC
        .with_ymd_and_hms(2026, 11, 24, hour, minute, 0)
        .unwrap()
        .timestamp_millis()
}

fn channel() -> Channel {
    Config::parse(
        r#"
        timezone = "UTC"
        [[channel]]
        number = 3
        name = "CARTOONS"
        source = { jellyfin_collection = "Cartoons" }
        mode = "sequential"
        "#,
    )
    .unwrap()
    .channels
    .remove(0)
}

fn event(start_ms: i64, end_ms: i64, media: &str, offset_ms: u64, kind: EventKind) -> Event {
    Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media),
        media_offset_ms: offset_ms,
        media_duration_ms: 1_320_000,
        kind,
    }
}

fn stored(events: &[Event]) -> Store {
    let mut store = Store::open_in_memory().unwrap();
    let mut episode = MediaItem::new("ep", "Bart the Genius", MediaKind::Episode, 1_320_000);
    episode.series = Some(SeriesEpisode {
        series_id: MediaId::new("simpsons"),
        series_name: "The Simpsons".into(),
        season: Some(1),
        number: Some(2),
    });
    let movie = MediaItem::new("film", "Night {Film} \\N", MediaKind::Movie, 5_400_000);
    let entries: Vec<LibraryEntry> = [episode, movie]
        .into_iter()
        .map(|item| LibraryEntry { item, path: None })
        .collect();
    store.replace_library(&entries, &[], 0).unwrap();
    if !events.is_empty() {
        store.write_events(3, events).unwrap();
    }
    store
}

/// `ep` 19:00-19:30 with a break at 19:11, then a gap, then `film` at 21:00.
fn schedule() -> Vec<Event> {
    vec![
        event(at(19, 0), at(19, 11), "ep", 0, EventKind::Program),
        event(at(19, 11), at(19, 13), "/clips/ad.mp4", 0, EventKind::Break),
        event(
            at(19, 13),
            at(19, 30),
            "ep",
            11 * MIN as u64,
            EventKind::Program,
        ),
        event(
            at(19, 30),
            at(19, 32),
            "/clips/filler.mp4",
            0,
            EventKind::Filler,
        ),
        event(at(19, 32), at(21, 0), "film", 0, EventKind::Program),
    ]
}

#[test]
fn names_the_channel_and_the_program_on_now_with_its_whole_extent() {
    let store = stored(&schedule());
    let shown = banner(&channel(), &store, at(19, 5), Tz::UTC);
    assert_eq!(
        shown,
        Banner {
            number: 3,
            name: "CARTOONS".into(),
            title: "The Simpsons: Bart the Genius".into(),
            times: "19:00–19:30".into(),
            clock: "19:05".into(),
            next: r"NEXT 19:32 Night {Film} \N".into(),
        }
    );
    // A break inside the program is still the program.
    assert_eq!(
        banner(&channel(), &store, at(19, 12), Tz::UTC).times,
        "19:00–19:30"
    );
}

#[test]
fn a_clip_between_programs_names_the_next_one() {
    let store = stored(&schedule());
    let shown = banner(&channel(), &store, at(19, 31), Tz::UTC);
    assert_eq!(shown.title, "NEXT: Night {Film} \\N");
    assert_eq!(shown.times, "19:32–21:00");
}

#[test]
fn a_gap_is_off_the_air_until_the_next_event_and_nothing_is_no_schedule() {
    let mut events = schedule();
    events.push(event(at(23, 0), at(23, 30), "ep", 0, EventKind::Program));
    let store = stored(&events);
    let shown = banner(&channel(), &store, at(22, 0), Tz::UTC);
    assert_eq!(shown.title, "OFF THE AIR");
    assert_eq!(shown.times, "BACK AT 23:00");

    let empty = stored(&[]);
    let shown = banner(&channel(), &empty, at(22, 0), Tz::UTC);
    assert_eq!(shown.title, "NO SCHEDULE");
    assert_eq!(shown.times, "");
}

#[test]
fn times_are_local_to_the_config_time_zone() {
    let chicago: Tz = "America/Chicago".parse().unwrap();
    // 19:00 UTC is 13:00 in Chicago in November.
    assert_eq!(clock(at(19, 0), chicago), "13:00");
    assert_eq!(clock(at(9, 5), Tz::UTC), "09:05");
}

#[test]
fn next_names_the_program_after_the_one_on_and_is_empty_otherwise() {
    let store = stored(&schedule());
    // During the filler between the episode and the film, the title is
    // already the next program, so there is no NEXT line.
    let between = banner(&channel(), &store, at(19, 31), Tz::UTC);
    assert!(between.title.starts_with("NEXT: "), "{}", between.title);
    assert_eq!(between.next, "");
    // The film is the last program: nothing follows it.
    assert_eq!(banner(&channel(), &store, at(20, 0), Tz::UTC).next, "");
    // Off the air, nothing is on to follow.
    assert_eq!(banner(&channel(), &store, at(22, 0), Tz::UTC).next, "");
}

#[test]
fn a_music_video_is_titled_artist_then_title() {
    let mut store = Store::open_in_memory().unwrap();
    let mut video = MediaItem::new("mv", "Song", MediaKind::MusicVideo, 240_000);
    video.artists = vec!["Artist".into(), "Guest".into()];
    let mut bare = MediaItem::new("bare", "Instrumental", MediaKind::MusicVideo, 180_000);
    bare.album = Some("Album".into());
    let entries: Vec<LibraryEntry> = [video, bare]
        .into_iter()
        .map(|item| LibraryEntry { item, path: None })
        .collect();
    store.replace_library(&entries, &[], 0).unwrap();

    assert_eq!(
        program_title(&MediaId::new("mv"), &store),
        "Artist, Guest – Song"
    );
    // Without artists the title stands alone; the album is not shown.
    assert_eq!(program_title(&MediaId::new("bare"), &store), "Instrumental");
}
