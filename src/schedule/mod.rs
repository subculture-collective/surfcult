//! Generates a channel's schedule: which media plays when.
//!
//! [`extend`] keeps a channel's week filled in the
//! [`Store`](crate::store::Store). Inside it, `generate` is a pure function. It
//! takes a channel's number, mode and break plan (with the clips it may use),
//! the pools to play, each with its weight, the programs and last break clip
//! before the window and a time window, and it returns
//! [`Event`](crate::timeline::Event)s and the new cursor. It reads no clock and
//! does no I/O, so the same inputs always give the same events.
//! `generate_blocks` is its counterpart for a blocks channel: it takes one
//! lane (pools with their weights, and a mode) per block and for the default, the spans the
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
//! first). A channel with a `grid` differs: see "Grids". A blocks channel
//! differs too: see "Blocks".
//!
//! # Grids
//!
//! With `grid` set, a program starts only on a multiple of the grid in epoch
//! time. When the last event ends off the grid (a program of another length,
//! or the start of a run), the time to the next grid line is a gap, filled
//! before the next program: whole clips drawn from the break pools, then whole
//! idents (`breaks.pad`), then whole clips from the `filler` pool, each pool
//! walked once, a clip placed only if it fits in the time left. Time still
//! left is one event as long as that time from the shortest ident at least
//! that long (the longest if none is), else from the filler pool: one clip
//! repeated as a last resort. Break clips advance the rotation and the repeat
//! limit as a break does. With no clip at all the gap has no events and is
//! counted in [`Note::DeadAir`]. A gap is cut where its block or stretch ends.
//! Without `grid` nothing changes.
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
//! - **Fill.** A break plays one clip from its `open` pool, then whole clips
//!   drawn from its weighted pools, then the pad, then one clip from its
//!   `close` pool, chosen when the break starts so its length is kept free.
//!   Each draw picks a pool by weight (SplitMix64 seeded with the channel and
//!   the time the draws start) and takes the next clip of that pool's walk,
//!   in the order its `rotation` gives, that fits in the time left and that
//!   `repeat_after` does not hold back; a pool with no such clip left drops
//!   out. With one pool that places each clip that fits, in rotation order.
//!   A clip is never cut and never plays twice in one break. A clip too long
//!   for the time left waits for a later break. Each placed clip is one event
//!   as long as the clip (rounded up to a whole millisecond), of the kind its
//!   pool's `kind` gives: `Break` for commercials and trailers, `Ident` for
//!   bumps, stingers and idents, `Filler` for filler.
//! - **Padding.** Time the clips leave is one pad event exactly as long as
//!   the shortfall: an `EventKind::Ident` from the `breaks.pad` pool when it
//!   has a clip, else an `EventKind::Filler` from the channel's `filler` pool.
//!   It plays the pool's shortest clip at least that long, or its longest one
//!   if none is. The pad is the one piece that may stop before its clip ends,
//!   so both pools should hold idents and slates. Without a pad or filler
//!   clip the break is shorter than `breaks.length`; a break with nothing at all to show is
//!   left out and the program runs on. Both are counted in
//!   [`Note::ShortBreaks`].
//! - **Rotation and limits.** Each pool's `rotation` and `repeat_after` keys
//!   set them, and each pool has one rotation per channel however many times
//!   the breaks and `between` name it; a clip plays at most once per break
//!   either way. Every clip the channel airs, whatever placed it, counts as
//!   an airing in each pool that holds it. With `rotation = "order"` (the
//!   default) clips rotate in pool order, continuing after the pool's newest
//!   airing. The rotation remembers its place for [`ROTATION_MEMORY_MS`] (24
//!   hours): a draw that starts later than that after the pool's last clip
//!   started begins at the first clip, in an unbroken run as in a resumed
//!   one. With `rotation = "shuffle"` each break walks the pool in an order
//!   drawn from SplitMix64 seeded with the channel and the walk's start. With
//!   `repeat_after`, a clip does not play again until that long after its
//!   last airing started on the channel; a clip still cooling down is
//!   skipped, and a break no clip can fill is padded. `extend` reads the
//!   stored clip events (`Store::clips_between`) back to the longer of
//!   `ROTATION_MEMORY_MS` and the longest `repeat_after`. Clips that share a
//!   media id (one file reached through two links) count once, at the first
//!   one's place in the pool. Config validation refuses `rotation` and
//!   `repeat_after` on a pool no channel's `breaks` or `between` draws from.
//! - **Clip ids.** A clip's `media_id` is its canonical path as text
//!   ([`clip_media_id`]); a path that is not UTF-8 is left out. The event kind
//!   says whether the id is a clip or a Jellyfin item. The tuner already joins
//!   at the program after a break (`timeline::program_from`), and the
//!   shuffle's cooldown counts a cut program once.
//! - **Determinism.** Cuts, padding and an `order` rotation use no
//!   randomness, and a `shuffle` rotation's seed is only the channel and the
//!   break's start. The result depends only on the item, the pools and the
//!   stored break clips, so a run resumed at a program boundary matches an
//!   unbroken one.
//!
//! # Between
//!
//! A channel with `between` plays clips where a program ends and the next
//! program of the same stretch starts: one clip each from `open`, the
//! weighted pools and `close`, or, with a `length`, a fill exactly like a
//! break's, padded the same way. It does not play before the first program
//! of a block or of the time after a sign-off, after filler or dead air, or
//! before a program taken back for a sign-off. A run that starts where a
//! stored program ends (`Resume::last_event`) places one first, so a resumed
//! run matches an unbroken one.
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
//! A weighted lineup has several sources, each with a weight, and each program
//! slot first picks a source and then an item from it. `sources` in
//! `channels.toml` lists them, for example
//! `{ jellyfin_series = "The Simpsons", weight = 3 }`; a weight is a whole
//! number of at least 1 and is 1 when left out. Over many picks a source's
//! share of the programs approaches its weight divided by the total, however
//! many items it holds.
//!
//! - **Seed.** One SplitMix64 per slot, seeded with the channel number and the
//!   slot's `start_ms` as the shuffle is. Its first draw is a number below the
//!   total weight of the playable sources; the source whose running total of
//!   weights first exceeds it, in config order, is picked. Its next draw
//!   picks the item, as the shuffle does, from the source's pool.
//! - **Cooldown.** Each source's pool has its own shuffle cooldown (see
//!   Shuffle): with N distinct playable items in it, the N - k most recently
//!   played distinct items of that pool, k = max(1, ceil(N / 4)), cannot be
//!   picked from it. The history is the programs before the slot, whichever
//!   source picked them, that are in the pool, so an item two sources share
//!   cools down in both. No item repeats within N - k + 1 consecutive programs
//!   of its pool, and a one-item source plays that item whenever it is picked.
//! - **Empty sources.** A source with no playable item is left out of the
//!   draw and reported with [`Note::SourceNotPlayable`]; the others share the
//!   picks by their weights. With none playable the lane has nothing to
//!   play, like any lane, and says [`Note::NoPlayableItems`].
//! - **History.** `extend` reads recent programs until every source's pool
//!   holds its N - k cooling items or the history ends, as for a shuffle, so
//!   a resumed run matches an unbroken one.
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
//! # Sign-off
//!
//! A channel with `sign_off` ends each night with one clip and is then off
//! the air (`sign_off.rs`). Each night is the epoch span from `at` to `until`
//! in the config's time zone, read with the block rules for clock changes;
//! `until` is the next morning when it is not after `at`.
//!
//! - **Before.** No program, and so none of its breaks, starts if it would
//!   still be on at `at`: the generator writes each program before the
//!   sign-off whole and takes it back if it ends later. The gap to `at` plays
//!   the filler pool in pool order, whole clips, in cycles counted back from
//!   `at` so the last clip ends as the sign-off starts; without filler it is
//!   dead air.
//! - **The clip.** One `EventKind::Ident` event at `at`, as long as the clip
//!   (cut at `until` if longer): the named file in the pool, else the pool's
//!   first usable clip. With none, [`Note::NoSignOffClip`].
//! - **Off the air.** No events from the end of the clip to `until`. It
//!   overrides a blocks channel's blocks and default.
//! - **Resuming.** At `until` the plan resumes where it left off. A run that
//!   starts inside the stretch before a sign-off is in the gap when the
//!   channel's last stored event (`Resume::last_event`) is filler, or is a
//!   program that ended earlier in the stretch and after which the next
//!   program would still be on at `at`. When no stored event ends inside the
//!   stretch, it is in the gap when the first program from the stretch's
//!   start would still be on at `at`. Otherwise it places programs, so a run
//!   after an outage does not fill the day with filler. Everything else
//!   depends only on the time, so a resumed run matches an unbroken one.
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
//! up the pools the channel's breaks and `between` draw from and its pad,
//! filler and sign-off pools there by name, and a pool that is missing counts
//! as empty. A channel with breaks or `between` also reads its recent stored
//! clips, where the rotations continue and the repeat limits count from.
//! `extend` reads the schedule end and where to resume, then writes the new
//! events in one transaction. A sequential channel reads only its last stored
//! program. A blocks channel reads its recent programs until each
//! sequential block has found one program from its pool and each shuffle
//! block, and each source of a weighted block, its N - k items, or the history
//! ends. A shuffle or weighted channel reads its recent programs back to the
//! N - k distinct items cooling down in each pool. Usually that is one page of twice the
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
//! channel's uncovered filler or the gap before a sign-off, the sign-off
//! clip, or time with no events. It keeps every event
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
mod sign_off;
#[cfg(test)]
mod tests;

