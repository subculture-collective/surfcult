//! The program guide's data: for each channel, the programs in a time window,
//! one cell per program.
//!
//! A program cut by breaks is several program events with the same media id
//! and contiguous media offsets, with break, filler or ident events between
//! them. [`cells`] merges those segments into one [`Cell`] that runs from the
//! first segment's start to the last segment's end. Break, filler and ident
//! events never become cells.
//!
//! Nothing here reads the clock: callers pass the window, and the time
//! [`Cell::progress`] should answer for. [`cells`] is pure apart from the
//! title lookups through a [`MediaSource`]; [`build`] reads each channel's
//! events from the [`Store`] and calls it.
//!
//! Times are Unix epoch milliseconds. Cells are half-open intervals
//! `[start_ms, end_ms)`, like events.
//!
//! A music channel schedules one program event per song, too short to read
//! in a guide cell. [`listings`], which the TV guide, the web admin and
//! XMLTV read, groups a music channel's songs into blocks of
//! [`MUSIC_BLOCK_MS`] aligned to the clock in the config time zone ([`blocks`]);
//! video channels get the same cells as [`build`]. The schedule itself, the
//! banner and `surfcult now` keep one event and one cell per song.

#[cfg(test)]
mod tests;

use chrono::{TimeZone, Timelike};

use crate::config::{Channel, Tz};
use crate::library::{MediaId, MediaSource, Plays};
use crate::overlay::program_title;
use crate::store::{Store, StoreError};
use crate::timeline::{Event, EventKind};

/// How far ahead of now the guide shows by default: three hours.
pub const DEFAULT_SPAN_MS: i64 = 3 * 60 * 60 * 1000;

/// How long a music channel's guide block is: an hour, aligned to the
/// local clock. It must divide a day.
pub const MUSIC_BLOCK_MS: i64 = 60 * 60 * 1000;

/// The most artists a block's title names.
pub const BLOCK_ARTISTS: usize = 3;

/// The stretch of time the guide shows, `[from_ms, to_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub from_ms: i64,
    pub to_ms: i64,
}

impl Window {
    /// The default window: `now_ms` through [`DEFAULT_SPAN_MS`] later.
    pub fn starting_at(now_ms: i64) -> Self {
        Self {
            from_ms: now_ms,
            to_ms: now_ms.saturating_add(DEFAULT_SPAN_MS),
        }
    }
}

/// One program in the guide.
///
/// `start_ms` and `end_ms` are the program's real start and end, including
/// any breaks inside it. They are not clipped to the window, so a cell can
/// start before the window or end after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Cell {
    pub media_id: MediaId,
    pub start_ms: i64,
    pub end_ms: i64,
    /// The item's title, or `None` when the media source cannot resolve the
    /// item (it left the library, or the source is unavailable).
    pub title: Option<String>,
    /// `None` for a program: the media model has no description yet. A
    /// music block lists its songs, one display title a line.
    pub description: Option<String>,
    /// The item's genres; empty when the item cannot be resolved. A music
    /// block has its songs' genres, each once.
    pub genres: Vec<String>,
    /// A music block's songs, in time order; empty for a program. A block's
    /// `media_id` is its first song's.
    pub songs: Vec<Cell>,
}

impl Cell {
    /// How far through the cell `at_ms` is, from 0.0 at its start towards 1.0
    /// at its end, counting breaks inside the program as program time. `None`
    /// when the cell is not on at `at_ms`.
    pub fn progress(&self, at_ms: i64) -> Option<f64> {
        if at_ms < self.start_ms || at_ms >= self.end_ms {
            return None;
        }
        Some((at_ms - self.start_ms) as f64 / (self.end_ms - self.start_ms) as f64)
    }
}

/// One channel's line of the guide.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub channel: u32,
    /// The programs that overlap the window, in time order.
    pub cells: Vec<Cell>,
}

