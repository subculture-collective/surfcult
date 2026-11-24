//! Commercial breaks: where a program is cut and which clips fill each break.
//! See "Breaks" in the module docs of `schedule` for the rules.

use std::collections::HashSet;
use std::time::Duration;

use super::rng::{self, SplitMix64};
use crate::clips::Clip;
use crate::config::Rotation as Order;
use crate::library::{MediaId, MediaItem};
use crate::timeline::EventKind;

/// How far a cut may move to reach a chapter mark: 60 seconds either way.
pub const SNAP_MS: u64 = 60_000;

/// How long the break rotation remembers its place: 24 hours. A break that
/// starts more than this after the last break clip started begins again at
/// the pool's first clip. The bound lets `extend` read only the last day of
/// events for the last break clip (or longer, back to the pool's
/// `repeat_after`), and because an unbroken run applies the same rule, a
/// resumed run still matches it.
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
    /// The order the break pool's clips are drawn in.
    pub rotation: Order,
    /// The break pool's repeat limit: a clip does not play again until this
    /// long after its last airing started. `None` is no limit.
    pub repeat_after_ms: Option<i64>,
    /// The `breaks.pad` pool, idents and slates, which pads a break the clips
    /// do not fill.
    pub idents: &'a [Clip],
    /// The channel's filler pool, which pads a break when `idents` has no
    /// clip.
    pub filler: &'a [Clip],
}

