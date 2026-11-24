//! The pure generator. See the module docs of `schedule` for the rules.

use std::collections::{HashMap, HashSet};

use super::blocks::Span;
use super::breaks::{BreakPlan, Rotation, cut_points, usable};
use super::rng::{self, SplitMix64};
use super::sign_off::Night;
use super::{Generated, MAX_EVENTS, Note, ScheduleError};
use crate::clips::Clip;
use crate::config::{Mode, Source};
use crate::library::{MediaId, MediaItem};
use crate::timeline::{Event, EventKind};

/// Who the schedule is for, how it is ordered and where its breaks go.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Params<'a> {
    pub channel: u32,
    pub mode: Mode,
    /// `None` schedules each program whole, with no breaks.
    pub breaks: Option<BreakPlan<'a>>,
}

/// One source's program pool and its weight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SourcePool<'a> {
    /// Named in [`Note::SourceNotPlayable`].
    pub source: &'a Source,
    /// Used by `weighted`; other modes ignore it. A weight of 0 counts as 1.
    pub weight: u32,
    pub items: &'a [MediaItem],
}

/// The program pools played in one mode: a block or the default of a blocks
/// channel, or a whole channel with one lineup. A `weighted` lane picks among
/// its sources; any other mode plays its first source (the config allows
/// only one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Lane<'a> {
    pub mode: Mode,
    pub sources: Vec<SourcePool<'a>>,
}

impl<'a> Lane<'a> {
    /// The items a sequential or shuffle lane draws from, every source's in
    /// order.
    pub(crate) fn items(&self) -> impl Iterator<Item = &'a MediaItem> {
        self.sources.iter().flat_map(|entry| entry.items)
    }
}

/// A blocks channel's inputs to [`generate_blocks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Layout<'a> {
    pub channel: u32,
    pub breaks: Option<BreakPlan<'a>>,
    /// One lane per block, in config order, then the default's lane if there
    /// is one. A span's `block` indexes this list.
    pub lanes: Vec<Lane<'a>>,
    /// The index in `lanes` of the default, which plays time no block covers.
    pub default: Option<usize>,
    /// The channel's time, from [`spans`](super::blocks::spans).
    pub spans: Vec<Span>,
    /// The channel's `filler` pool, which plays time with nothing else to
    /// play.
    pub filler: &'a [Clip],
    /// The nights the channel is off the air, from
    /// [`nights`](super::sign_off::nights), in time order. Empty without a
    /// sign-off.
    pub nights: Vec<Night>,
    /// The id and length of the clip that starts each night.
    pub sign_off: Option<(MediaId, u64)>,
}

/// Where the previous run stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resume {
    /// Media ids of the programs scheduled before this run, newest first. The
    /// first is the cursor, the last item scheduled. A sequential channel
    /// reads only the cursor; shuffle reads as many as it needs to find the
    /// items cooling down; a blocks channel's lanes each look for their own
    /// items.
    pub recent: Vec<MediaId>,
    /// The start time and media id of the last break clip scheduled before
    /// this run. The break pool's rotation continues after it unless the next
    /// break starts more than [`ROTATION_MEMORY_MS`](super::ROTATION_MEMORY_MS) later.
    pub last_break: Option<(i64, MediaId)>,
    /// The start times and media ids of the break clips scheduled before
    /// this run, newest first, back to [`BreakPlan::memory_ms`]: what the
    /// break pool's `repeat_after` counts from.
    pub aired: Vec<(i64, MediaId)>,
    /// The end time and kind of the channel's last event before this run.
    /// Only a run with nights off the air reads it, to tell a run that starts
    /// in the gap before a sign-off from one that starts after a program.
    pub last_event: Option<(i64, EventKind)>,
}

impl Resume {
    fn cursor(&self) -> Option<&MediaId> {
        self.recent.first()
    }
}

