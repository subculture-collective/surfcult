//! `surfcult now`: what each channel is showing, one line per channel.
//!
//! The owner's check against the TV: it reads the store read-only and never
//! asks Jellyfin. For a program the line has its title, its local start and
//! end (the whole program, breaks included) and the live position in the
//! media; for a break, filler or ident clip the clip's file name, the
//! event's times and the position in the clip.

use std::path::Path;

use crate::config::{Config, Tz};
use crate::guide::{self, Window};
use crate::overlay::{clock, program_title};
use crate::schedule::HORIZON_MS;
use crate::store::Store;
use crate::timeline::{EventKind, Lookup, lookup};

/// One line per channel of `config`, in config order, for `now_ms`.
pub fn now_lines(config: &Config, store: &Store, now_ms: i64) -> Vec<String> {
    config
        .channels
        .iter()
        .map(|channel| {
            let what = on_now(channel.number, store, now_ms, config.timezone);
            format!("{:>4}  {}  {what}", channel.number, channel.name)
        })
        .collect()
}

fn on_now(channel: u32, store: &Store, now_ms: i64, tz: Tz) -> String {
    let events = match store.events_between(channel, now_ms, now_ms.saturating_add(1)) {
        Ok(events) => events,
        Err(e) => return format!("cannot read the schedule: {e}"),
    };
    match lookup(&events, now_ms) {
        Lookup::On {
            event, offset_ms, ..
        } if event.kind == EventKind::Program => {
            let (start, end) = guide::build(store, store, &[channel], Window::starting_at(now_ms))
                .ok()
                .and_then(|rows| rows.into_iter().next())
                .and_then(|row| row.current(now_ms).map(|c| (c.start_ms, c.end_ms)))
                .unwrap_or((event.start_ms, event.end_ms));
            format!(
                "{}  {}-{}  at {}",
                program_title(&event.media_id, store),
                clock(start, tz),
                clock(end, tz),
                position(offset_ms)
            )
        }
        Lookup::On {
            event, offset_ms, ..
        } => {
            let kind = match event.kind {
                EventKind::Break => "break",
                EventKind::Filler => "filler",
                EventKind::Ident | EventKind::Program => "ident",
            };
            let file = Path::new(event.media_id.as_str()).file_name().map_or_else(
                || event.media_id.to_string(),
                |name| name.to_string_lossy().into_owned(),
            );
            format!(
                "{kind} {file}  {}-{}  at {}",
                clock(event.start_ms, tz),
                clock(event.end_ms, tz),
                position(offset_ms)
            )
        }
        Lookup::Gap { .. } => {
            match store.events_between(channel, now_ms, now_ms.saturating_add(HORIZON_MS)) {
                Ok(later) => match later.first() {
                    Some(next) => format!("off the air until {}", clock(next.start_ms, tz)),
                    None => "nothing scheduled".into(),
                },
                Err(e) => format!("cannot read the schedule: {e}"),
            }
        }
    }
}

/// A media position as `H:MM:SS`.
fn position(ms: u64) -> String {
    let seconds = ms / 1000;
    format!(
        "{}:{:02}:{:02}",
        seconds / 3600,
        seconds / 60 % 60,
        seconds % 60
    )
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;
    use crate::library::{MediaId, MediaItem, MediaKind};
    use crate::store::LibraryEntry;
    use crate::timeline::Event;

    fn at(hour: u32, minute: u32, second: u32) -> i64 {
        Tz::UTC
            .with_ymd_and_hms(2026, 11, 24, hour, minute, second)
            .unwrap()
            .timestamp_millis()
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

    #[test]
    fn one_line_per_channel_with_what_is_on_and_the_live_position() {
        let config = Config::parse(
            r#"
            timezone = "UTC"
            [[channel]]
            number = 3
            name = "CARTOONS"
            source = { jellyfin_collection = "Cartoons" }
            mode = "sequential"
            [[channel]]
            number = 4
            name = "MOVIES"
            source = { jellyfin_library = "Movies" }
            mode = "sequential"
            [[channel]]
            number = 5
            name = "ADS"
            source = { jellyfin_library = "Movies" }
            mode = "sequential"
            [[channel]]
            number = 6
            name = "EMPTY"
            source = { jellyfin_library = "Movies" }
            mode = "sequential"
            "#,
        )
        .unwrap();
        let mut store = Store::open_in_memory().unwrap();
        let item = MediaItem::new("show", "The Show", MediaKind::Movie, 1_320_000);
        store
            .replace_library(&[LibraryEntry { item, path: None }], &[], 0)
            .unwrap();
        store
            .write_events(
                3,
                &[
                    event(at(19, 0, 0), at(19, 11, 0), "show", 0, EventKind::Program),
                    event(
                        at(19, 11, 0),
                        at(19, 13, 0),
                        "/c/ad.mp4",
                        0,
                        EventKind::Break,
                    ),
                    event(
                        at(19, 13, 0),
                        at(19, 24, 0),
                        "show",
                        660_000,
                        EventKind::Program,
                    ),
                ],
            )
            .unwrap();
        store
            .write_events(
                4,
                &[event(
                    at(23, 0, 0),
                    at(23, 22, 0),
                    "show",
                    0,
                    EventKind::Program,
                )],
            )
            .unwrap();
        store
            .write_events(
                5,
                &[event(
                    at(19, 14, 0),
                    at(19, 15, 0),
                    "/c/ad.mp4",
                    0,
                    EventKind::Break,
                )],
            )
            .unwrap();
        let lines = now_lines(&config, &store, at(19, 14, 30));
        assert_eq!(
            lines,
            [
                "   3  CARTOONS  The Show  19:00-19:24  at 0:12:30",
                "   4  MOVIES  off the air until 23:00",
                "   5  ADS  break ad.mp4  19:14-19:15  at 0:00:30",
                "   6  EMPTY  nothing scheduled",
            ]
        );
    }
}