impl Row {
    /// The cell on at `at_ms`, if any. A break between two programs belongs
    /// to no cell.
    pub fn current(&self, at_ms: i64) -> Option<&Cell> {
        self.cells
            .iter()
            .find(|cell| cell.progress(at_ms).is_some())
    }
}

/// The guide cells for one channel's `events` that overlap `window`.
///
/// `events` must be sorted by start time and must not overlap. Two program
/// events join into one cell when they have the same media id, the second
/// starts at the media offset where the first ended, and only break, filler
/// or ident events fill the time between them, with no hole. A program that
/// crosses a window edge needs all its segments in `events`, or its cell
/// starts or ends at the first or last segment given; [`build`] reads them.
pub fn cells(events: &[Event], media: &impl MediaSource, window: Window) -> Vec<Cell> {
    merge(events)
        .into_iter()
        .filter(|span| span.end_ms > window.from_ms && span.start_ms < window.to_ms)
        .map(|span| {
            let item = media.resolve(&span.media_id).ok();
            Cell {
                title: item.as_ref().map(|item| item.title.clone()),
                description: None,
                genres: item.map(|item| item.genres).unwrap_or_default(),
                media_id: span.media_id,
                start_ms: span.start_ms,
                end_ms: span.end_ms,
                songs: Vec::new(),
            }
        })
        .collect()
}

/// One [`Row`] per channel in `channels`, in that order, for `window`.
///
/// Reads each channel's events in the window from `store`. When a program
/// crosses a window edge it also reads the neighbouring events, one at a
/// time, until it reaches that program's first and last segment, so the
/// cell has the program's full extent. Titles come from `media`; in
/// production that is the same store.
pub fn build(
    store: &Store,
    media: &impl MediaSource,
    channels: &[u32],
    window: Window,
) -> Result<Vec<Row>, StoreError> {
    channels
        .iter()
        .map(|&channel| {
            let events = read_events(store, channel, window)?;
            Ok(Row {
                channel,
                cells: cells(&events, media, window),
            })
        })
        .collect()
}

/// One [`Row`] per channel, in the order given, for `window`, as the guide
/// screens show them: a video channel's cells as [`build`] gives them, and a
/// music channel's songs grouped into blocks ([`blocks`]) aligned to the
/// clock in `timezone`.
pub fn listings(
    store: &Store,
    media: &impl MediaSource,
    channels: &[&Channel],
    timezone: Tz,
    window: Window,
) -> Result<Vec<Row>, StoreError> {
    channels
        .iter()
        .map(|channel| {
            let events = match channel.plays() {
                Plays::Videos => {
                    return Ok(build(store, media, &[channel.number], window)?.remove(0));
                }
                Plays::Songs => read_block_events(store, channel.number, timezone, window)?,
            };
            Ok(Row {
                channel: channel.number,
                cells: blocks(&events, media, &channel.name, timezone, window),
            })
        })
        .collect()
}

/// The start of the [`MUSIC_BLOCK_MS`] block `at_ms` is in, on the local
/// clock in `timezone`. Each local hour is its own block, so the hour a
/// clock change repeats is two blocks and the hour it skips is none.
pub fn block_start(at_ms: i64, timezone: Tz) -> i64 {
    let Some(local) = timezone.timestamp_millis_opt(at_ms).single() else {
        return at_ms - at_ms.rem_euclid(MUSIC_BLOCK_MS);
    };
    let into_day = i64::from(local.num_seconds_from_midnight()) * 1000
        + i64::from(local.nanosecond() / 1_000_000);
    at_ms - into_day.rem_euclid(MUSIC_BLOCK_MS)
}

