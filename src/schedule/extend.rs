//! Keeps a channel's week filled: reads the stored schedule, calls `generate`
//! and writes the new events in one transaction.

use std::collections::HashSet;

use super::{
    BreakPlan, Note, Params, ROTATION_MEMORY_MS, Resume, ScheduleError, cooling_target, generate,
};
use crate::clips::Pool;
use crate::config::{Channel, Mode, Plan};
use crate::library::{MediaId, MediaItem};
use crate::store::Store;

/// How far ahead of `now` a channel's schedule is kept filled: seven days.
pub const HORIZON_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// What [`extend`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extended {
    /// The caller said the clock is not trusted, so nothing was read or written.
    ClockNotTrusted,
    /// The stored schedule already reaches seven days past `now`.
    UpToDate { until_ms: i64 },
    /// Events for `[from_ms, to_ms)` were generated and `written` of them stored.
    Generated {
        from_ms: i64,
        to_ms: i64,
        written: usize,
        notes: Vec<Note>,
    },
}

/// Fills `channel`'s schedule from `pool` up to `now_ms` plus seven days.
///
/// `clips` are the scanned clip pools, usually
/// [`ClipLibrary::pools`](crate::clips::ClipLibrary::pools). When the channel
/// has `breaks`, its break pool and its `filler` pool are looked up there by
/// name; a pool that is not there counts as empty. A channel without breaks
/// ignores `clips`.
///
/// Does nothing unless `clock_trusted` is true: a Pi whose clock has not
/// synchronized must not store a wrong week. Starts at the later of `now_ms`
/// and the end of the last stored event, so repeated calls extend the schedule
/// without gaps. Where to continue comes from the channel's stored program
/// events, and the new events are its only write, so a crash cannot leave the
/// schedule and the resume point disagreeing.
pub fn extend(
    store: &mut Store,
    channel: &Channel,
    pool: &[MediaItem],
    clips: &[Pool],
    now_ms: i64,
    clock_trusted: bool,
) -> Result<Extended, ScheduleError> {
    if !clock_trusted {
        return Ok(Extended::ClockNotTrusted);
    }
    let mode = match &channel.plan {
        Plan::Fixed { mode, .. } => *mode,
        Plan::Blocks(_) => return Err(ScheduleError::UnsupportedPlan),
    };
    let horizon_ms = now_ms
        .checked_add(HORIZON_MS)
        .ok_or(ScheduleError::TimeOutOfRange)?;
    let n = channel.number;

    // Events that end after `now_ms`; the last of them is where to continue.
    let last_end = store
        .events_between(n, now_ms, i64::MAX)?
        .last()
        .map(|event| event.end_ms);
    let from_ms = last_end.map_or(now_ms, |end| end.max(now_ms));
    if from_ms >= horizon_ms {
        return Ok(Extended::UpToDate { until_ms: from_ms });
    }

    // Sequential needs only the last earlier program. Shuffle needs the
    // programs back to the N - k distinct pool items cooling down.
    let recent = if mode == Mode::Shuffle {
        recent_until_cooling(store, n, pool, from_ms)?
    } else {
        store.last_program_before(n, from_ms)?.into_iter().collect()
    };
    let breaks = channel.breaks.as_ref().map(|breaks| {
        let clips_of = |name: &str| {
            clips
                .iter()
                .find(|pool| pool.name() == name)
                .map_or(&[][..], Pool::clips)
        };
        BreakPlan {
            every_ms: millis(breaks.every),
            length_ms: millis(breaks.length),
            clips: clips_of(&breaks.pool),
            pad: channel.filler.as_deref().map_or(&[][..], clips_of),
        }
    });
    // The rotation continues after the last break clip already stored. A clip
    // that started more than ROTATION_MEMORY_MS before `from_ms` cannot affect
    // the result: every new break starts at or after `from_ms`, so the
    // rotation starts again at the first clip either way. Reading only that
    // window keeps a channel that never stored a break from walking its whole
    // history.
    let last_break = match breaks {
        Some(_) => {
            store.last_break_before(n, from_ms.saturating_sub(ROTATION_MEMORY_MS), from_ms)?
        }
        None => None,
    };
    let resume = Resume { recent, last_break };

    let params = Params {
        channel: n,
        mode,
        breaks,
    };
    let generated = generate(&params, pool, &resume, from_ms, horizon_ms)?;
    if !generated.events.is_empty() {
        store.write_events(n, &generated.events)?;
    }
    Ok(Extended::Generated {
        from_ms,
        to_ms: horizon_ms,
        written: generated.events.len(),
        notes: generated.notes,
    })
}

/// A configured duration in whole milliseconds. Config durations are whole
/// seconds or more, so nothing is lost.
fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Reads `channel`'s programs before `before_ms`, newest first, until they hold
/// the N - k distinct pool items a shuffle keeps cooling down or the history
/// ends. Every run therefore sees the same cooling items as an unbroken run
/// would. Pages of twice the pool's length leave room for programs cut into
/// several events and ids no longer in the pool, so one page is usually
/// enough. The read goes further back when the recent rows are mostly such
/// ids, and reads the whole history when it holds fewer than N - k distinct
/// pool items.
fn recent_until_cooling(
    store: &Store,
    channel: u32,
    pool: &[MediaItem],
    before_ms: i64,
) -> Result<Vec<MediaId>, ScheduleError> {
    let (playable, target) = cooling_target(pool);
    let page = u32::try_from(pool.len().saturating_mul(2).max(1)).unwrap_or(u32::MAX);
    let mut recent = Vec::new();
    let mut found = HashSet::new();
    let mut bound = before_ms;
    while found.len() < target {
        let rows = store.recent_programs_before(channel, bound, page)?;
        let full = rows.len() == page as usize;
        for (start_ms, id) in rows {
            bound = start_ms;
            if let Some(&pooled) = playable.get(id.as_str()) {
                found.insert(pooled);
            }
            recent.push(id);
        }
        if !full {
            break;
        }
    }
    Ok(recent)
}
