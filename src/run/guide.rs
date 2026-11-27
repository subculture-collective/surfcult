//! The program guide screen: which channel has the focus, which rows are on
//! screen, and what the guide shows at a moment ([`Guide::view`]).
//!
//! The guide covers the current half hour and the two after it. The focus is
//! on a channel; its program on now (or its first program in the window) is
//! the focused cell, and the info panel describes it. The rows on screen
//! follow the focus.
//!
//! After [`IDLE_MS`] without a key, the rows scroll upward on their own, 4
//! pixel rows every [`SCROLL_EVERY_MS`], and loop. The next key stops the
//! scroll and puts the rows back where the focus is; the key does nothing
//! else. Scrolling moves the player's offset into one tall bitmap of every
//! row ([`Guide::top_row`]), so nothing is drawn again.

use chrono::{Datelike, TimeZone, Timelike};

use crate::config::{Channel, Tz};
use crate::guide::{self, Window};
use crate::library::{MediaKind, MediaSource};
use crate::overlay::{clock, program_title};
use crate::paint::guide::{GuideCell, GuideRow, GuideView, PITCH, ROWS};
use crate::store::Store;

/// The time each of the guide's three slots covers.
pub const SLOT_MS: i64 = 30 * 60 * 1000;
/// The time the whole grid covers.
pub const SPAN_MS: i64 = 3 * SLOT_MS;
/// How far ahead the guide can go: 24 hours, in half hours.
pub const MAX_LATER: i64 = 48;
/// How long the guide waits without a key before it scrolls.
pub const IDLE_MS: i64 = 5_000;
/// How often a scroll step comes.
pub const SCROLL_EVERY_MS: i64 = 100;
/// The pixel rows a scroll step moves: a multiple of 4, so the CRT pass's
/// scanlines stay on the screen's rows.
pub const SCROLL_STEP: u32 = 4;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Guide {
    /// The index of the focused channel in the TV's channel numbers.
    focus: usize,
    /// The index of the first channel on screen.
    first: usize,
    /// The window start the body was last drawn for.
    pub(super) drawn_for: Option<i64>,
    /// The clock text last drawn.
    pub(super) clock_drawn: Option<String>,
    /// The bitmap row the rows overlay starts at on screen now.
    pub(super) shown_row: Option<u32>,
    /// When the last key came, or the guide opened.
    idle_since_ms: i64,
    /// How many half hours past the current one the grid starts.
    later: i64,
}

impl Guide {
    /// A guide opened at `now_ms` with the focus on channel `index` of
    /// `count`.
    pub(super) fn new(index: usize, count: usize, now_ms: i64) -> Self {
        let mut guide = Self {
            focus: index.min(count.saturating_sub(1)),
            first: 0,
            drawn_for: None,
            clock_drawn: None,
            shown_row: None,
            idle_since_ms: now_ms,
            later: 0,
        };
        guide.follow(count);
        guide
    }

    /// Moves the grid half an hour later (`later`) or earlier, between now
    /// and [`MAX_LATER`] half hours ahead. Returns whether it moved.
    pub(super) fn shift(&mut self, later: bool) -> bool {
        let before = self.later;
        self.later = if later {
            (self.later + 1).min(MAX_LATER)
        } else {
            (self.later - 1).max(0)
        };
        self.later != before
    }

    /// Where the grid starts at `now_ms`: the current half hour, moved on
    /// by [`Guide::shift`].
    pub(super) fn from(&self, now_ms: i64) -> i64 {
        window_start(now_ms) + self.later * SLOT_MS
    }

    pub(super) fn focus(&self) -> usize {
        self.focus
    }

    /// Whether the rows are scrolling on their own at `now_ms`. Fewer
    /// channels than fit on screen never scroll.
    pub(super) fn scrolling(&self, now_ms: i64, count: usize) -> bool {
        count > ROWS && now_ms - self.idle_since_ms >= IDLE_MS
    }

    /// A key came at `now_ms`. Returns whether the rows were scrolling, in
    /// which case the key only stops them.
    pub(super) fn touch(&mut self, now_ms: i64, count: usize) -> bool {
        let was = self.scrolling(now_ms, count);
        self.idle_since_ms = now_ms;
        was
    }

    /// The bitmap row the rows overlay should start at on screen at
    /// `now_ms`: the focus's rows, or where the scroll has got to. The
    /// bitmap holds every channel's row and then the first [`ROWS`] again, so
    /// the loop has no seam.
    pub(super) fn top_row(&self, now_ms: i64, count: usize) -> u32 {
        let rest = self.first as u32 * PITCH;
        if !self.scrolling(now_ms, count) {
            return rest;
        }
        let steps = (now_ms - self.idle_since_ms - IDLE_MS) / SCROLL_EVERY_MS;
        let moved = u64::try_from(steps).unwrap_or(0) * u64::from(SCROLL_STEP);
        let loop_len = u64::from(count as u32 * PITCH);
        ((u64::from(rest) + moved) % loop_len) as u32
    }