/// A music channel's guide cells: its songs grouped into blocks that
/// overlap `window`.
///
/// A song belongs to the block it starts in ([`block_start`]). A block runs
/// from its first song's start to its last song's end. Breaks, filler and
/// idents between songs are part of the block, and the ones between two
/// blocks belong to the earlier one, so back-to-back blocks abut. A hole in
/// the events (dead air, or the time off the air after a sign-off) ends the
/// block at the last song before it; songs after the hole in the same hour
/// start a new block.
///
/// A block's title names the [`BLOCK_ARTISTS`] artists with the most airtime
/// in it, ties going to the one heard first, as `Santana · The Who · Cream`.
/// The row already names the channel, so the title leaves it out unless no
/// song's artists are known; then it is `channel_name`. Like [`cells`],
/// `events` must be sorted and must hold every song of a block that crosses
/// a window edge.
pub fn blocks(
    events: &[Event],
    media: &impl MediaSource,
    channel_name: &str,
    timezone: Tz,
    window: Window,
) -> Vec<Cell> {
    // Each block's song events and its end.
    let mut groups: Vec<(Vec<&Event>, i64)> = Vec::new();
    // The block the last song is in, while no hole has come since it.
    let mut open: Option<i64> = None;
    let mut last_end: Option<i64> = None;
    for event in events {
        if last_end != Some(event.start_ms) {
            open = None;
        }
        last_end = Some(event.end_ms);
        if event.kind != EventKind::Program {
            continue;
        }
        let block = block_start(event.start_ms, timezone);
        match groups.last_mut() {
            Some((songs, end)) if open == Some(block) => {
                songs.push(event);
                *end = event.end_ms;
            }
            last => {
                // A block followed by the next with no hole runs up to it.
                if open.is_some()
                    && let Some((_, end)) = last
                {
                    *end = event.start_ms;
                }
                groups.push((vec![event], event.end_ms));
            }
        }
        open = Some(block);
    }
    groups
        .into_iter()
        .filter(|(songs, end)| *end > window.from_ms && songs[0].start_ms < window.to_ms)
        .map(|(group, end)| block(&group, end, media, channel_name))
        .collect()
}

/// One block's cell from its song events.
fn block(events: &[&Event], end_ms: i64, media: &impl MediaSource, channel_name: &str) -> Cell {
    let owned: Vec<Event> = events.iter().map(|&e| e.clone()).collect();
    let everything = Window {
        from_ms: i64::MIN,
        to_ms: i64::MAX,
    };
    let songs = cells(&owned, media, everything);
    // Each artist's airtime, in the order they are first heard.
    let mut airtime: Vec<(String, i64)> = Vec::new();
    let mut genres: Vec<String> = Vec::new();
    for song in &songs {
        let Ok(item) = media.resolve(&song.media_id) else {
            continue;
        };
        for artist in &item.artists {
            let length = song.end_ms - song.start_ms;
            match airtime.iter_mut().find(|(name, _)| name == artist) {
                Some((_, time)) => *time += length,
                None => airtime.push((artist.clone(), length)),
            }
        }
        for genre in &song.genres {
            if !genres.contains(genre) {
                genres.push(genre.clone());
            }
        }
    }
    // A stable sort keeps first-heard order among equal airtimes.
    airtime.sort_by_key(|(_, time)| std::cmp::Reverse(*time));
    let title = if airtime.is_empty() {
        channel_name.to_string()
    } else {
        airtime
            .iter()
            .take(BLOCK_ARTISTS)
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>()
            .join(" · ")
    };
    let description = songs
        .iter()
        .map(|song| program_title(&song.media_id, media))
        .collect::<Vec<_>>()
        .join("\n");
    Cell {
        media_id: events[0].media_id.clone(),
        start_ms: events[0].start_ms,
        end_ms,
        title: Some(title),
        description: Some(description),
        genres,
        songs,
    }
}