/// Generates events covering `[start_ms, end_ms)` for a channel with one
/// mode over `sources`. The last event may end after `end_ms`.
/// `start_ms >= end_ms` gives no events.
pub(crate) fn generate(
    params: &Params<'_>,
    sources: &[SourcePool<'_>],
    resume: &Resume,
    start_ms: i64,
    end_ms: i64,
) -> Result<Generated, ScheduleError> {
    let layout = Layout {
        channel: params.channel,
        breaks: params.breaks,
        lanes: vec![Lane {
            mode: params.mode,
            sources: sources.to_vec(),
        }],
        default: None,
        spans: vec![Span {
            start_ms,
            end_ms: i64::MAX,
            block: Some(0),
        }],
        filler: &[],
        nights: Vec::new(),
        sign_off: None,
    };
    run(&layout, true, resume, start_ms, end_ms)
}

/// Like [`generate`], for a channel with one lineup and a sign-off:
/// `layout` has one lane and one span over all time, the channel's filler
/// clips for the gap before each sign-off, and its nights off the air.
pub(crate) fn generate_fixed(
    layout: &Layout<'_>,
    resume: &Resume,
    start_ms: i64,
    end_ms: i64,
) -> Result<Generated, ScheduleError> {
    run(layout, true, resume, start_ms, end_ms)
}

/// Generates events covering `[start_ms, end_ms)` for a blocks channel. A
/// program still on at the end of its span is cut there; the last event may
/// end after `end_ms`. Time with nothing to play has no events.
pub(crate) fn generate_blocks(
    layout: &Layout<'_>,
    resume: &Resume,
    start_ms: i64,
    end_ms: i64,
) -> Result<Generated, ScheduleError> {
    run(layout, false, resume, start_ms, end_ms)
}

/// The shared run. `fixed` is a channel with one lane over all time: its
/// cursor is the last program, and a pool with nothing playable gives no
/// events at all.
fn run(
    layout: &Layout<'_>,
    fixed: bool,
    resume: &Resume,
    start_ms: i64,
    end_ms: i64,
) -> Result<Generated, ScheduleError> {
    let mut notes = Vec::new();
    let mut pickers: Vec<Option<Picker<'_>>> = layout
        .lanes
        .iter()
        .map(|lane| Picker::new(lane, layout.channel, resume, fixed, &mut notes))
        .collect();
    if fixed && pickers.iter().all(Option::is_none) {
        return Ok(nothing(resume, notes));
    }
    // Which lane plays each span: its block, else the default, else none.
    let playable = |lane: &usize| pickers.get(*lane).is_some_and(Option::is_some);
    let span_lanes: Vec<Option<usize>> = layout
        .spans
        .iter()
        .map(|span| {
            span.block
                .filter(playable)
                .or(layout.default.filter(playable))
        })
        .collect();
    let stretches = stretches(&layout.spans, &span_lanes, &layout.nights);
    let filler = usable(layout.filler);

    let mut rotation = layout.breaks.as_ref().map(|plan| {
        (
            plan.every_ms,
            Rotation::new(
                plan,
                layout.channel,
                resume.last_break.as_ref(),
                &resume.aired,
            ),
        )
    });
    let mut last_break = resume.last_break.clone();
    let mut short_breaks = 0;
    let mut dead_air_ms: u64 = 0;
    let mut signed_off_without_clip = false;

    let mut out = Out {
        events: Vec::new(),
        at_ms: start_ms,
    };
    let mut cursor = resume.cursor().cloned();
    for stretch in &stretches {
        let span = &stretch.span;
        if out.at_ms >= end_ms {
            break;
        }
        let limit = span.end_ms.min(end_ms);
        if out.at_ms >= limit {
            continue;
        }
        if stretch.off_air {
            match &layout.sign_off {
                Some((id, ms)) => {
                    let into = out.at_ms.abs_diff(span.start_ms);
                    if into < *ms {
                        out.push_until(span.end_ms, id, into, ms - into, *ms, EventKind::Ident)?;
                    }
                }
                None => signed_off_without_clip = true,
            }
            out.at_ms = out.at_ms.max(span.end_ms);
            continue;
        }
        let Some(lane) = stretch.lane else {
            if filler.is_empty() {
                dead_air_ms += out.at_ms.abs_diff(limit);
                out.at_ms = span.end_ms;
            } else {
                fill_with_filler(&mut out, &filler, span.start_ms, span.end_ms, limit)?;
            }
            continue;
        };
        // Before a sign-off no program starts that would still be on when it
        // starts: each program is written whole and taken back if it ends
        // later, and the rest of the stretch is a gap. A run that starts
        // inside the stretch after filler starts in that gap. A run that
        // starts after a program that ended earlier in the stretch, such as
        // one from the freeze point in dead air or one after an outage, first
        // tries the next program from where that program ended: it is in the
        // gap only if the gap began there. A run that starts inside the
        // stretch with no stored event ending in it tries the first program
        // from the start of the stretch the same way.
        let hard_stop = stretch.before_sign_off;
        let cut_at = if hard_stop { i64::MAX } else { span.end_ms };
        let mut in_gap = false;
        let mut probe_for: Option<i64> = None;
        if hard_stop && out.at_ms > span.start_ms {
            match resume.last_event {
                Some((ended_ms, kind)) if ended_ms > span.start_ms => {
                    if kind != EventKind::Program {
                        in_gap = true;
                    } else if ended_ms < out.at_ms {
                        probe_for = Some(out.at_ms);
                        out.at_ms = ended_ms;
                    }
                }
                // No stored event ends inside the stretch: the gap began at
                // its start if the first program there would not fit.
                _ => {
                    probe_for = Some(out.at_ms);
                    out.at_ms = span.start_ms;
                }
            }
        }
        while !in_gap && out.at_ms < limit {
            let saved = hard_stop.then(|| {
                (
                    pickers[lane].clone(),
                    rotation.clone(),
                    last_break.clone(),
                    short_breaks,
                    out.events.len(),
                    out.at_ms,
                )
            });
            let picker = pickers[lane].as_mut().expect("the lane is playable");
            let item = picker.next(out.at_ms);
            let runtime = item.runtime_ms;
            let mut dropped_break = false;
            let mut segment_from = 0;
            if let Some((every_ms, rotation)) = rotation.as_mut() {
                for cut in cut_points(item, *every_ms) {
                    // A break that would start at or after the end of the
                    // block never plays, so it is not filled and not counted
                    // as short.
                    let Some(starts_at) = i64::try_from(cut - segment_from)
                        .ok()
                        .and_then(|ms| out.at_ms.checked_add(ms))
                        .filter(|at| *at < cut_at)
                    else {
                        break;
                    };
                    let (pieces, short) = rotation.fill(starts_at);
                    short_breaks += usize::from(short);
                    // A break with nothing to show is left out and the program
                    // runs on.
                    if pieces.is_empty() {
                        continue;
                    }
                    out.push_until(
                        cut_at,
                        &item.id,
                        segment_from,
                        cut - segment_from,
                        runtime,
                        EventKind::Program,
                    )?;
                    for piece in pieces {
                        let piece_at = out.at_ms;
                        let placed = out.push_until(
                            cut_at,
                            &piece.media_id,
                            0,
                            piece.length_ms,
                            piece.media_duration_ms,
                            piece.kind,
                        )?;
                        if piece.kind == EventKind::Break {
                            if placed {
                                last_break = Some((piece_at, piece.media_id));
                            } else {
                                dropped_break = true;
                            }
                        }
                    }
                    segment_from = cut;
                }
            }
            out.push_until(
                cut_at,
                &item.id,
                segment_from,
                runtime - segment_from,
                runtime,
                EventKind::Program,
            )?;
            if let Some((picker, saved_rotation, saved_break, saved_short, len, at_ms)) = saved
                && (out.at_ms > span.end_ms || probe_for.is_some())
            {
                let overran = out.at_ms > span.end_ms;
                pickers[lane] = picker;
                rotation = saved_rotation;
                last_break = saved_break;
                short_breaks = saved_short;
                out.events.truncate(len);
                out.at_ms = probe_for.take().unwrap_or(at_ms);
                if overran {
                    in_gap = true;
                    break;
                }
                continue;
            }
            // Break clips cut off by the end of a block never play, so the
            // rotation continues after the last one that does and the repeat
            // limit forgets them.
            if dropped_break && let Some((_, rotation)) = rotation.as_mut() {
                rotation.continue_after(last_break.as_ref(), cut_at);
            }
            for picker in pickers.iter_mut().flatten() {
                picker.record(&item.id);
            }
            cursor = Some(item.id.clone());
        }
        // The gap is filled like uncovered time, with the filler cycle
        // counted back from the sign-off so its last clip ends as the
        // sign-off starts.
        if in_gap {
            if filler.is_empty() {
                dead_air_ms += out.at_ms.abs_diff(limit);
                out.at_ms = span.end_ms;
            } else {
                fill_with_filler(&mut out, &filler, span.end_ms, span.end_ms, limit)?;
            }
        }
    }
    if signed_off_without_clip {
        notes.push(Note::NoSignOffClip);
    }
    if short_breaks > 0 {
        notes.push(Note::ShortBreaks {
            count: short_breaks,
        });
    }
    if dead_air_ms > 0 {
        notes.push(Note::DeadAir { ms: dead_air_ms });
    }
    let events = out.events;
    Ok(Generated {
        events,
        cursor,
        notes,
    })
}

/// Fills from `out.at_ms` up to `limit` with the filler pool's clips, cut at
/// `cut_at`. The clips play in pool order, over and over, in cycles that
/// start at `anchor_ms` and every whole cycle before or after it, so where a
/// run resumes does not change what plays. `clips` is not empty.
fn fill_with_filler(
    out: &mut Out,
    clips: &[(MediaId, u64)],
    anchor_ms: i64,
    cut_at: i64,
    limit: i64,
) -> Result<(), ScheduleError> {
    let cycle = clips
        .iter()
        .fold(0u64, |sum, (_, ms)| sum.saturating_add(*ms));
    let since = i128::from(out.at_ms) - i128::from(anchor_ms);
    let mut offset = u64::try_from(since.rem_euclid(i128::from(cycle)))
        .map_err(|_| ScheduleError::TimeOutOfRange)?;
    let mut i = 0;
    while offset >= clips[i].1 {
        offset -= clips[i].1;
        i += 1;
    }
    while out.at_ms < limit {
        let (id, ms) = &clips[i];
        out.push_until(cut_at, id, offset, ms - offset, *ms, EventKind::Filler)?;
        offset = 0;
        i = (i + 1) % clips.len();
    }
    Ok(())
}

/// A piece of the channel's time as `run` walks it: a span, the part of a
/// span outside the nights off the air, or one such night.
struct Stretch {
    span: Span,
    /// The lane that plays it; `None` for uncovered and off-air time.
    lane: Option<usize>,
    /// It ends where a sign-off starts.
    before_sign_off: bool,
    off_air: bool,
}

/// Cuts each night out of `spans` and puts it in their place, in time order.
/// `lanes` holds each span's lane.
fn stretches(spans: &[Span], lanes: &[Option<usize>], nights: &[Night]) -> Vec<Stretch> {
    let mut stretches = Vec::new();
    let mut nights = nights.iter().peekable();
    for (span, &lane) in spans.iter().zip(lanes) {
        let mut from = span.start_ms;
        while from < span.end_ms {
            while nights.next_if(|night| night.end_ms <= from).is_some() {}
            let end = match nights.peek() {
                Some(night) if night.start_ms <= from => {
                    // A night already pushed when it began in an earlier
                    // span is only skipped past here.
                    if stretches.last().is_none_or(|last: &Stretch| {
                        !last.off_air || last.span.end_ms != night.end_ms
                    }) {
                        stretches.push(Stretch {
                            span: Span {
                                start_ms: night.start_ms,
                                end_ms: night.end_ms,
                                block: None,
                            },
                            lane: None,
                            before_sign_off: false,
                            off_air: true,
                        });
                    }
                    from = night.end_ms;
                    continue;
                }
                Some(night) => night.start_ms.min(span.end_ms),
                None => span.end_ms,
            };
            stretches.push(Stretch {
                span: Span {
                    start_ms: from,
                    end_ms: end,
                    block: span.block,
                },
                lane,
                before_sign_off: nights.peek().is_some_and(|night| night.start_ms == end),
                off_air: false,
            });
            from = end;
        }
    }
    stretches
}

/// The events of a run, appended back to back from `at_ms`.
struct Out {
    events: Vec<Event>,
    /// Where the next event starts.
    at_ms: i64,
}

impl Out {
    fn push(
        &mut self,
        media_id: &MediaId,
        media_offset_ms: u64,
        length_ms: u64,
        media_duration_ms: u64,
        kind: EventKind,
    ) -> Result<(), ScheduleError> {
        if self.events.len() >= MAX_EVENTS {
            return Err(ScheduleError::TooManyEvents);
        }
        let length = i64::try_from(length_ms).map_err(|_| ScheduleError::TimeOutOfRange)?;
        let end_ms = self
            .at_ms
            .checked_add(length)
            .ok_or(ScheduleError::TimeOutOfRange)?;
        self.events.push(Event {
            start_ms: self.at_ms,
            end_ms,
            media_id: media_id.clone(),
            media_offset_ms,
            media_duration_ms,
            kind,
        });
        self.at_ms = end_ms;
        Ok(())
    }

    /// Like `push`, but the event is cut to end by `cut_at`. Once `at_ms` has
    /// reached `cut_at` nothing is pushed and it returns false.
    fn push_until(
        &mut self,
        cut_at: i64,
        media_id: &MediaId,
        media_offset_ms: u64,
        length_ms: u64,
        media_duration_ms: u64,
        kind: EventKind,
    ) -> Result<bool, ScheduleError> {
        if self.at_ms >= cut_at {
            return Ok(false);
        }
        let room = cut_at.abs_diff(self.at_ms);
        self.push(
            media_id,
            media_offset_ms,
            length_ms.min(room),
            media_duration_ms,
            kind,
        )?;
        Ok(true)
    }
}

/// How many distinct items a shuffle over `n` playable items keeps cooling
/// down: N - k with k = max(1, ceil(N / 4)).
fn cooldown(n: usize) -> usize {
    n.saturating_sub(n.div_ceil(4).max(1))
}

/// The ids a shuffle over `items` counts toward its cooldown, its distinct
/// playable items, and how many of them it keeps cooling down.
pub(crate) fn cooling_target<'a>(
    items: impl Iterator<Item = &'a MediaItem>,
) -> (HashSet<&'a str>, usize) {
    let mut seen = HashSet::new();
    let playable: HashSet<&str> = items
        .filter(|item| seen.insert(item.id.as_str()))
        .filter(|item| item.runtime_ms > 0)
        .map(|item| item.id.as_str())
        .collect();
    let count = cooldown(playable.len());
    (playable, count)
}

