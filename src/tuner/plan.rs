//! The two choices the tuner makes when a clip is about to start: which clip
//! to play, and which program to queue behind it.

use super::Reason;
use crate::clips::Clip;
use crate::library::MediaId;
use crate::timeline::{self, Event, Lookup};

/// A program to queue and where in it to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub media_id: MediaId,
    pub offset_ms: u64,
}

/// The clip's length in milliseconds; saturates at `u64::MAX`.
pub(super) fn millis(clip: &Clip) -> u64 {
    u64::try_from(clip.duration.as_millis()).unwrap_or(u64::MAX)
}

/// The shortest clip that is at least `start_cost_ms` long. If none is that
/// long, the longest clip. Of equal clips the first in pool order wins.
/// `None` only when there are no clips.
pub(super) fn choose_clip(clips: &[Clip], start_cost_ms: u64) -> Option<&Clip> {
    let long_enough = clips
        .iter()
        .filter(|clip| millis(clip) >= start_cost_ms)
        .min_by_key(|clip| millis(clip));
    long_enough.or_else(|| {
        clips
            .iter()
            .fold(None, |best: Option<&Clip>, clip| match best {
                Some(best) if millis(best) >= millis(clip) => Some(best),
                _ => Some(clip),
            })
    })
}

/// The program to queue so it is in the right place at `at_ms`, when the
/// tune-in clip ends. [`timeline::program_from`] makes the choice: a program
/// on at `at_ms` at the look-ahead offset, or, when a break, filler or ident is
/// on, the program after the break at the offset where its event starts. The
/// clip stands in for the break.
///
/// An empty schedule, no event on at `at_ms`, or no program after the break is
/// an error. So is a program after the break that starts more than
/// `max_early_ms` after `at_ms`: the run before it is a long filler stretch,
/// such as time no block covers, not a break the clip can stand in for.
pub(super) fn program_at(
    events: &[Event],
    at_ms: i64,
    max_early_ms: u64,
) -> Result<Target, Reason> {
    if events.is_empty() {
        return Err(Reason::NoEvents);
    }
    match timeline::program_from(events, at_ms) {
        Lookup::On { event, .. }
            if event.start_ms > at_ms && event.start_ms.abs_diff(at_ms) > max_early_ms =>
        {
            Err(Reason::NoProgramSoon {
                next_program_ms: event.start_ms,
            })
        }
        Lookup::On {
            event, offset_ms, ..
        } => Ok(Target {
            media_id: event.media_id.clone(),
            offset_ms,
        }),
        Lookup::Gap { next_start_ms } => Err(Reason::Gap { next_start_ms }),
    }
}
