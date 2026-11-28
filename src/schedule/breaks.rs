//! Commercial breaks: where a program is cut and which clips fill each break.
//! See "Breaks" in the module docs of `schedule` for the rules.

use std::collections::{HashMap, HashSet};
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

/// A clip pool as the generator draws from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ClipPool<'a> {
    /// Every use of one name shares one rotation and one repeat limit.
    pub name: &'a str,
    /// The pool's clips, in pool order.
    pub clips: &'a [Clip],
    /// The order the clips are drawn in.
    pub rotation: Order,
    /// A clip does not play again until this long after its last airing
    /// started. `None` is no limit.
    pub repeat_after_ms: Option<i64>,
    /// The kind its clips are scheduled as.
    pub kind: EventKind,
}

/// The clips of one break or one `between`: one clip from `open`, clips
/// drawn from `pools` by weight, one clip from `close`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Interlude<'a> {
    /// Each pool with its weight; a weight of 0 counts as 1.
    pub pools: Vec<(ClipPool<'a>, u32)>,
    pub open: Option<ClipPool<'a>>,
    pub close: Option<ClipPool<'a>>,
}

impl<'a> Interlude<'a> {
    /// One pool with weight 1 and no open or close clip.
    #[cfg(test)]
    pub(crate) fn of(pool: ClipPool<'a>) -> Self {
        Self {
            pools: vec![(pool, 1)],
            open: None,
            close: None,
        }
    }

    /// Every pool it draws from, open and close included.
    pub(super) fn uses(&self) -> impl Iterator<Item = &ClipPool<'a>> {
        self.open
            .iter()
            .chain(self.pools.iter().map(|(pool, _)| pool))
            .chain(&self.close)
    }
}

/// A channel's break settings and the clips they may use.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BreakPlan<'a> {
    /// The interval between cuts, in program time.
    pub every_ms: u64,
    /// How long each break runs.
    pub length_ms: u64,
    /// What fills each break.
    pub fill: Interlude<'a>,
    /// The `breaks.pad` pool, idents and slates, which pads a break the clips
    /// do not fill.
    pub idents: &'a [Clip],
    /// The channel's filler pool, which pads a break when `idents` has no
    /// clip.
    pub filler: &'a [Clip],
}

/// A channel's `between` settings: what plays between two programs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BetweenPlan<'a> {
    /// `None` places one clip; a length is filled and padded like a break.
    pub length_ms: Option<u64>,
    pub fill: Interlude<'a>,
}

/// How far before a run the clip history must reach for the run to match an
/// unbroken one: [`ROTATION_MEMORY_MS`] or the longest repeat limit of the
/// pools the breaks and `between` draw from, whichever is longer.
pub(crate) fn memory_ms(breaks: Option<&BreakPlan<'_>>, between: Option<&BetweenPlan<'_>>) -> i64 {
    breaks
        .map(|plan| &plan.fill)
        .into_iter()
        .chain(between.map(|plan| &plan.fill))
        .flat_map(Interlude::uses)
        .filter_map(|pool| pool.repeat_after_ms)
        .fold(ROTATION_MEMORY_MS, i64::max)
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

/// The salt of a break's draws among its weighted pools, so they do not
/// repeat the shuffle of a pool walked from the same seed.
const DRAW_SALT: u64 = 0x5EED_B7EA_4D3A_0001;

/// An interlude's pools as indexes into [`Rotations`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Drawn {
    pools: Vec<(usize, u64)>,
    open: Option<usize>,
    close: Option<usize>,
}

/// One pool's rotation and its clips' airings.
#[derive(Clone)]
struct PoolState<'a> {
    name: &'a str,
    order: Order,
    repeat_after_ms: Option<i64>,
    kind: EventKind,
    clips: Vec<(MediaId, u64)>,
    /// For each clip, when its airings started, ascending: the stored history
    /// the run was given, then this run's.
    aired: Vec<Vec<i64>>,
}

/// Draws the clips of a channel's breaks and `between` from their pools, and
/// carries each pool's rotation and each clip's airings from one draw to the
/// next. Every clip the channel airs, whatever placed it, is recorded in each
/// pool that holds it, as the stored history the next run reads holds it.
#[derive(Clone)]
pub(super) struct Rotations<'a> {
    channel: u32,
    pools: Vec<PoolState<'a>>,
    /// Where each clip id is: pool and clip indexes.
    by_id: HashMap<MediaId, Vec<(usize, usize)>>,
}