fn nothing(resume: &Resume, notes: Vec<Note>) -> Generated {
    Generated {
        events: Vec::new(),
        cursor: resume.cursor().cloned(),
        notes,
    }
}

/// The distinct items of `items`, first of each id, in order.
fn distinct<'a>(items: impl Iterator<Item = &'a MediaItem>) -> Vec<&'a MediaItem> {
    let mut seen = HashSet::new();
    items.filter(|item| seen.insert(item.id.as_str())).collect()
}

/// One pool's shuffle: what it picks from and what is cooling down.
#[derive(Clone)]
struct Cooldown<'a> {
    playable: Vec<&'a MediaItem>,
    position: HashMap<&'a str, usize>,
    /// Indexes into `playable` of the programs before this slot, oldest
    /// first: the resumed history that is still in the pool, then this
    /// run's programs from the pool.
    played: Vec<usize>,
    /// How many distinct items are cooling down: N - k.
    cooldown: usize,
}

impl<'a> Cooldown<'a> {
    /// `items` are distinct; the ones with no runtime are left out.
    fn new(items: Vec<&'a MediaItem>, resume: &Resume) -> Self {
        let playable: Vec<&MediaItem> = items.into_iter().filter(|i| i.runtime_ms > 0).collect();
        let position: HashMap<&str, usize> = playable
            .iter()
            .enumerate()
            .map(|(i, item)| (item.id.as_str(), i))
            .collect();
        // An id that is no longer in the pool is ignored.
        let played = resume
            .recent
            .iter()
            .rev()
            .filter_map(|id| position.get(id.as_str()).copied())
            .collect();
        Self {
            cooldown: cooldown(playable.len()),
            playable,
            position,
            played,
        }
    }

