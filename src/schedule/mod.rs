//! Generates a channel's schedule: which media plays when.
//!
//! [`extend`] keeps a channel's week filled in the
//! [`Store`](crate::store::Store). Inside it, `generate` is a pure function. It
//! takes a channel's number and mode, the items to play, the programs before
//! the window and a time window, and it returns
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
//! Items play in the pool's order. A run continues after the cursor item, the
//! media id of the last program already stored, and wraps from the last item
//! to the first. With no cursor it starts at the first item. If the cursor
//! item is no longer in the pool it starts at the first item and says so with
//! [`Note::CursorNotInPool`]; that restarts the order, which is better than
//! guessing a position.
//!
//! # Shuffle
//!
//! Each program slot picks at random from the items that are not cooling down.
//!
//! - **Rule.** Let N be the number of distinct playable items in the pool and
//!   k = max(1, ceil(N / 4)). Walk the earlier programs newest first (the
//!   resumed history, then this run's picks) and collect distinct pool ids
//!   until there are N - k of them: those items are cooling down. Every other
//!   playable item is eligible, including one that never played. An id that is
//!   no longer in the pool is ignored and does not count toward N - k. When the
//!   history holds fewer than N - k distinct pool items, fewer are cooling down.
//!   At least k items are always eligible, so a pick never fails.
//! - **What it guarantees.** No item repeats within N - k + 1 consecutive
//!   programs. With N = 1 the one item always plays; with N >= 2 the item that
//!   just played never plays next; with N up to 4, k is 1 and the items rotate
//!   in a fixed order once the history is full. An eligible item stays eligible
//!   until it plays, so every item plays eventually. There is no pass: an item
//!   can play again before some other item has played once.
//! - **Seed.** Each pick draws an index into the eligible items, in pool order,
//!   from SplitMix64 (`rng.rs`) seeded with the channel number and the slot's
//!   `start_ms`. Nothing else goes into the seed: no date, counter or run state.
//!   A run that stops at an event boundary and resumes from the stored history
//!   therefore produces exactly the events an unbroken run would, provided the
//!   history it is given reaches back to N - k distinct pool items or to the
//!   first stored program; [`extend`] reads that far. The generator uses no
//!   `rand` crate and no hasher with random state, so the same inputs give the
//!   same order on every machine.
//! - **History.** `Resume::recent` holds the earlier programs newest first.
//!   [`extend`] reads it from the channel's stored program events in pages of
//!   twice the pool's length, going further back until the rows hold N - k
//!   distinct pool items or the history ends. A program cut into several
//!   events counts once in the walk.
//!
//! # Persisting
//!
//! [`extend`] does nothing unless the caller says the clock is trusted (see
//! "Clock" in the design). It generates from the later of `now` and the end of
//! the channel's last stored event up to `now` plus seven days. The caller
//! supplies the pool: it takes `source` from the channel's `Plan::Fixed`,
//! resolves it with [`library::resolve_pool`](crate::library::resolve_pool) and
//! passes the result to `extend`. Until issue #30 only collections resolve.
//! `extend` reads the schedule end and where to resume, then writes the new
//! events in one transaction. A sequential channel reads only its last stored
//! program. A shuffle channel reads its recent programs back to the N - k
//! distinct pool items cooling down. Usually that is one page of twice the
//! pool's length; it reads further back when the recent programs are mostly
//! ids no longer in the pool, and reads the whole history when that holds
//! fewer than N - k distinct pool items. There is no separate cursor to write,
//! so a crash cannot make the next run repeat items. Because the resume point
//! is read from the events, `Store::replace_events_from` also moves it back to
//! the last program it keeps. `extend` never reads the system clock; the
//! caller passes `now_ms`.

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

pub use extend::{Extended, HORIZON_MS, extend};
pub(crate) use generate::{Params, Resume, cooling_target, generate};

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
