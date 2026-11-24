//! Generates a channel's schedule: which media plays when.
//!
//! [`extend`] keeps a channel's week filled in the
//! [`Store`](crate::store::Store). Inside it, `generate` is a pure function. It
//! takes a channel's number, mode and time zone, the items to play, where the
//! last run stopped and a time window, and it returns
//! [`Event`](crate::timeline::Event)s and the new cursor. It reads no clock and
//! does no I/O, so the same inputs always give the same events.
//!
//! Times are Unix epoch milliseconds (`i64`), as in [`crate::timeline`].
//!
//! # What a run produces
//!
//! Programs run back to back. Each program is one `EventKind::Program` event
//! that starts where the previous one ended, with `media_offset_ms` 0 and
//! `media_duration_ms` equal to the item's runtime. Events cover the whole
//! window with no gap and no overlap; the last one may run past the end. Items
//! with a runtime of 0 are skipped, and an item id that appears twice in the
//! pool is used once (the first). Breaks, grids, weighted picks, time blocks and
//! regeneration are later work; a channel's `grid`, `breaks` and `filler`
//! settings are ignored for now.
//!
//! # Sequential
//!
//! Items play in the pool's order (the order of `MediaSource::list_items`). A
//! run continues after the cursor item, which is the media id of the last item
//! the previous run scheduled, and wraps from the last item to the first. With
//! no cursor it starts at the first item. If the cursor item is no longer in
//! the pool it starts at the first item and says so with
//! [`Note::CursorNotInPool`]; that restarts the order, which is better than
//! guessing a position.
//!
//! # Shuffle
//!
//! Items play in a seeded random order, and no item repeats until every other
//! item has played. One trip through the pool is a *pass*.
//!
//! - **Seed.** A pass is ordered by a Fisher-Yates shuffle driven by SplitMix64
//!   (`rng.rs`), seeded from the channel number, the *local* date in the
//!   channel's time zone at the moment the pass is drawn, and a counter of the
//!   passes already drawn in this run. The counter makes two passes on the same
//!   date differ. The local date is a calendar date, not "24 hours since the
//!   start", so a day with 23 or 25 hours changes nothing. The generator uses no
//!   `rand` crate and no hasher with random state, so the same channel, date and
//!   library give the same order on every machine.
//! - **Continuing.** The cursor is only one media id, so it cannot say which
//!   items a pass has used. The caller therefore also passes the ids of the
//!   programs scheduled earlier, oldest first (`Resume::played`). [`extend`]
//!   reads both from the channel's stored program events, at any time, past or
//!   future: the cursor is the last of them. The generator replays
//!   them to find the items already used in the current pass, then plays the
//!   rest of the pass in the seeded order, then starts a new pass. A restart
//!   therefore never replays an item inside the same pass. If `played` is empty
//!   but there is a cursor, only the cursor item counts as used.
//! - **First item of a pass.** It is never the item that just played, unless the
//!   pool has one item.
//! - **Limits.** A run that stops and resumes on the same local date plays what
//!   an unbroken run would have played, for the pass it resumes in. After that,
//!   or on a later date, the order can differ from an unbroken run, but it is
//!   still a valid shuffle. If the pool changes, an added item joins the current
//!   pass and a removed one is forgotten. If the stored history was cut short
//!   (old events deleted) the replay can start in the middle of a pass and
//!   misjudge it once. Passes are numbered per run, so two runs that each begin
//!   a pass on the same date can start from the same order.
//!
//! # Persisting
//!
//! [`extend`] does nothing unless the caller says the clock is trusted (see
//! "Clock" in the design). It generates from the later of `now` and the end of
//! the channel's last stored event up to `now` plus seven days. The caller
//! supplies the pool. `extend` reads the schedule end and the program history,
//! then writes the new events in one transaction. There is no separate cursor
//! to write, so a crash cannot make the next run repeat items. Because the
//! resume point is read from the events, `Store::replace_events_from` also moves
//! it back to the last program it keeps. `extend` never reads the system clock;
//! the caller passes `now_ms`.

mod extend;
mod generate;
mod rng;
#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;

use crate::config::Mode;
use crate::library::MediaId;
use crate::store::StoreError;
use crate::timeline::Event;

pub use extend::{HORIZON_MS, Refill, extend};
#[cfg(test)]
use generate::local_date_days;
pub(crate) use generate::{Params, Resume, generate};

/// The most events one run will produce. A run over seven days that reaches
/// this has items only seconds long, which means the library data is wrong.
pub const MAX_EVENTS: usize = 200_000;

/// What `generate` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Generated {
    pub events: Vec<Event>,
    /// The media id of the last item scheduled. When no event was produced it
    /// is the cursor that came in.
    pub cursor: Option<MediaId>,
    /// Things the caller may want to log. Empty on an ordinary run.
    pub notes: Vec<Note>,
}

/// Something unusual that `generate` handled without failing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Note {
    /// The pool has no item with a runtime above 0, so nothing was scheduled.
    NoPlayableItems,
    /// A sequential channel's cursor item is not in the pool, so the order
    /// restarted at the first item.
    CursorNotInPool { media_id: MediaId },
    /// This many items were left out because their runtime is 0.
    SkippedZeroRuntime { count: usize },
}

#[derive(Debug)]
pub enum ScheduleError {
    /// The mode is not generated yet (`weighted`).
    UnsupportedMode(Mode),
    /// The channel uses time-of-day blocks, which are not generated yet.
    UnsupportedPlan,
    /// A time or runtime is too large to add up as epoch milliseconds, or is
    /// outside the dates the calendar supports.
    TimeOutOfRange,
    /// The run would produce more than [`MAX_EVENTS`] events.
    TooManyEvents,
    Store(StoreError),
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedMode(mode) => write!(f, "mode {mode:?} is not generated yet"),
            Self::UnsupportedPlan => f.write_str("blocks channels are not generated yet"),
            Self::TimeOutOfRange => f.write_str("a time or runtime is out of range"),
            Self::TooManyEvents => write!(f, "more than {MAX_EVENTS} events in one run"),
            Self::Store(error) => write!(f, "schedule store: {error}"),
        }
    }
}

impl Error for ScheduleError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Store(error) => Some(error),
            _ => None,
        }
    }
}

impl From<StoreError> for ScheduleError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}
