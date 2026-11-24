//! What plays in a program event when the file's real length differs from
//! the length it was scheduled with. The scheduled end always wins.

use super::breaks::usable;
use crate::clips::Clip;
use crate::store::{Store, StoreError};
use crate::timeline::{Event, EventKind};

/// How far a file's real length may differ from its scheduled length and
/// still count as [`Fit::Exact`]: 500 ms, the tuner's late tolerance. A
/// player or probe often reports a length a few milliseconds off the
/// library's runtime, which is not worth padding or recording.
pub const FIT_TOLERANCE_MS: u64 = 500;

/// How a file's real length compares with its event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// The file covers the event and does not run past it, or runs past it
    /// only where the schedule cuts it anyway (at a break or a block's end).
    Exact,
    /// The file ends `by_ms` before the event's end. `padded` says whether the
    /// filler pool covered the rest; without a filler clip the rest is empty.
    Short { by_ms: u64, padded: bool },
    /// The event reaches the item's scheduled end and the file runs `by_ms`
    /// past it. The file is cut at the event's end.
    Long { by_ms: u64 },
}

impl Fit {
    /// The playback-history outcome for a mismatch: `padded`, `short` (no
    /// filler clip to pad with) or `cut`. `None` for [`Fit::Exact`].
    pub fn outcome(self) -> Option<&'static str> {
        match self {
            Self::Exact => None,
            Self::Short { padded: true, .. } => Some("padded"),
            Self::Short { padded: false, .. } => Some("short"),
            Self::Long { .. } => Some("cut"),
        }
    }
}

/// What [`fit`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fitted {
    /// What plays in the event's time, back to back. With a filler clip the
    /// events end at the event's end. Without one, a short file's events end
    /// where the file ends, and when the file does not reach the event at all
    /// (it ends at or before `media_offset_ms`) the list is empty and nothing
    /// plays until the event's end.
    pub events: Vec<Event>,
    pub fit: Fit,
}

/// Fits the file of program `event` into the event's time, given the file's
/// real length `real_ms`: the library's runtime after a sync corrected it, or
/// the length the player reports.
///
/// The event plays the media from `media_offset_ms` for its length. If the
/// file ends before that, the program part ends where the file does and the
/// rest, up to the event's end, is filled from `filler`: its clips from the
/// first, in pool order, whole, over and over, with the last one cut at the
/// event's end. A file that ends before the event's offset leaves the whole
/// event to filler. If the event reaches the item's scheduled end
/// (`media_duration_ms`) and the file is longer, the event is unchanged and
/// the file is cut at its end. A difference of up to [`FIT_TOLERANCE_MS`]
/// either way is [`Fit::Exact`] and changes nothing. Break, filler and ident events
/// come back unchanged as [`Fit::Exact`]; their clips' lengths are probed
/// when the pools are scanned.
///
/// The stored schedule is not changed: this is what the event plays, and the
/// events after it start on time.
pub fn fit(event: &Event, real_ms: u64, filler: &[Clip]) -> Fitted {
    let unchanged = |fit| Fitted {
        events: vec![event.clone()],
        fit,
    };
    if event.kind != EventKind::Program {
        return unchanged(Fit::Exact);
    }
    let length_ms = event.end_ms.abs_diff(event.start_ms);
    let media_end_ms = event.media_offset_ms.saturating_add(length_ms);
    if real_ms.saturating_add(FIT_TOLERANCE_MS) < media_end_ms {
        let played_ms = real_ms.saturating_sub(event.media_offset_ms);
        let mut at_ms = event.start_ms.saturating_add_unsigned(played_ms);
        let mut events = Vec::new();
        if played_ms > 0 {
            events.push(Event {
                end_ms: at_ms,
                ..event.clone()
            });
        }
        let clips = usable(filler);
        for (media_id, clip_ms) in clips.iter().cycle() {
            if at_ms >= event.end_ms {
                break;
            }
            let end_ms = at_ms.saturating_add_unsigned(*clip_ms).min(event.end_ms);
            events.push(Event {
                start_ms: at_ms,
                end_ms,
                media_id: media_id.clone(),
                media_offset_ms: 0,
                media_duration_ms: *clip_ms,
                kind: EventKind::Filler,
            });
            at_ms = end_ms;
        }
        return Fitted {
            events,
            fit: Fit::Short {
                by_ms: media_end_ms - real_ms,
                padded: !clips.is_empty(),
            },
        };
    }
    if real_ms > media_end_ms.saturating_add(FIT_TOLERANCE_MS)
        && media_end_ms >= event.media_duration_ms
    {
        return unchanged(Fit::Long {
            by_ms: real_ms - media_end_ms,
        });
    }
    unchanged(Fit::Exact)
}

/// Records a mismatch in `channel`'s playback history, under the program's
/// media id, at `at_ms`, with the outcome word of `fit`
/// ([`Fit::outcome`]). Records nothing for [`Fit::Exact`]. Returns whether a
/// row was written.
///
/// It records one row per call, and the caller calls it once per program
/// event it fits. A program cut by breaks is several events, so a short file
/// writes one row for every segment it falls short in: the segment where it
/// ends and each later segment it does not reach.
pub fn record_fit(
    store: &mut Store,
    channel: u32,
    event: &Event,
    fit: Fit,
    at_ms: i64,
) -> Result<bool, StoreError> {
    let Some(outcome) = fit.outcome() else {
        return Ok(false);
    };
    store.record_playback(channel, &event.media_id, at_ms, outcome)?;
    Ok(true)
}
