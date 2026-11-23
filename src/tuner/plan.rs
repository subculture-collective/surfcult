//! The two choices the tuner makes when a clip is about to start: which clip
//! to play, and which program to queue behind it.

use super::Reason;
use crate::clips::Clip;
use crate::library::MediaId;
use crate::timeline::{Event, EventKind, Lookup, lookup};

/// A program to queue and where in it to start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Target {
    pub media_id: MediaId,
    pub offset_ms: u64,
}

fn millis(clip: &Clip) -> u64 {
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
/// tune-in clip ends.
///
/// - A program event on at `at_ms` gives that program at the look-ahead
///   offset.
/// - A break, filler or ident event on at `at_ms` means the clip stands in for
///   the break. The target is the program that follows the break, at the
///   offset where that event starts. A break can be several events in a row;
///   the target is the first program event after all of them.
/// - No event on at `at_ms`, or no program after the break, is an error.
pub(super) fn program_at(events: &[Event], at_ms: i64) -> Result<Target, Reason> {
    match lookup(events, at_ms) {
        Lookup::Gap { .. } if events.is_empty() => Err(Reason::NoEvents),
        Lookup::Gap { next_start_ms } => Err(Reason::Gap { next_start_ms }),
        Lookup::On { event, offset_ms } if event.kind == EventKind::Program => Ok(Target {
            media_id: MediaId::new(event.media_id.clone()),
            offset_ms,
        }),
        Lookup::On { event, .. } => program_after_break(events, event),
    }
}

/// `event` is a non-program event inside `events`. Walks forward through the
/// break to the program that follows it.
fn program_after_break(events: &[Event], event: &Event) -> Result<Target, Reason> {
    let index = events
        .iter()
        .position(|candidate| std::ptr::eq(candidate, event))
        .unwrap_or(events.len());
    let mut break_end = event.end_ms;
    for next in events.iter().skip(index + 1) {
        if next.start_ms != break_end {
            return Err(Reason::Gap {
                next_start_ms: Some(next.start_ms),
            });
        }
        if next.kind == EventKind::Program {
            return Ok(Target {
                media_id: MediaId::new(next.media_id.clone()),
                offset_ms: next.media_offset_ms,
            });
        }
        break_end = next.end_ms;
    }
    Err(Reason::Gap {
        next_start_ms: None,
    })
}