impl<'a> Rotations<'a> {
    /// The rotations of `pools`, the first of each name. `aired` holds the
    /// channel's clips before this run, newest first, as far back as
    /// [`memory_ms`]; each pool's rotation continues after the newest of its
    /// clips there, and the repeat limits count from them.
    pub(super) fn new(
        channel: u32,
        pools: impl IntoIterator<Item = ClipPool<'a>>,
        aired: &[(i64, MediaId)],
    ) -> Self {
        let mut rotations = Self {
            channel,
            pools: Vec::new(),
            by_id: HashMap::new(),
        };
        for pool in pools {
            if rotations.pools.iter().any(|state| state.name == pool.name) {
                continue;
            }
            let clips = usable(pool.clips);
            let p = rotations.pools.len();
            for (c, (id, _)) in clips.iter().enumerate() {
                rotations.by_id.entry(id.clone()).or_default().push((p, c));
            }
            rotations.pools.push(PoolState {
                name: pool.name,
                order: pool.rotation,
                repeat_after_ms: pool.repeat_after_ms,
                kind: pool.kind,
                aired: vec![Vec::new(); clips.len()],
                clips,
            });
        }
        for (start_ms, id) in aired.iter().rev() {
            rotations.record(id, *start_ms);
        }
        rotations
    }