impl BreakPlan<'_> {
    /// How far before a run the break history must reach for the run to
    /// match an unbroken one: [`ROTATION_MEMORY_MS`] or the repeat limit,
    /// whichever is longer.
    pub(crate) fn memory_ms(&self) -> i64 {
        self.repeat_after_ms
            .map_or(ROTATION_MEMORY_MS, |ms| ms.max(ROTATION_MEMORY_MS))
    }
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
pub(super) fn usable(clips: &[Clip]) -> Vec<(MediaId, u64)> {
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

/// Fills breaks from one plan and carries the pool's rotation and each clip's
/// airings from one break to the next.
#[derive(Clone)]
pub(super) struct Rotation {
    channel: u32,
    length_ms: u64,
    order: Order,
    repeat_after_ms: Option<i64>,
    clips: Vec<(MediaId, u64)>,
    idents: Vec<(MediaId, u64)>,
    filler: Vec<(MediaId, u64)>,
    /// Index into `clips` of the last clip placed, and when it started.
    last: Option<(usize, i64)>,
    /// For each clip, when its airings started, oldest first: the stored
    /// history the run was given, then this run's.
    aired: Vec<Vec<i64>>,
}

impl Rotation {
    /// `last` is the start time and media id of the channel's last break clip
    /// before this run; an `order` rotation continues after it. An id no
    /// longer in the pool starts the rotation at the first clip. `aired` holds
    /// the channel's break clips before this run, newest first, as far back
    /// as [`BreakPlan::memory_ms`]; the repeat limit counts from them.
    pub(super) fn new(
        plan: &BreakPlan<'_>,
        channel: u32,
        last: Option<&(i64, MediaId)>,
        aired: &[(i64, MediaId)],
    ) -> Self {
        let clips = usable(plan.clips);
        let mut history = vec![Vec::new(); clips.len()];
        for (start_ms, id) in aired.iter().rev().chain(last) {
            if let Some(i) = clips.iter().position(|(clip, _)| clip == id) {
                history[i].push(*start_ms);
            }
        }
        for starts in &mut history {
            starts.sort_unstable();
            starts.dedup();
        }
        let mut rotation = Self {
            channel,
            length_ms: plan.length_ms,
            order: plan.rotation,
            repeat_after_ms: plan.repeat_after_ms,
            clips,
            idents: usable(plan.idents),
            filler: usable(plan.filler),
            last: None,
            aired: history,
        };
        rotation.continue_after(last, i64::MAX);
        rotation
    }

    /// Forgets the airings that start at or after `dropped_from`, break clips
    /// cut off by the end of a block that never play, and moves the rotation
    /// back so it continues after `last`, the start time and media id of the
    /// newest break clip that was kept, as `new` does.
    pub(super) fn continue_after(&mut self, last: Option<&(i64, MediaId)>, dropped_from: i64) {
        for starts in &mut self.aired {
            while starts.last().is_some_and(|&at| at >= dropped_from) {
                starts.pop();
            }
        }
        self.last = last.and_then(|(start_ms, id)| {
            let i = self.clips.iter().position(|(clip, _)| clip == id)?;
            Some((i, *start_ms))
        });
    }

    /// The pieces of a break that starts at `start_ms`, and whether it came
    /// out shorter than the configured length because nothing could pad it.
    /// An empty list means no break.
    ///
    /// Walks the pool once and places each clip that fits in the time left
    /// and is not held back by the repeat limit, so no clip plays twice in one
    /// break and none is cut. With `order` the walk starts after the last clip
    /// placed, or at the first clip if that started more than
    /// [`ROTATION_MEMORY_MS`] before `start_ms`. With `shuffle` it follows an
    /// order drawn for this break from the channel and `start_ms`. If time is
    /// left, one pad piece covers it: from the ident pool if it has a clip,
    /// else from the filler pool.
    pub(super) fn fill(&mut self, start_ms: i64) -> (Vec<Piece>, bool) {
        let mut pieces = Vec::new();
        let mut left = self.length_ms;
        let mut at = start_ms;
        for i in self.walk(start_ms) {
            let (id, ms) = &self.clips[i];
            if *ms > left || self.cooling(i, at) {
                continue;
            }
            left -= ms;
            self.last = Some((i, at));
            self.aired[i].push(at);
            at = at.saturating_add(i64::try_from(*ms).unwrap_or(i64::MAX));
            pieces.push(Piece {
                media_id: id.clone(),
                length_ms: *ms,
                media_duration_ms: *ms,
                kind: EventKind::Break,
            });
        }
        if left == 0 {
            return (pieces, false);
        }
        let pad = pad_clip(&self.idents, left)
            .map(|clip| (clip, EventKind::Ident))
            .or_else(|| pad_clip(&self.filler, left).map(|clip| (clip, EventKind::Filler)));
        match pad {
            Some(((id, ms), kind)) => {
                pieces.push(Piece {
                    media_id: id.clone(),
                    length_ms: left,
                    media_duration_ms: *ms,
                    kind,
                });
                (pieces, false)
            }
            None => (pieces, true),
        }
    }

    /// The indexes into `clips` a break that starts at `start_ms` tries, in
    /// order, each once.
    fn walk(&self, start_ms: i64) -> Vec<usize> {
        let n = self.clips.len();
        match self.order {
            Order::Order => {
                let first = match self.last {
                    Some((i, at)) if start_ms.saturating_sub(at) <= ROTATION_MEMORY_MS => i + 1,
                    _ => 0,
                };
                (0..n).map(|step| (first + step) % n).collect()
            }
            Order::Shuffle => {
                // Fisher-Yates, seeded only by the channel and the break's
                // start, so a resumed run draws the same order.
                let mut order: Vec<usize> = (0..n).collect();
                let mut rng = SplitMix64::new(rng::seed(self.channel, start_ms));
                for i in (1..n).rev() {
                    order.swap(i, rng.below(i + 1));
                }
                order
            }
        }
    }

    /// Whether clip `i` would break the repeat limit if it started at `at_ms`.
    fn cooling(&self, i: usize, at_ms: i64) -> bool {
        self.repeat_after_ms.is_some_and(|limit| {
            self.aired[i]
                .last()
                .is_some_and(|&last| at_ms.saturating_sub(last) < limit)
        })
    }
}

/// The shortest clip in `pool` at least `left` long, or the longest one if
/// none is. The earlier in pool order wins a tie.
fn pad_clip(pool: &[(MediaId, u64)], left: u64) -> Option<&(MediaId, u64)> {
    let long_enough = pool
        .iter()
        .filter(|(_, ms)| *ms >= left)
        .min_by_key(|(_, ms)| *ms);
    long_enough.or_else(|| {
        // `max_by_key` keeps the last of equal maxima; reverse for the first.
        pool.iter().rev().max_by_key(|(_, ms)| *ms)
    })
}