    /// Moves the focus one row up (`up`) or down, stopping at the ends.
    /// Returns whether it moved.
    pub(super) fn step(&mut self, up: bool, count: usize) -> bool {
        let before = self.focus;
        self.focus = if up {
            self.focus.saturating_sub(1)
        } else {
            (self.focus + 1).min(count.saturating_sub(1))
        };
        self.follow(count);
        self.focus != before
    }

    /// Keeps the focused row on screen, and the screen full when it can be.
    fn follow(&mut self, count: usize) {
        if self.focus < self.first {
            self.first = self.focus;
        } else if self.focus >= self.first + ROWS {
            self.first = self.focus + 1 - ROWS;
        }
        self.first = self.first.min(count.saturating_sub(ROWS));
    }

    /// What the guide shows at `now_ms` for `channels` (in the TV's order).
    pub(super) fn view(
        &self,
        channels: &[&Channel],
        store: &Store,
        now_ms: i64,
        timezone: Tz,
    ) -> GuideView {
        let from = self.from(now_ms);
        // The focused cell is the program on at the grid's start, or now.
        let at = now_ms.max(from);
        let window = Window {
            from_ms: from,
            to_ms: from + SPAN_MS,
        };
        // Every channel, and the first ROWS again after them for the loop.
        let repeat = if channels.len() > ROWS { ROWS } else { 0 };
        let on_screen: Vec<&Channel> = channels
            .iter()
            .chain(channels.iter().take(repeat))
            .copied()
            .collect();
        let numbers: Vec<u32> = channels.iter().map(|c| c.number).collect();
        let built = guide::build(store, store, &numbers, window).unwrap_or_default();
        let focus_number = channels.get(self.focus).map(|c| c.number);
        let mut info_title = String::new();
        let mut rows = Vec::new();
        for channel in &on_screen {
            let cells = built
                .iter()
                .find(|row| row.channel == channel.number)
                .map(|row| row.cells.clone())
                .unwrap_or_default();
            let focused_index = (Some(channel.number) == focus_number).then(|| {
                cells
                    .iter()
                    .position(|c| c.start_ms <= at && at < c.end_ms)
                    .unwrap_or(0)
            });
            let cells: Vec<GuideCell> = cells
                .iter()
                .enumerate()
                .map(|(i, cell)| {
                    let title = program_title(&cell.media_id, store);
                    let focused = focused_index == Some(i);
                    if focused {
                        info_title = title.clone();
                    }
                    GuideCell {
                        title,
                        left: (cell.start_ms - from) as f32 / SPAN_MS as f32,
                        right: (cell.end_ms - from) as f32 / SPAN_MS as f32,
                        continues_before: cell.start_ms < from,
                        continues_after: cell.end_ms > from + SPAN_MS,
                        movie: store
                            .resolve(&cell.media_id)
                            .is_ok_and(|item| item.kind == MediaKind::Movie),
                        focused,
                    }
                })
                .collect();
            rows.push(GuideRow {
                number: channel.number,
                name: channel.name.clone(),
                cells,
            });
        }
        let focused = channels.get(self.focus);
        let local = timezone.timestamp_millis_opt(now_ms).single();
        let day = local.map(|l| weekday(l.weekday())).unwrap_or_default();
        let date = local
            .map(|l| format!("{} {} {}", weekday(l.weekday()), month(l.month()), l.day()))
            .unwrap_or_default();
        let info_title = if info_title.is_empty() {
            "NOTHING SCHEDULED".to_string()
        } else {
            format!("“{info_title}”")
        };
        GuideView {
            info: [
                focused.map(|c| c.name.clone()).unwrap_or_default(),
                info_title,
                day.to_string(),
                focused
                    .map(|c| format!("Channel {}", c.number))
                    .unwrap_or_default(),
            ],
            slots: [0, 1, 2].map(|i| clock(from + i * SLOT_MS, timezone)),
            date,
            rows,
        }
    }
}

/// The start of the half hour `now_ms` is in.
pub(super) fn window_start(now_ms: i64) -> i64 {
    now_ms - now_ms.rem_euclid(SLOT_MS)
}

/// `now_ms` as `HH:MM:SS` in `timezone`, for the guide's clock.
pub(super) fn clock_with_seconds(now_ms: i64, timezone: Tz) -> String {
    match timezone.timestamp_millis_opt(now_ms).single() {
        Some(l) => format!("{:02}:{:02}:{:02}", l.hour(), l.minute(), l.second()),
        None => "--:--:--".into(),
    }
}

