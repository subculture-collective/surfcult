//! Generates a channel's schedule: which media plays when.
//!
//! [`extend`] keeps a channel's week filled in the
//! [`Store`](crate::store::Store). Inside it, `generate` is a pure function. It
//! takes a channel's number, mode, weights and break plan (with the clips it
//! may use), the items to play, the programs and last break clip before the
//! window and a time window, and it returns
//! [`Event`](crate::timeline::Event)s and the new cursor. It reads no clock and
//! does no I/O, so the same inputs always give the same events.
//! `generate_blocks` is its counterpart for a blocks channel: it takes one
//! lane (pool, mode and weights) per block and for the default, the spans the
//! blocks cover in epoch time, and the filler clips.
//!
//! Times are Unix epoch milliseconds (`i64`), as in [`crate::timeline`].
//!
//! # What a run produces
//!
//! Programs run back to back. Without breaks each program is one
//! `EventKind::Program` event that starts where the previous one ended, with
//! `media_offset_ms` 0 and `media_duration_ms` equal to the item's runtime.
//! Events cover the whole window with no gap and no overlap; the last program
//! may run past the end, and a program cut by breaks is always written whole,
//! so a run ends on a program's last segment. Items with a runtime of 0 are
//! skipped, and an item id that appears twice in the pool is used once (the
//! first). Grids are later work; a channel's `grid` setting is ignored for
//! now. A blocks channel differs: see "Blocks".
//!
//! # Breaks
//!
//! A channel with `breaks` has its programs cut, and each cut filled with a
//! break. Breaks add to the slot: a 22-minute program with two 2-minute breaks
//! occupies 26 minutes, as three program events and the break events between
//! them.
//!
//! - **Cuts.** The nominal cuts are the multiples of `breaks.every` in the
//!   program's own time, strictly inside the runtime (none at the end). Each
//!   moves to the nearest Jellyfin chapter mark within [`SNAP_MS`] (60 s),
//!   the earlier one on a tie, if that mark is after the previous cut and
//!   before the end; otherwise it stays at the interval. The program events
//!   share the item's `media_id` and `media_duration_ms`; each starts at the
//!   previous cut's `media_offset_ms`.
//! - **Fill.** A break is filled from the break pool with whole clips: it
//!   walks the pool once, in pool order, starting after the last clip placed,
//!   and places each clip that fits in the time left. A clip is never cut and
//!   never plays twice in one break. A clip too long for the time left waits
//!   for a later break. Each placed clip is one `EventKind::Break` event as
//!   long as the clip (rounded up to a whole millisecond).
//! - **Padding.** Time the clips leave is one `EventKind::Filler` event from
//!   the channel's `filler` pool, exactly as long as the shortfall: the
//!   shortest filler clip at least that long, or the longest one if none is.
//!   The pad is the one piece that may stop before its clip ends, so the
//!   filler pool should hold idents and slates. Without a filler clip the break
//!   is shorter than `breaks.length`; a break with nothing at all to show is
//!   left out and the program runs on. Both are counted in
//!   [`Note::ShortBreaks`].
//! - **Rotation and limits.** `channels.toml` has no rotation or frequency
//!   settings, so these are the rules: clips rotate in pool order across
//!   breaks, and a clip plays at most once per break. The rotation continues
//!   after the channel's last stored break clip (`Store::last_break_before`);
//!   an id no longer in the pool restarts it at the first clip. The rotation
//!   remembers its place for [`ROTATION_MEMORY_MS`] (24 hours): a break that
//!   starts later than that after the last break clip started begins at the
//!   first clip, in an unbroken run as in a resumed one, so `extend` reads only
//!   the last day of events. Clips that share a media id (one file reached
//!   through two links) count once, at the first one's place in the pool.
//! - **Clip ids.** A clip's `media_id` is its canonical path as text
//!   ([`clip_media_id`]); a path that is not UTF-8 is left out. The event kind
//!   says whether the id is a clip or a Jellyfin item. The tuner already joins
//!   at the program after a break (`timeline::program_from`), and the
//!   shuffle's cooldown counts a cut program once.
//! - **Determinism.** Cuts, fill and padding use no randomness; they depend
//!   only on the item, the pools and the last stored break clip, so a run
//!   resumed at a program boundary matches an unbroken one.
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
//! # Weighted
//!
//! Each program slot picks one playable item with probability proportional to
//! its weight. `weights` in `channels.toml` maps an item title to a whole
//! number of at least 1; every pool item with that title has that weight and
//! every other item weighs 1. Over many picks each item's share approaches its
//! weight divided by the total.
//!
//! - **Seed.** The pick draws a number below the total weight from SplitMix64
//!   seeded with the channel number and the slot's `start_ms`, as the shuffle
//!   does, and takes the item whose running total first exceeds it, in pool
//!   order. It reads no history, so a resumed run matches an unbroken one.
//! - **Limits.** There is no cooldown: an item can play twice in a row, and a
//!   heavy item often will. A weight whose title matches no playable item is
//!   reported with [`Note::WeightNotInPool`].
//!
//! # Blocks
//!
//! A blocks channel's time is cut into spans (`blocks.rs`). Each block covers
//! `[start, end)` in local wall-clock time on each of its days, in the config's
//! time zone, and the spans hold the real instants, so a block that runs
//! 00:00 to 24:00 lasts 23 hours on the day the clocks go forward and 25 on the
//! day they go back.
//!
//! - **Clock changes.** A local time that occurs twice is its first
//!   occurrence. A local time that does not occur is the moment the clocks
//!   jump, the first local time after it; a block that lies wholly in the
//!   skipped hour has no span that day.
//! - **Programs.** A block plays its own pool in its own mode. The first
//!   program starts at the block's start, and a program, break or clip still
//!   on at the block's end is cut there; its event keeps the item's
//!   `media_duration_ms`. Break clips cut off this way never play, so the
//!   break rotation continues after the last one that did.
//! - **Fallback.** Time no block covers plays the channel's `default` lineup
//!   when it has one. Otherwise it plays the channel's `filler` pool: its clips
//!   in pool order, whole, over and over, counted from the start of the
//!   uncovered stretch so a resumed run picks up mid-cycle, with the last clip
//!   cut at the stretch's end. With neither, the time has no events (dead air)
//!   and [`Note::DeadAir`] says how long. A block whose pool has nothing
//!   playable falls back the same way.
//! - **Resuming.** Each lane keeps its own place across the run and across
//!   runs. A sequential block continues after the newest program, in the
//!   channel's history, that is in its pool, so Saturday's cartoons continue
//!   from last Saturday. A shuffle block's cooldown counts only programs from
//!   its pool. Every program counts for every lane whose pool holds it, so two
//!   blocks that share items share their place. A run that resumes at an event
//!   boundary gives the events an unbroken run would.
//!
//! # Persisting
//!
//! [`extend`] does nothing unless the caller says the clock is trusted (see
//! "Clock" in the design). It generates from the later of `now` and the end of
//! the channel's last stored event up to `now` plus seven days. The caller
//! supplies the pools: it resolves each source of `Plan::sources` with
//! [`library::resolve_pool`](crate::library::resolve_pool) and passes the pairs
//! to `extend`, with the config's time zone. `resolve_pool` handles every
//! source kind.
//! The caller also passes the scanned clip pools
//! ([`ClipLibrary::pools`](crate::clips::ClipLibrary::pools)); `extend` looks
//! up the channel's break and filler pools there by name, and a pool that is
//! missing counts as empty. A channel with breaks also reads its last stored
//! break clip, where the rotation continues.
//! `extend` reads the schedule end and where to resume, then writes the new
//! events in one transaction. A sequential or weighted channel reads only its
//! last stored program. A blocks channel reads its recent programs until each
//! sequential block has found one program from its pool and each shuffle
//! block its N - k items, or the history ends. A shuffle channel reads its
//! recent programs back to the N - k
//! distinct pool items cooling down. Usually that is one page of twice the
//! pool's length; it reads further back when the recent programs are mostly
//! ids no longer in the pool, and reads the whole history when that holds
//! fewer than N - k distinct pool items. There is no separate cursor to write,
//! so a crash cannot make the next run repeat items. Because the resume point
//! is read from the events, `Store::replace_events_from` also moves it back to
//! the last program it keeps. `extend` never reads the system clock; the
//! caller passes `now_ms`.
//!
//! # Regenerating
//!
//! [`regenerate`] takes the same arguments as [`extend`] and replaces the
//! schedule after the freeze point, `now` plus [`FREEZE_MS`] (30 minutes),
//! when a channel's config or an item's runtime changed. It starts at the
//! first boundary at or after the freeze point: a program's start, a blocks
//! channel's uncovered filler, or time with no events. It keeps every event
//! before it (so a program and its breaks running past the freeze point are
//! kept whole), and resumes from the history before it as `extend` does.
//! With unchanged inputs the schedule comes out the same; a changed runtime
//! changes only events from the item's next slot after the boundary.
//!
//! # When a file's length differs
//!
//! A kept event still has the runtime it was generated with. At play time the
//! scheduled end wins: [`fit`] takes a program event and the file's real
//! length and returns what plays in the event's time. A short file is padded
//! from the filler pool to the event's end; a long one is cut there. A
//! difference of up to [`FIT_TOLERANCE_MS`] is no mismatch.
//! [`record_fit`] writes the mismatch to playback history as `padded`, `short`
//! (no filler clip) or `cut`. Neither changes the stored schedule.

mod blocks;
mod breaks;
mod extend;
mod fit;
mod generate;
mod regenerate;
mod rng;
#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;

use crate::library::MediaId;
use crate::store::StoreError;
use crate::timeline::Event;

pub(crate) use breaks::BreakPlan;
pub use breaks::{ROTATION_MEMORY_MS, SNAP_MS, clip_media_id};
pub use extend::{Extended, HORIZON_MS, extend};
pub use fit::{FIT_TOLERANCE_MS, Fit, Fitted, fit, record_fit};
pub(crate) use generate::{
    Lane, Layout, Params, Resume, cooling_target, generate, generate_blocks,
};
pub use regenerate::{FREEZE_MS, regenerate};

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
    /// This many breaks came out shorter than `breaks.length`, or were left
    /// out, because the break pool's clips did not fill them and the channel
    /// has no filler clip to pad the rest.
    ShortBreaks { count: usize },
    /// A weighted lane has a weight for this title and no playable item has
    /// it, so the weight does nothing.
    WeightNotInPool { title: String },
    /// A blocks channel had this many milliseconds with nothing to play: no
    /// block, no playable default and no filler clip. That time has no events.
    DeadAir { ms: u64 },
}

#[derive(Debug)]
pub enum ScheduleError {
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