/// A music channel's events for `window`: every event from the start of the
/// block the window starts in (or an earlier one, when a song crosses into
/// it) to the end of the block the window ends in, then on through breaks,
/// filler and idents to the next song, so the last block ends where the
/// next one starts whatever the window.
fn read_block_events(
    store: &Store,
    channel: u32,
    timezone: Tz,
    window: Window,
) -> Result<Vec<Event>, StoreError> {
    let mut from = block_start(window.from_ms, timezone);
    let to = block_start(window.to_ms.saturating_sub(1), timezone).saturating_add(MUSIC_BLOCK_MS);
    let mut events = store.events_between(channel, from, to)?;
    while let Some(first) = events.first()
        && first.start_ms < from
    {
        from = block_start(first.start_ms, timezone);
        events = store.events_between(channel, from, to)?;
    }
    // Read on until the first song of the next block, or a hole.
    while let Some(last) = events.last()
        && !(last.kind == EventKind::Program && last.start_ms >= to)
    {
        let end = last.end_ms;
        match store
            .events_between(channel, end, end.saturating_add(1))?
            .pop()
        {
            Some(event) if event.start_ms == end => events.push(event),
            _ => break,
        }
    }
    Ok(events)
}

/// The window's events plus the neighbouring events of any program that
/// crosses a window edge.
fn read_events(store: &Store, channel: u32, window: Window) -> Result<Vec<Event>, StoreError> {
    let inside = store.events_between(channel, window.from_ms, window.to_ms)?;

    let mut earlier: Vec<Event> = Vec::new();
    while let Some(first) = earlier.last().or(inside.first()) {
        if !may_start_earlier(first) {
            break;
        }
        let start = first.start_ms;
        // The event on just before `start`, if it ends exactly there.
        match store
            .events_between(channel, start.saturating_sub(1), start)?
            .pop()
        {
            Some(event) if event.end_ms == start => earlier.push(event),
            _ => break,
        }
    }

    let mut later: Vec<Event> = Vec::new();
    while let Some(last) = later.last().or(inside.last()) {
        if !may_end_later(last) {
            break;
        }
        let end = last.end_ms;
        // The event that starts exactly at `end`, if there is one.
        match store
            .events_between(channel, end, end.saturating_add(1))?
            .pop()
        {
            Some(event) if event.start_ms == end => later.push(event),
            _ => break,
        }
    }

    earlier.reverse();
    earlier.extend(inside);
    earlier.extend(later);
    Ok(earlier)
}

/// Whether an earlier segment of the same program may lead into `event`: it
/// is a break, filler or ident, or a program segment that does not start at
/// the beginning of its media.
fn may_start_earlier(event: &Event) -> bool {
    event.kind != EventKind::Program || event.media_offset_ms > 0
}

/// Whether a later segment may continue the program at `event`: it is a
/// break, filler or ident, or a program segment that stops before the end of
/// its media.
fn may_end_later(event: &Event) -> bool {
    event.kind != EventKind::Program || next_offset(event) < event.media_duration_ms
}

/// The media offset just after `event` ends.
fn next_offset(event: &Event) -> u64 {
    let length = u64::try_from(event.end_ms.saturating_sub(event.start_ms)).unwrap_or(0);
    event.media_offset_ms.saturating_add(length)
}

/// A merged program before its title is looked up.
struct Span {
    media_id: MediaId,
    start_ms: i64,
    end_ms: i64,
    /// The media offset where the next segment must start to join this span.
    next_offset_ms: u64,
}

/// Merges program segments into spans and drops every other event.
fn merge(events: &[Event]) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    // Whether the last span may still take a segment: no hole since it.
    let mut open = false;
    let mut last_end: Option<i64> = None;
    for event in events {
        if last_end.is_some_and(|end| end != event.start_ms) {
            open = false;
        }
        last_end = Some(event.end_ms);
        if event.kind != EventKind::Program {
            continue;
        }
        if open
            && let Some(span) = spans.last_mut()
            && span.media_id == event.media_id
            && span.next_offset_ms == event.media_offset_ms
        {
            span.end_ms = event.end_ms;
            span.next_offset_ms = next_offset(event);
            continue;
        }
        spans.push(Span {
            media_id: event.media_id.clone(),
            start_ms: event.start_ms,
            end_ms: event.end_ms,
            next_offset_ms: next_offset(event),
        });
        open = true;
    }
    spans
}