fn weekday(day: chrono::Weekday) -> &'static str {
    use chrono::Weekday::*;
    match day {
        Mon => "Monday",
        Tue => "Tuesday",
        Wed => "Wednesday",
        Thu => "Thursday",
        Fri => "Friday",
        Sat => "Saturday",
        Sun => "Sunday",
    }
}

fn month(month: u32) -> &'static str {
    [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ]
    .get(month.saturating_sub(1) as usize)
    .copied()
    .unwrap_or("")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_focus_stops_at_the_ends_and_the_rows_follow_it() {
        let mut g = Guide::new(0, 6, 0);
        assert_eq!((g.focus, g.first), (0, 0));
        assert!(!g.step(true, 6), "already at the top");
        for _ in 0..4 {
            assert!(g.step(false, 6));
        }
        assert_eq!((g.focus, g.first), (4, 1));
        assert!(g.step(false, 6));
        assert!(!g.step(false, 6), "already at the bottom");
        assert_eq!((g.focus, g.first), (5, 2));
        for _ in 0..3 {
            g.step(true, 6);
        }
        assert_eq!((g.focus, g.first), (2, 2));
        g.step(true, 6);
        assert_eq!((g.focus, g.first), (1, 1));
    }

    #[test]
    fn opening_on_a_low_channel_fills_the_screen() {
        // The focus on the last of six channels shows the last four.
        let g = Guide::new(5, 6, 0);
        assert_eq!((g.focus, g.first), (5, 2));
        // Fewer channels than rows start at the top.
        let g = Guide::new(2, 3, 0);
        assert_eq!((g.focus, g.first), (2, 0));
    }

    #[test]
    fn the_window_starts_on_the_half_hour() {
        let half = SLOT_MS;
        assert_eq!(window_start(10 * half + 1), 10 * half);
        assert_eq!(window_start(10 * half), 10 * half);
        assert_eq!(clock_with_seconds(3_723_000, Tz::UTC), "01:02:03");
    }

    #[test]
    fn the_rows_scroll_after_five_seconds_without_a_key_and_loop() {
        let count = 6;
        let g = Guide::new(0, count, 1_000);
        assert_eq!(g.top_row(5_999, count), 0);
        assert!(!g.scrolling(5_999, count));
        assert!(g.scrolling(6_000, count));
        assert_eq!(g.top_row(6_099, count), 0);
        assert_eq!(g.top_row(6_100, count), SCROLL_STEP);
        assert_eq!(g.top_row(7_000, count), 10 * SCROLL_STEP);
        // Six rows of 92 pixels loop after 552 pixel rows: 138 steps.
        let loop_len = count as u32 * PITCH;
        assert_eq!(loop_len / SCROLL_STEP, 138);
        assert_eq!(g.top_row(6_000 + 138 * SCROLL_EVERY_MS, count), 0);
        assert_eq!(g.top_row(6_000 + 139 * SCROLL_EVERY_MS, count), SCROLL_STEP);
    }

    #[test]
    fn a_key_stops_the_scroll_and_the_wait_starts_again() {
        let count = 6;
        let mut g = Guide::new(0, count, 0);
        assert!(!g.touch(4_000, count), "not scrolling yet");
        assert!(!g.scrolling(8_999, count));
        assert!(g.touch(9_500, count), "the key stops the scroll");
        assert_eq!(g.top_row(9_500, count), 0, "back where the focus is");
        assert_eq!(g.top_row(14_499, count), 0);
        assert!(g.scrolling(14_500, count));
    }

    #[test]
    fn a_scroll_starts_from_the_rows_the_focus_put_on_screen() {
        let count = 6;
        let mut g = Guide::new(0, count, 0);
        for _ in 0..5 {
            g.step(false, count);
        }
        assert_eq!(g.top_row(0, count), 2 * PITCH);
        assert_eq!(g.top_row(5_100, count), 2 * PITCH + SCROLL_STEP);
    }

    #[test]
    fn four_channels_or_fewer_never_scroll() {
        let g = Guide::new(0, ROWS, 0);
        assert!(!g.scrolling(60_000, ROWS));
        assert_eq!(g.top_row(60_000, ROWS), 0);
    }

    #[test]
    fn left_and_right_move_the_grid_by_half_hours_between_now_and_a_day_ahead() {
        let mut g = Guide::new(0, 6, 0);
        let now = 10 * SLOT_MS + 5;
        assert_eq!(g.from(now), 10 * SLOT_MS);
        assert!(!g.shift(false), "not before now");
        assert!(g.shift(true));
        assert_eq!(g.from(now), 11 * SLOT_MS);
        for _ in 0..60 {
            g.shift(true);
        }
        assert_eq!(g.from(now), (10 + MAX_LATER) * SLOT_MS);
        assert!(!g.shift(true), "not past a day ahead");
        assert!(g.shift(false));
        assert_eq!(g.from(now), (10 + MAX_LATER - 1) * SLOT_MS);
    }
}