    /// `fill`'s pools as indexes. Every pool in it was given to `new`.
    pub(super) fn drawn(&self, fill: &Interlude<'_>) -> Drawn {
        let index = |pool: &ClipPool<'_>| {
            self.pools
                .iter()
                .position(|state| state.name == pool.name)
                .expect("the pool was given to Rotations::new")
        };
        Drawn {
            pools: fill
                .pools
                .iter()
                .map(|(pool, weight)| (index(pool), u64::from((*weight).max(1))))
                .collect(),
            open: fill.open.as_ref().map(index),
            close: fill.close.as_ref().map(index),
        }
    }

    /// Records that clip `id` started airing at `at_ms`, in every pool that
    /// holds it.
    pub(super) fn record(&mut self, id: &MediaId, at_ms: i64) {
        let Some(places) = self.by_id.get(id) else {
            return;
        };
        for &(p, c) in places {
            let starts = &mut self.pools[p].aired[c];
            let i = starts.partition_point(|&at| at < at_ms);
            if starts.get(i) != Some(&at_ms) {
                starts.insert(i, at_ms);
            }
        }
    }

    /// Forgets the airings that start at or after `dropped_from`: clips cut
    /// off by the end of a block, which never play. Each rotation then
    /// continues after the newest clip that was kept.
    pub(super) fn forget_from(&mut self, dropped_from: i64) {
        for starts in self.pools.iter_mut().flat_map(|pool| &mut pool.aired) {
            while starts.last().is_some_and(|&at| at >= dropped_from) {
                starts.pop();
            }
        }
    }

    /// The pieces of a break or `between` that starts at `start_ms`, and
    /// whether it came out shorter than `length_ms` because nothing could pad
    /// it. An empty list means nothing plays.
    ///
    /// One clip from `open` comes first, then clips drawn from the weighted
    /// pools, then one pad piece for the time they leave, from `idents` if it
    /// has a clip, else from `filler`, then one clip from `close`. The
    /// closing clip is chosen when the interlude starts, so its length is
    /// kept free. With no `length_ms` the pools give one clip and nothing is
    /// padded. No clip plays twice in one interlude, none is cut, and none
    /// starts while `repeat_after` holds it back.
    pub(super) fn interlude(
        &mut self,
        drawn: &Drawn,
        start_ms: i64,
        length_ms: Option<u64>,
        idents: &[(MediaId, u64)],
        filler: &[(MediaId, u64)],
    ) -> (Vec<Piece>, bool) {
        let mut used = HashSet::new();
        let mut pieces = Vec::new();
        let mut at = start_ms;
        let mut left = length_ms;
        if let Some(p) = drawn.open
            && let Some(c) = self.pick(p, start_ms, at, left, &used)
        {
            let piece = self.place(p, c, &mut at, &mut used);
            left = left.map(|left| left - piece.length_ms);
            pieces.push(piece);
        }
        let close = drawn.close.and_then(|p| {
            let c = self.pick(p, start_ms, start_ms, left, &used)?;
            let (id, ms) = &self.pools[p].clips[c];
            used.insert(id.clone());
            left = left.map(|left| left - ms);
            Some((p, c))
        });
        let (drawn_pieces, rest) = self.draw(&drawn.pools, at, left, &mut used);
        for piece in drawn_pieces {
            at = after(at, piece.length_ms);
            pieces.push(piece);
        }
        let mut short = false;
        if let Some(rest) = rest.filter(|rest| *rest > 0) {
            match pad(idents, filler, rest) {
                Some(piece) => {
                    self.record(&piece.media_id, at);
                    at = after(at, rest);
                    pieces.push(piece);
                }
                None => short = true,
            }
        }
        if let Some((p, c)) = close {
            pieces.push(self.place(p, c, &mut at, &mut used));
        }
        (pieces, short)
    }

    /// Clips drawn from `pools` from `start_ms`, until no pool has a clip
    /// that fits in `left`, or one clip when `left` is `None`, and the time
    /// they leave.
    ///
    /// Each draw picks a pool with probability proportional to its weight,
    /// from SplitMix64 seeded with the channel and `start_ms`, and takes the
    /// next clip of that pool's walk that fits, is not held back by the
    /// repeat limit and has not played in this interlude. A pool whose walk
    /// has no such clip left is out of the draws. Each pool is walked once,
    /// so with one pool this places each clip that fits, in rotation order.
    fn draw(
        &mut self,
        pools: &[(usize, u64)],
        start_ms: i64,
        mut left: Option<u64>,
        used: &mut HashSet<MediaId>,
    ) -> (Vec<Piece>, Option<u64>) {
        let mut pieces = Vec::new();
        let mut at = start_ms;
        let mut walks: Vec<Option<std::vec::IntoIter<usize>>> = vec![None; pools.len()];
        let mut live: Vec<usize> = (0..pools.len()).collect();
        let mut rng = SplitMix64::new(rng::seed(self.channel, start_ms) ^ DRAW_SALT);
        while !live.is_empty() {
            let k = if live.len() == 1 {
                0
            } else {
                let total: u64 = live.iter().map(|&e| pools[e].1).sum();
                let mut draw = rng.below_u64(total);
                live.iter()
                    .position(|&e| {
                        let hit = draw < pools[e].1;
                        draw = draw.saturating_sub(pools[e].1);
                        hit
                    })
                    .unwrap_or(0)
            };
            let (p, _) = pools[live[k]];
            let walk = walks[live[k]].get_or_insert_with(|| self.walk(p, start_ms).into_iter());
            let pool = &self.pools[p];
            let found = walk.find(|&c| {
                let (id, ms) = &pool.clips[c];
                left.is_none_or(|left| *ms <= left) && !self.cooling(p, c, at) && !used.contains(id)
            });
            match found {
                Some(c) => {
                    let piece = self.place(p, c, &mut at, used);
                    left = left.map(|left| left - piece.length_ms);
                    pieces.push(piece);
                    if left.is_none() {
                        break;
                    }
                }
                None => {
                    live.remove(k);
                }
            }
        }
        (pieces, left)
    }

    /// The first clip of pool `p`'s walk from `walk_ms` that fits in `left`,
    /// is not held back at `at_ms` and has not played in this interlude.
    fn pick(
        &self,
        p: usize,
        walk_ms: i64,
        at_ms: i64,
        left: Option<u64>,
        used: &HashSet<MediaId>,
    ) -> Option<usize> {
        let pool = &self.pools[p];
        self.walk(p, walk_ms).into_iter().find(|&c| {
            let (id, ms) = &pool.clips[c];
            left.is_none_or(|left| *ms <= left) && !self.cooling(p, c, at_ms) && !used.contains(id)
        })
    }

    /// Places clip `c` of pool `p` at `*at`, records it and moves `*at` past
    /// it.
    fn place(&mut self, p: usize, c: usize, at: &mut i64, used: &mut HashSet<MediaId>) -> Piece {
        let (id, ms) = self.pools[p].clips[c].clone();
        let kind = self.pools[p].kind;
        self.record(&id, *at);
        *at = after(*at, ms);
        used.insert(id.clone());
        Piece {
            media_id: id,
            length_ms: ms,
            media_duration_ms: ms,
            kind,
        }
    }

    /// The newest airing of pool `p`'s clips: which clip, and when.
    fn last(&self, p: usize) -> Option<(usize, i64)> {
        self.pools[p]
            .aired
            .iter()
            .enumerate()
            .filter_map(|(c, starts)| Some((c, *starts.last()?)))
            .max_by_key(|&(_, at)| at)
    }

    /// The indexes into pool `p`'s clips a draw that starts at `start_ms`
    /// tries, in order, each once.
    ///
    /// With `order` the walk starts after the pool's newest airing, or at the
    /// first clip if that started more than [`ROTATION_MEMORY_MS`] before
    /// `start_ms`. With `shuffle` it follows an order drawn from the channel
    /// and `start_ms`.
    fn walk(&self, p: usize, start_ms: i64) -> Vec<usize> {
        let n = self.pools[p].clips.len();
        match self.pools[p].order {
            Order::Order => {
                let first = match self.last(p) {
                    Some((c, at)) if start_ms.saturating_sub(at) <= ROTATION_MEMORY_MS => c + 1,
                    _ => 0,
                };
                (0..n).map(|step| (first + step) % n).collect()
            }
            Order::Shuffle => {
                // Fisher-Yates, seeded only by the channel and the draw's
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

    /// Whether clip `c` of pool `p` would break the repeat limit if it
    /// started at `at_ms`.
    fn cooling(&self, p: usize, c: usize, at_ms: i64) -> bool {
        let pool = &self.pools[p];
        pool.repeat_after_ms.is_some_and(|limit| {
            pool.aired[c]
                .last()
                .is_some_and(|&last| at_ms.saturating_sub(last) < limit)
        })
    }
}

/// `at_ms` plus `ms`.
fn after(at_ms: i64, ms: u64) -> i64 {
    at_ms.saturating_add(i64::try_from(ms).unwrap_or(i64::MAX))
}

/// One piece exactly `left` long from the shortest ident at least that long
/// (the longest if none is), else the same from the filler pool.
fn pad(idents: &[(MediaId, u64)], filler: &[(MediaId, u64)], left: u64) -> Option<Piece> {
    let ((id, ms), kind) = pad_clip(idents, left)
        .map(|clip| (clip, EventKind::Ident))
        .or_else(|| pad_clip(filler, left).map(|clip| (clip, EventKind::Filler)))?;
    Some(Piece {
        media_id: id.clone(),
        length_ms: left,
        media_duration_ms: *ms,
        kind,
    })
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

/// The pieces that fill a grid gap of `gap_ms` from `start_ms`: whole clips
/// drawn from the break pools (when the channel has breaks), then whole
/// idents, then whole filler clips, each pool walked once in order and a clip
/// placed only if it fits in the time left. Time still left is one piece, as
/// long as that time, from the shortest ident at least that long (the longest
/// if none is), else the same from the filler pool: a single clip repeated as
/// a last resort. With no clip at all the list is empty. Every piece is
/// recorded in `rotations`.
pub(super) fn fill_gap(
    rotations: &mut Rotations<'_>,
    breaks: Option<&Drawn>,
    idents: &[(MediaId, u64)],
    filler: &[(MediaId, u64)],
    start_ms: i64,
    gap_ms: u64,
) -> Vec<Piece> {
    let (mut pieces, left) = match breaks {
        Some(drawn) => rotations.draw(&drawn.pools, start_ms, Some(gap_ms), &mut HashSet::new()),
        None => (Vec::new(), Some(gap_ms)),
    };
    let mut left = left.unwrap_or(0);
    let mut at = after(start_ms, gap_ms - left);
    for (pool, kind) in [(idents, EventKind::Ident), (filler, EventKind::Filler)] {
        for (id, ms) in pool {
            if *ms <= left {
                left -= ms;
                rotations.record(id, at);
                at = after(at, *ms);
                pieces.push(Piece {
                    media_id: id.clone(),
                    length_ms: *ms,
                    media_duration_ms: *ms,
                    kind,
                });
            }
        }
    }
    if left > 0
        && let Some(piece) = pad(idents, filler, left)
    {
        rotations.record(&piece.media_id, at);
        pieces.push(piece);
    }
    pieces
}
