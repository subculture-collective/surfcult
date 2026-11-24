//! Commercial breaks: where a program is cut and which clips fill each break.
//! See "Breaks" in the module docs of `schedule` for the rules.

use std::collections::HashSet;
use std::time::Duration;

use crate::clips::Clip;
use crate::library::{MediaId, MediaItem};
use crate::timeline::EventKind;

/// How far a cut may move to reach a chapter mark: 60 seconds either way.
pub const SNAP_MS: u64 = 60_000;

/// How long the break rotation remembers its place: 24 hours. A break that
/// starts more than this after the last break clip started begins again at
/// the pool's first clip. The bound lets `extend` read only the last day of
/// events for the last break clip, and because an unbroken run applies the
/// same rule, a resumed run still matches it.
pub const ROTATION_MEMORY_MS: i64 = 24 * 60 * 60 * 1000;

/// A channel's break settings and the clips they may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BreakPlan<'a> {
    /// The interval between cuts, in program time.
    pub every_ms: u64,
    /// How long each break runs.
    pub length_ms: u64,
    /// The break pool's clips, in pool order.
    pub clips: &'a [Clip],
    /// The channel's filler pool, which pads a break the clips do not fill.
    pub pad: &'a [Clip],
}

/// One event of a break, before it is given a start time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Piece {
    pub media_id: MediaId,
    pub length_ms: u64,
    pub media_duration_ms: u64,
    pub kind: EventKind,
}

/// The media id a clip is scheduled under: its canonical path as text.
/// `None` for a path that is not valid UTF-8, which is left out.
pub fn clip_media_id(clip: &Clip) -> Option<MediaId> {
    clip.path.to_str().map(MediaId::new)
}

/// A clip's length in whole milliseconds, rounded up so its event is never
/// shorter than the clip.
fn clip_ms(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos().div_ceil(1_000_000)).unwrap_or(u64::MAX)
}

/// The clips a break can use, with ids and lengths, in pool order. Clips
/// without a valid id or with no length are left out, and so is a clip whose
/// id an earlier clip already has (one file reached through two links): the
/// first in pool order stays.
fn usable(clips: &[Clip]) -> Vec<(MediaId, u64)> {
    let mut seen = HashSet::new();
    clips
        .iter()
        .filter_map(|clip| Some((clip_media_id(clip)?, clip_ms(clip.duration))))
        .filter(|(_, ms)| *ms > 0)
        .filter(|(id, _)| seen.insert(id.clone()))
        .collect()
}

/// Where in `item` the breaks go, as media offsets, ascending, each strictly
/// between 0 and the runtime.
///
/// The nominal cuts are the multiples of `every_ms` below the runtime. Each
/// moves to the nearest chapter mark within [`SNAP_MS`], the earlier one on a
/// tie, if that mark is after the previous cut and before the end. A nominal
/// cut at or before the previous cut is dropped.
pub(super) fn cut_points(item: &MediaItem, every_ms: u64) -> Vec<u64> {
    let runtime = item.runtime_ms;
    let mut cuts = Vec::new();
    if every_ms == 0 {
        return cuts;
    }
    let mut previous = 0;
    let mut nominal = every_ms;
    while nominal < runtime {
        let snapped = item
            .chapters
            .iter()
            .map(|chapter| chapter.start_ms)
            .filter(|&mark| mark > previous && mark < runtime && mark.abs_diff(nominal) <= SNAP_MS)
            .min_by_key(|&mark| (mark.abs_diff(nominal), mark));
        let cut = snapped.unwrap_or(nominal);
        if cut > previous {
            cuts.push(cut);
            previous = cut;
        }
        nominal = match nominal.checked_add(every_ms) {
            Some(next) => next,
            None => break,
        };
    }
    cuts
}

/// Fills breaks from one plan and carries the pool's rotation from one break
/// to the next.
pub(super) struct Rotation {
    length_ms: u64,
    clips: Vec<(MediaId, u64)>,
    pad: Vec<(MediaId, u64)>,
    /// Index into `clips` of the last clip placed, and when it started.
    last: Option<(usize, i64)>,
}

impl Rotation {
    /// `last` is the start time and media id of the channel's last break clip
    /// before this run; the rotation continues after it. An id no longer in
    /// the pool starts the rotation at the first clip.
    pub(super) fn new(plan: &BreakPlan<'_>, last: Option<&(i64, MediaId)>) -> Self {
        let clips = usable(plan.clips);
        let last = last.and_then(|(start_ms, id)| {
            let i = clips.iter().position(|(clip, _)| clip == id)?;
            Some((i, *start_ms))
        });
        Self {
            length_ms: plan.length_ms,
            clips,
            pad: usable(plan.pad),
            last,
        }
    }

    /// The pieces of a break that starts at `start_ms`, and whether it came
    /// out shorter than the configured length because nothing could pad it.
    /// An empty list means no break.
    ///
    /// Walks the pool once, starting after the last clip placed, and places
    /// each clip that fits in the time left, so no clip plays twice in one
    /// break and none is cut. If time is left, one filler piece covers it. If
    /// the last clip started more than [`ROTATION_MEMORY_MS`] before
    /// `start_ms`, the walk starts at the first clip.
    pub(super) fn fill(&mut self, start_ms: i64) -> (Vec<Piece>, bool) {
        let mut pieces = Vec::new();
        let mut left = self.length_ms;
        let n = self.clips.len();
        let first = match self.last {
            Some((i, at)) if start_ms.saturating_sub(at) <= ROTATION_MEMORY_MS => i + 1,
            _ => 0,
        };
        let mut at = start_ms;
        for step in 0..n {
            let i = (first + step) % n;
            let (id, ms) = &self.clips[i];
            if *ms <= left {
                left -= ms;
                self.last = Some((i, at));
                at = at.saturating_add(i64::try_from(*ms).unwrap_or(i64::MAX));
                pieces.push(Piece {
                    media_id: id.clone(),
                    length_ms: *ms,
                    media_duration_ms: *ms,
                    kind: EventKind::Break,
                });
            }
        }
        if left == 0 {
            return (pieces, false);
        }
        match self.pad_clip(left) {
            Some((id, ms)) => {
                pieces.push(Piece {
                    media_id: id.clone(),
                    length_ms: left,
                    media_duration_ms: *ms,
                    kind: EventKind::Filler,
                });
                (pieces, false)
            }
            None => (pieces, true),
        }
    }

    /// The shortest filler clip at least `left` long, or the longest one if
    /// none is. The earlier in pool order wins a tie.
    fn pad_clip(&self, left: u64) -> Option<&(MediaId, u64)> {
        let long_enough = self
            .pad
            .iter()
            .filter(|(_, ms)| *ms >= left)
            .min_by_key(|(_, ms)| *ms);
        long_enough.or_else(|| {
            // `max_by_key` keeps the last of equal maxima; reverse for the first.
            self.pad.iter().rev().max_by_key(|(_, ms)| *ms)
        })
    }
}
