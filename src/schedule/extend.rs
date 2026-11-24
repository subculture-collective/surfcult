//! Keeps a channel's week filled: reads the stored schedule, calls `generate`
//! and writes the new events in one transaction.

use super::{Note, Params, Resume, ScheduleError, generate};
use crate::config::{Channel, Mode, Plan, Tz};
use crate::library::MediaItem;
use crate::store::Store;

/// How far ahead of `now` a channel's schedule is kept filled: seven days.
pub const HORIZON_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// What [`extend`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refill {
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
    tz: Tz,
    now_ms: i64,
    clock_trusted: bool,
) -> Result<Refill, ScheduleError> {
    if !clock_trusted {
        return Ok(Refill::ClockNotTrusted);
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
        return Ok(Refill::UpToDate { until_ms: from_ms });
    }

    // Every earlier program, oldest first. A program cut into several events
    // counts once. The last one is the cursor; shuffle replays all of them to
    // find where the current pass stands.
    let mut history = Vec::new();
    for id in store.programs_before(n, from_ms)? {
        if history.last() != Some(&id) {
            history.push(id);
        }
    }
    let resume = Resume {
        cursor: history.last().cloned(),
        played: if mode == Mode::Shuffle {
            history
        } else {
            Vec::new()
        },
    };

    let params = Params {
        channel: n,
        mode,
        tz,
    };
    let generated = generate(&params, pool, &resume, from_ms, horizon_ms)?;
    if !generated.events.is_empty() {
        store.write_events(n, &generated.events)?;
    }
    Ok(Refill::Generated {
        from_ms,
        to_ms: horizon_ms,
        written: generated.events.len(),
        notes: generated.notes,
    })
}
