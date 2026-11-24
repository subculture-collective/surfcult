//! The pure generator. See the module docs of `schedule` for the rules.

use std::collections::{HashMap, HashSet};

use super::rng::{self, SplitMix64};
use super::{Generated, MAX_EVENTS, Note, ScheduleError};
use crate::config::Mode;
use crate::library::{MediaId, MediaItem};
use crate::timeline::{Event, EventKind};

/// Who the schedule is for and how it is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Params {
    pub channel: u32,
    pub mode: Mode,
}

/// Where the previous run stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Resume {
    /// Media ids of the programs scheduled before this run, newest first. The
    /// first is the cursor, the last item scheduled. Sequential reads only the
    /// cursor; shuffle reads as many as it needs to find the items cooling down.
    pub recent: Vec<MediaId>,
}

impl Resume {
    fn cursor(&self) -> Option<&MediaId> {
        self.recent.first()
    }
}

/// Generates events covering `[start_ms, end_ms)`. The last event may end after
/// `end_ms`. `start_ms >= end_ms` gives no events.
pub(crate) fn generate(
    params: &Params,
    pool: &[MediaItem],
    resume: &Resume,
    start_ms: i64,
    end_ms: i64,
) -> Result<Generated, ScheduleError> {
    if !matches!(params.mode, Mode::Sequential | Mode::Shuffle) {
        return Err(ScheduleError::UnsupportedMode(params.mode));
    }

    let mut seen = HashSet::new();
    let unique: Vec<&MediaItem> = pool
        .iter()
        .filter(|item| seen.insert(item.id.as_str()))
        .collect();
    let mut notes = Vec::new();
    let skipped = unique.iter().filter(|item| item.runtime_ms == 0).count();
    if skipped > 0 {
        notes.push(Note::SkippedZeroRuntime { count: skipped });
    }
    if skipped == unique.len() {
        notes.push(Note::NoPlayableItems);
        return Ok(nothing(resume, notes));
    }

    let mut picker = match params.mode {
        Mode::Sequential => Picker::sequential(unique, resume, &mut notes),
        _ => Picker::shuffle(unique, params, resume),
    };

    let mut events = Vec::new();
    let mut cursor = resume.cursor().cloned();
    let mut at_ms = start_ms;
    while at_ms < end_ms {
        if events.len() >= MAX_EVENTS {
            return Err(ScheduleError::TooManyEvents);
        }
        let item = picker.next(at_ms);
        let runtime = i64::try_from(item.runtime_ms).map_err(|_| ScheduleError::TimeOutOfRange)?;
        let next_ms = at_ms
            .checked_add(runtime)
            .ok_or(ScheduleError::TimeOutOfRange)?;
        events.push(Event {
            start_ms: at_ms,
            end_ms: next_ms,
            media_id: item.id.clone(),
            media_offset_ms: 0,
            media_duration_ms: item.runtime_ms,
            kind: EventKind::Program,
        });
        cursor = Some(item.id.clone());
        at_ms = next_ms;
    }
    Ok(Generated {
        events,
        cursor,
        notes,
    })
}

/// How many distinct items a shuffle over `n` playable items keeps cooling
/// down: N - k with k = max(1, ceil(N / 4)).
fn cooldown(n: usize) -> usize {
    n.saturating_sub(n.div_ceil(4).max(1))
}

/// The ids a shuffle over `pool` counts toward its cooldown, its distinct
/// playable items, and how many of them it keeps cooling down.
pub(crate) fn cooling_target(pool: &[MediaItem]) -> (HashSet<&str>, usize) {
    let mut seen = HashSet::new();
    let playable: HashSet<&str> = pool
        .iter()
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

/// Chooses the next item to play. At least one item has a runtime above 0.
enum Picker<'a> {
    Sequential {
        /// Every distinct pool item, zero runtimes included, because the cursor
        /// is a position in this list.
        items: Vec<&'a MediaItem>,
        next: usize,
    },
    Shuffle {
        playable: Vec<&'a MediaItem>,
        channel: u32,
        /// Indexes into `playable` of the programs before this slot, oldest
        /// first: the resumed history that is still in the pool, then this
        /// run's picks.
        played: Vec<usize>,
        /// How many distinct items are cooling down: N - k.
        cooldown: usize,
    },
}

impl<'a> Picker<'a> {
    fn sequential(items: Vec<&'a MediaItem>, resume: &Resume, notes: &mut Vec<Note>) -> Self {
        let position = resume
            .cursor()
            .and_then(|cursor| items.iter().position(|item| &item.id == cursor));
        if let (Some(cursor), None) = (resume.cursor(), position) {
            notes.push(Note::CursorNotInPool {
                media_id: cursor.clone(),
            });
        }
        Self::Sequential {
            next: position.map_or(0, |p| p + 1),
            items,
        }
    }

    fn shuffle(items: Vec<&'a MediaItem>, params: &Params, resume: &Resume) -> Self {
        let playable: Vec<&MediaItem> = items.into_iter().filter(|i| i.runtime_ms > 0).collect();
        let index: HashMap<&str, usize> = playable
            .iter()
            .enumerate()
            .map(|(i, item)| (item.id.as_str(), i))
            .collect();
        // An id that is no longer in the pool is ignored.
        let played = resume
            .recent
            .iter()
            .rev()
            .filter_map(|id| index.get(id.as_str()).copied())
            .collect();
        Self::Shuffle {
            cooldown: cooldown(playable.len()),
            channel: params.channel,
            playable,
            played,
        }
    }

    fn next(&mut self, at_ms: i64) -> &'a MediaItem {
        match self {
            Self::Sequential { items, next } => loop {
                let item = items[*next % items.len()];
                *next = (*next + 1) % items.len();
                if item.runtime_ms > 0 {
                    return item;
                }
            },
            Self::Shuffle {
                playable,
                channel,
                played,
                cooldown,
            } => {
                // Walk back to the `cooldown` most recent distinct items. A
                // program cut into several events appears once in the walk.
                let mut cooling = HashSet::new();
                for &i in played.iter().rev() {
                    if cooling.len() == *cooldown {
                        break;
                    }
                    cooling.insert(i);
                }
                let eligible: Vec<usize> = (0..playable.len())
                    .filter(|i| !cooling.contains(i))
                    .collect();
                let mut rng = SplitMix64::new(rng::seed(*channel, at_ms));
                let chosen = eligible[rng.below(eligible.len())];
                played.push(chosen);
                playable[chosen]
            }
        }
    }
}
