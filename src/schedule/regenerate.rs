//! Replaces a channel's future after its config or its items' durations
//! changed, keeping what is about to air.

use super::extend::fill;
use super::sign_off::nights;
use super::{Extended, HORIZON_MS, ScheduleError};
use crate::clips::Pool;
use crate::config::{Channel, Source, Tz};
use crate::library::MediaItem;
use crate::store::Store;
use crate::timeline::{Event, EventKind};

/// How far ahead of `now` the schedule is frozen: 30 minutes. A regeneration
/// never changes an event that starts before `now` plus this.
pub const FREEZE_MS: i64 = 30 * 60 * 1000;

/// Regenerates `channel`'s schedule after the freeze point, `now_ms` plus
/// [`FREEZE_MS`], up to `now_ms` plus seven days. Takes the same arguments as
/// [`extend`](super::extend); call it when the channel's config or the
/// runtime of an item in its pools changed.
///
/// The new schedule starts at the boundary, the first place at or after the
/// freeze point where the generator can pick up exactly where the stored
/// schedule is:
///
/// - the start of a program (its first event, with `media_offset_ms` 0);
/// - a filler event that follows another filler event, which is a blocks
///   channel's uncovered time and not the padding of a break;
/// - an ident event that starts when a night off the air starts, which is the
///   sign-off clip;
/// - time with no events (a blocks channel's dead air, the gap before a
///   sign-off without filler, or the time off the air): its start, or the
///   freeze point if that falls inside it.
///
/// Every event before the boundary is kept as it is, including a program and
/// its breaks that run past the freeze point, so the show on screen and the
/// next half hour never change. In one transaction the events from the
/// boundary on are deleted and the new ones written
/// ([`Store::replace_events_from`]). The generator resumes from the stored
/// history before the boundary, as [`extend`](super::extend) does, so with
/// unchanged inputs the schedule comes out the same, and a changed runtime
/// changes only the events from the item's next slot after the boundary on.
///
/// When the stored schedule offers no boundary, because it ends before one,
/// the time after its end is filled as [`extend`](super::extend) fills it.
///
/// If the new events have no program and the replaced ones do, the stored
/// schedule is kept and nothing is written (`written` is 0): a pool with
/// nothing playable more often means a library fetch failed than that the
/// channel should go dark. The notes say why, for example
/// [`Note::NoPlayableItems`](super::Note::NoPlayableItems).
///
/// Does nothing unless `clock_trusted` is true.
pub fn regenerate(
    store: &mut Store,
    channel: &Channel,
    pools: &[(Source, Vec<MediaItem>)],
    clips: &[Pool],
    timezone: Tz,
    now_ms: i64,
    clock_trusted: bool,
) -> Result<Extended, ScheduleError> {
    if !clock_trusted {
        return Ok(Extended::ClockNotTrusted);
    }
    let horizon_ms = now_ms
        .checked_add(HORIZON_MS)
        .ok_or(ScheduleError::TimeOutOfRange)?;
    let freeze_ms = now_ms
        .checked_add(FREEZE_MS)
        .ok_or(ScheduleError::TimeOutOfRange)?;
    let n = channel.number;

    let stored = store.events_between(n, now_ms, i64::MAX)?;
    let sign_offs: Vec<i64> = match &channel.sign_off {
        Some(sign_off) => nights(sign_off, timezone, now_ms, horizon_ms)?
            .iter()
            .map(|night| night.start_ms)
            .collect(),
        None => Vec::new(),
    };
    let from_ms = boundary(&stored, &sign_offs, now_ms, freeze_ms).unwrap_or_else(|| {
        stored
            .last()
            .map_or(now_ms, |event| event.end_ms.max(now_ms))
    });
    if from_ms >= horizon_ms {
        return Ok(Extended::UpToDate { until_ms: from_ms });
    }

    let generated = fill(store, channel, pools, clips, timezone, from_ms, horizon_ms)?;
    let is_program = |event: &Event| event.kind == EventKind::Program;
    let replaced = stored.iter().filter(|event| event.start_ms >= from_ms);
    let written = if !generated.events.iter().any(is_program) && replaced.clone().any(is_program) {
        0
    } else {
        store.replace_events_from(n, from_ms, &generated.events)?;
        generated.events.len()
    };
    Ok(Extended::Generated {
        from_ms,
        to_ms: horizon_ms,
        written,
        notes: generated.notes,
    })
}

/// The first place at or after `freeze_ms` in `stored`, the channel's events
/// that end after `now_ms`, where a regeneration can start. See
/// [`regenerate`]. `sign_offs` are the times the channel's nights off the air
/// start. `None` when the events end before one.
fn boundary(stored: &[Event], sign_offs: &[i64], now_ms: i64, freeze_ms: i64) -> Option<i64> {
    let mut previous: Option<&Event> = None;
    for event in stored {
        // Time with no events before `event` that reaches past the freeze point.
        let gap_ms = previous.map_or(now_ms, |p| p.end_ms).max(freeze_ms);
        if event.start_ms > gap_ms {
            return Some(gap_ms);
        }
        // A break's padding follows a break clip or a program, never filler.
        let uncovered = event.kind == EventKind::Filler
            && previous.is_some_and(|p| p.kind == EventKind::Filler);
        let signs_off = event.kind == EventKind::Ident && sign_offs.contains(&event.start_ms);
        if event.start_ms >= freeze_ms && (starts_program(event) || uncovered || signs_off) {
            return Some(event.start_ms);
        }
        previous = Some(event);
    }
    None
}

/// Whether `event` is the first event of a program. A program cut by breaks
/// continues in later events at higher offsets.
fn starts_program(event: &Event) -> bool {
    event.kind == EventKind::Program && event.media_offset_ms == 0
}