    /// Picks one of the items that are not cooling down, with one draw from
    /// `rng`.
    fn pick(&self, rng: &mut SplitMix64) -> &'a MediaItem {
        // Walk back to the `cooldown` most recent distinct items. A program
        // cut into several events appears once in the walk.
        let mut cooling = HashSet::new();
        for &i in self.played.iter().rev() {
            if cooling.len() == self.cooldown {
                break;
            }
            cooling.insert(i);
        }
        let eligible: Vec<usize> = (0..self.playable.len())
            .filter(|i| !cooling.contains(i))
            .collect();
        self.playable[eligible[rng.below(eligible.len())]]
    }

    fn record(&mut self, id: &MediaId) {
        if let Some(&i) = self.position.get(id.as_str()) {
            self.played.push(i);
        }
    }
}

/// Chooses the next item to play from one lane. At least one item has a
/// runtime above 0.
#[derive(Clone)]
enum Picker<'a> {
    Sequential {
        /// Every distinct pool item, zero runtimes included, because the cursor
        /// is a position in this list.
        items: Vec<&'a MediaItem>,
        position: HashMap<&'a str, usize>,
        next: usize,
    },
    Shuffle {
        pool: Cooldown<'a>,
        channel: u32,
    },
    Weighted {
        /// The sources with something playable, each with its own shuffle.
        pools: Vec<Cooldown<'a>>,
        /// Running totals of the sources' weights: entry i is the sum of the
        /// weights of sources 0 to i.
        cumulative: Vec<u64>,
        channel: u32,
    },
}

