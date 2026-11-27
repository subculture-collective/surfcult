//! The channel banner: what it says when the viewer changes channel or
//! presses OK.
//!
//! [`banner`] reads the channel's schedule and the library from the
//! [`Store`] and returns a [`Banner`]: the channel's number and name, the
//! program on now with its local start and end time, or `OFF THE AIR` and
//! `NO SCHEDULE` when nothing is. How it is drawn is the player's business
//! ([`Player::show_banner`](crate::player::Player::show_banner));
//! [`MpvPlayer`](crate::player::MpvPlayer) draws it in the Cable Grid design
//! with [`paint::banner`](crate::paint::banner).

use chrono::{TimeZone, Timelike};

use crate::config::{Channel, Tz};
use crate::guide::{self, Cell, Window};
use crate::library::{MediaId, MediaSource};
use crate::schedule::HORIZON_MS;
use crate::store::Store;
use crate::timeline::{Lookup, lookup};

/// How long the banner stays up unless the config or the environment says
/// otherwise.
pub const DEFAULT_BANNER_MS: i64 = 5_000;

/// What the banner says. The player draws it; every field is plain text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Banner {
    pub number: u32,
    pub name: String,
    /// The program on now (`Series: Episode` for an episode), `NEXT: ...`
    /// during a break between programs, `OFF THE AIR` or `NO SCHEDULE`.
    pub title: String,
    /// Local start and end of that program, such as `19:00–19:30`, the time
    /// the channel comes back on (`BACK AT 06:00`), or empty.
    pub times: String,
    /// The local time of day when the banner was made, `HH:MM`.
    pub clock: String,
}

/// The banner for `channel` at `now_ms`, with times in `timezone`.
///
/// The program on now is the guide cell on now, so a program cut by breaks
/// shows its whole extent, also during a break inside it. During a break,
/// filler or ident between two programs the banner names the next program.
/// When nothing is on and something is scheduled later, it says
/// `OFF THE AIR` and when the channel comes back; when the stored schedule
/// has nothing more for the next seven days, `NO SCHEDULE`. A schedule that
/// cannot be read counts as none.
pub fn banner(channel: &Channel, store: &Store, now_ms: i64, timezone: Tz) -> Banner {
    let (title, times) = what_is_on(channel.number, store, now_ms, timezone);
    Banner {
        number: channel.number,
        name: channel.name.clone(),
        title,
        times,
        clock: clock(now_ms, timezone),
    }
}

fn what_is_on(channel: u32, store: &Store, now_ms: i64, timezone: Tz) -> (String, String) {
    let cells = guide::build(store, store, &[channel], Window::starting_at(now_ms))
        .ok()
        .and_then(|rows| rows.into_iter().next())
        .map(|row| row.cells)
        .unwrap_or_default();
    if let Some(cell) = cells
        .iter()
        .find(|c| c.start_ms <= now_ms && now_ms < c.end_ms)
    {
        return (title(cell, store), span(cell, timezone));
    }
    let clip_on = store
        .events_between(channel, now_ms, now_ms.saturating_add(1))
        .is_ok_and(|events| matches!(lookup(&events, now_ms), Lookup::On { .. }));
    if clip_on && let Some(next) = cells.iter().find(|c| c.start_ms > now_ms) {
        return (
            format!("NEXT: {}", title(next, store)),
            span(next, timezone),
        );
    }
    let next_start = store
        .events_between(channel, now_ms, now_ms.saturating_add(HORIZON_MS))
        .ok()
        .and_then(|events| events.into_iter().find(|e| e.start_ms > now_ms))
        .map(|event| event.start_ms);
    match next_start {
        Some(at) => (
            "OFF THE AIR".into(),
            format!("BACK AT {}", clock(at, timezone)),
        ),
        None => ("NO SCHEDULE".into(), String::new()),
    }
}

fn title(cell: &Cell, media: &impl MediaSource) -> String {
    program_title(&cell.media_id, media)
}

/// The item's title, with its series' name first for an episode. A program
/// the library does not have is `UNKNOWN PROGRAM`.
pub fn program_title(media_id: &MediaId, media: &impl MediaSource) -> String {
    match media.resolve(media_id) {
        Ok(item) => match item.series {
            Some(series) if !series.series_name.is_empty() => {
                format!("{}: {}", series.series_name, item.title)
            }
            _ => item.title,
        },
        Err(_) => "UNKNOWN PROGRAM".into(),
    }
}

fn span(cell: &Cell, timezone: Tz) -> String {
    format!(
        "{}–{}",
        clock(cell.start_ms, timezone),
        clock(cell.end_ms, timezone)
    )
}

/// `at_ms` as the local time of day, `HH:MM` on a 24-hour clock.
pub fn clock(at_ms: i64, timezone: Tz) -> String {
    match timezone.timestamp_millis_opt(at_ms).single() {
        Some(local) => format!("{:02}:{:02}", local.hour(), local.minute()),
        None => "--:--".into(),
    }
}

#[cfg(test)]
mod tests;