use std::error::Error;
use std::fmt;

use crate::config::Source;
use crate::library::MediaId;
use crate::store::StoreError;
use crate::timeline::Event;

pub(crate) use breaks::BreakPlan;
pub use breaks::{ROTATION_MEMORY_MS, SNAP_MS, clip_media_id};
pub use extend::{Extended, HORIZON_MS, extend};
pub use fit::{FIT_TOLERANCE_MS, Fit, Fitted, fit, record_fit};
pub(crate) use generate::{
    Lane, Layout, Params, Resume, SourcePool, cooling_target, generate, generate_blocks,
    generate_fixed,
};
pub use regenerate::{FREEZE_MS, regenerate, reschedule};

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
    /// This many breaks, or `between`s with a length, came out shorter than
    /// their length, or were left out, because the break pools' clips did not fill them and the channel
    /// has no filler clip to pad the rest.
    ShortBreaks { count: usize },
    /// A source of a weighted lane has no item with a runtime above 0, so it
    /// is left out of the picks.
    SourceNotPlayable { source: Source },
    /// The channel had this many milliseconds with nothing to play and no
    /// filler clip: a blocks channel's time with no block or playable
    /// default, or the gap before a sign-off. That time has no events.
    DeadAir { ms: u64 },
    /// The sign-off pool has no usable clip, so the channel went off the air
    /// at the sign-off time without one.
    NoSignOffClip,
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
