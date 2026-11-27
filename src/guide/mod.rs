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

#[cfg(test)]
mod tests;

use crate::library::{MediaId, MediaSource};
use crate::store::{Store, StoreError};
use crate::timeline::{Event, EventKind};

/// How far ahead of now the guide shows by default: three hours.
pub const DEFAULT_SPAN_MS: i64 = 3 * 60 * 60 * 1000;

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
    /// Always `None`: the media model has no description yet.
    pub description: Option<String>,
    /// The item's genres; empty when the item cannot be resolved.
    pub genres: Vec<String>,
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
