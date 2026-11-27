//! The program guide screen: which channel has the focus, which rows are on
//! screen, and what the guide shows at a moment ([`Guide::view`]).
//!
//! The guide covers the current half hour and the two after it. The focus is
//! on a channel; its program on now (or its first program in the window) is
//! the focused cell, and the info panel describes it. The rows on screen
//! follow the focus.

use chrono::{Datelike, TimeZone, Timelike};

use crate::config::{Channel, Tz};
use crate::guide::{self, Window};
use crate::library::{MediaKind, MediaSource};
use crate::overlay::{clock, program_title};
use crate::paint::guide::{GuideCell, GuideRow, GuideView, ROWS};
use crate::store::Store;

/// The time each of the guide's three slots covers.
pub const SLOT_MS: i64 = 30 * 60 * 1000;
/// The time the whole grid covers.
pub const SPAN_MS: i64 = 3 * SLOT_MS;

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
}

impl Guide {
    /// A guide with the focus on channel `index` of `count`.
    pub(super) fn new(index: usize, count: usize) -> Self {
        let mut guide = Self {
            focus: index.min(count.saturating_sub(1)),
            first: 0,
            drawn_for: None,
            clock_drawn: None,
        };
        guide.follow(count);
        guide
    }

    pub(super) fn focus(&self) -> usize {
        self.focus
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
        let from = window_start(now_ms);
        let window = Window {
            from_ms: from,
            to_ms: from + SPAN_MS,
        };
        let on_screen: Vec<&Channel> = channels
            .iter()
            .skip(self.first)
            .take(ROWS)
            .copied()
            .collect();
        let numbers: Vec<u32> = on_screen.iter().map(|c| c.number).collect();
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
                    .position(|c| c.start_ms <= now_ms && now_ms < c.end_ms)
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
        let mut g = Guide::new(0, 6);
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
        let g = Guide::new(5, 6);
        assert_eq!((g.focus, g.first), (5, 2));
        // Fewer channels than rows start at the top.
        let g = Guide::new(2, 3);
        assert_eq!((g.focus, g.first), (2, 0));
    }

    #[test]
    fn the_window_starts_on_the_half_hour() {
        let half = SLOT_MS;
        assert_eq!(window_start(10 * half + 1), 10 * half);
        assert_eq!(window_start(10 * half), 10 * half);
        assert_eq!(clock_with_seconds(3_723_000, Tz::UTC), "01:02:03");
    }
}