impl<'a> Picker<'a> {
    /// `None` when the lane has nothing playable, which it notes.
    fn new(
        lane: &Lane<'a>,
        channel: u32,
        resume: &Resume,
        fixed: bool,
        notes: &mut Vec<Note>,
    ) -> Option<Self> {
        if lane.mode == Mode::Weighted {
            return Self::weighted(lane, channel, resume, notes);
        }
        let unique = distinct(lane.items());
        let skipped = unique.iter().filter(|item| item.runtime_ms == 0).count();
        if skipped > 0 {
            notes.push(Note::SkippedZeroRuntime { count: skipped });
        }
        if skipped == unique.len() {
            notes.push(Note::NoPlayableItems);
            return None;
        }
        Some(match lane.mode {
            Mode::Shuffle => Self::Shuffle {
                pool: Cooldown::new(unique, resume),
                channel,
            },
            _ => Self::sequential(unique, resume, fixed, notes),
        })
    }

    /// A fixed channel continues after its cursor, the last program, and notes
    /// a cursor that is not in the pool. A block continues after the newest
    /// program in the history that is in its pool.
    fn sequential(
        items: Vec<&'a MediaItem>,
        resume: &Resume,
        fixed: bool,
        notes: &mut Vec<Note>,
    ) -> Self {
        let position: HashMap<&str, usize> = items
            .iter()
            .enumerate()
            .map(|(i, item)| (item.id.as_str(), i))
            .collect();
        let at = |id: &MediaId| position.get(id.as_str()).copied();
        let last = if fixed {
            let last = resume.cursor().and_then(at);
            if let (Some(cursor), None) = (resume.cursor(), last) {
                notes.push(Note::CursorNotInPool {
                    media_id: cursor.clone(),
                });
            }
            last
        } else {
            resume.recent.iter().find_map(at)
        };
        Self::Sequential {
            next: last.map_or(0, |p| (p + 1) % items.len()),
            items,
            position,
        }
    }

