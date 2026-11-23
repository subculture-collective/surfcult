//! The thin layer that reads the store, calls [`generate`] and writes back.

use super::{Note, Params, Resume, ScheduleError, generate};
use crate::config::{Channel, Mode, Plan, Tz};
use crate::library::MediaSource;
use crate::store::Store;
use crate::timeline::EventKind;

/// How far ahead of `now` a channel's schedule is kept filled: seven days.
pub const HORIZON_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// What [`refill_channel`] did.
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

/// Fills `channel`'s schedule up to `now_ms` plus seven days.
///
/// Does nothing unless `clock_trusted` is true: a Pi whose clock has not
/// synchronized must not store a wrong week. Starts at the later of `now_ms`
/// and the end of the last stored event, so repeated calls extend the schedule
/// without gaps. The pool is every item `source` lists; turning a channel's
/// configured source into a smaller pool comes later.
pub fn refill_channel(
    store: &mut Store,
    source: &impl MediaSource,
    channel: &Channel,
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

    // Events that end after `now_ms`; the last of them is where to continue.
    let last_end = store
        .events_between(channel.number, now_ms, i64::MAX)?
        .last()
        .map(|event| event.end_ms);
    let from_ms = last_end.map_or(now_ms, |end| end.max(now_ms));
    if from_ms >= horizon_ms {
        return Ok(Refill::UpToDate { until_ms: from_ms });
    }

    let pool = source.list_items()?;
    let mut resume = Resume {
        cursor: store.cursor(channel.number)?,
        played: Vec::new(),
    };
    if mode == Mode::Shuffle {
        // All earlier programs, so the generator can find where the current
        // pass stands. A program cut into several events counts once.
        for event in store.events_between(channel.number, i64::MIN, from_ms)? {
            if event.kind == EventKind::Program && resume.played.last() != Some(&event.media_id) {
                resume.played.push(event.media_id);
            }
        }
    }

    let params = Params {
        channel: channel.number,
        mode,
        tz,
    };
    let generated = generate(&params, &pool, &resume, from_ms, horizon_ms)?;
    let written = generated.events.len();
    if let Some(cursor) = generated.cursor.as_deref().filter(|_| written > 0) {
        store.write_events(channel.number, &generated.events)?;
        store.set_cursor(channel.number, cursor)?;
    }
    Ok(Refill::Generated {
        from_ms,
        to_ms: horizon_ms,
        written,
        notes: generated.notes,
    })
}