    /// Each source with something playable gets its own shuffle over its own
    /// pool; the others are noted and left out of the draw. `None` when none
    /// is playable.
    fn weighted(
        lane: &Lane<'a>,
        channel: u32,
        resume: &Resume,
        notes: &mut Vec<Note>,
    ) -> Option<Self> {
        let mut pools = Vec::new();
        let mut cumulative = Vec::new();
        let mut total = 0u64;
        for entry in &lane.sources {
            let unique = distinct(entry.items.iter());
            let skipped = unique.iter().filter(|item| item.runtime_ms == 0).count();
            if skipped > 0 {
                notes.push(Note::SkippedZeroRuntime { count: skipped });
            }
            if skipped == unique.len() {
                notes.push(Note::SourceNotPlayable {
                    source: entry.source.clone(),
                });
                continue;
            }
            total += u64::from(entry.weight.max(1));
            cumulative.push(total);
            pools.push(Cooldown::new(unique, resume));
        }
        if pools.is_empty() {
            notes.push(Note::NoPlayableItems);
            return None;
        }
        Some(Self::Weighted {
            pools,
            cumulative,
            channel,
        })
    }

    fn next(&mut self, at_ms: i64) -> &'a MediaItem {
        match self {
            Self::Sequential { items, next, .. } => loop {
                let item = items[*next % items.len()];
                *next = (*next + 1) % items.len();
                if item.runtime_ms > 0 {
                    return item;
                }
            },
            Self::Shuffle { pool, channel } => {
                let mut rng = SplitMix64::new(rng::seed(*channel, at_ms));
                pool.pick(&mut rng)
            }
            Self::Weighted {
                pools,
                cumulative,
                channel,
            } => {
                // One generator for the slot: the first draw picks the
                // source, the next the item inside it.
                let total = *cumulative.last().expect("at least one playable source");
                let mut rng = SplitMix64::new(rng::seed(*channel, at_ms));
                let draw = rng.below_u64(total);
                pools[cumulative.partition_point(|&sum| sum <= draw)].pick(&mut rng)
            }
        }
    }

    /// Tells the picker that `id` was just scheduled, by any lane. A
    /// sequential lane continues after it and a shuffle counts it toward its
    /// cooldown, when it is in their pool.
    fn record(&mut self, id: &MediaId) {
        match self {
            Self::Sequential {
                items,
                position,
                next,
            } => {
                if let Some(&p) = position.get(id.as_str()) {
                    *next = (p + 1) % items.len();
                }
            }
            Self::Shuffle { pool, .. } => pool.record(id),
            Self::Weighted { pools, .. } => {
                for pool in pools {
                    pool.record(id);
                }
            }
        }
    }
}
