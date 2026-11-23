//! The pure generator. See the module docs of `schedule` for the rules.

use std::collections::{HashMap, HashSet, VecDeque};

use chrono::{DateTime, Datelike};

use super::rng::{self, SplitMix64};
use super::{Generated, MAX_EVENTS, Note, ScheduleError};
use crate::config::{Mode, Tz};
use crate::library::MediaItem;
use crate::timeline::{Event, EventKind};

/// Who the schedule is for and how it is ordered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    pub channel: u32,
    pub mode: Mode,
    /// Decides which calendar date a moment falls on, for the shuffle seed.
    pub tz: Tz,
}

/// Where the previous run stopped.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Resume {
    /// The media id of the last item scheduled.
    pub cursor: Option<String>,
    /// Media ids of the programs scheduled before this run, oldest first. Only
    /// shuffle reads it, to know which items the current pass has used.
    pub played: Vec<String>,
}

/// Generates events covering `[start_ms, end_ms)`. The last event may end after
/// `end_ms`. `start_ms >= end_ms` gives no events.
pub fn generate(
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
    let mut cursor = resume.cursor.clone();
    let mut at_ms = start_ms;
    while at_ms < end_ms {
        if events.len() >= MAX_EVENTS {
            return Err(ScheduleError::TooManyEvents);
        }
        let item = picker.next(at_ms)?;
        let runtime = i64::try_from(item.runtime_ms).map_err(|_| ScheduleError::TimeOutOfRange)?;
        let next_ms = at_ms
            .checked_add(runtime)
            .ok_or(ScheduleError::TimeOutOfRange)?;
        events.push(Event {
            start_ms: at_ms,
            end_ms: next_ms,
            media_id: item.id.as_str().to_string(),
            media_offset_ms: 0,
            media_duration_ms: item.runtime_ms,
            kind: EventKind::Program,
        });
        cursor = Some(item.id.as_str().to_string());
        at_ms = next_ms;
    }
    Ok(Generated {
        events,
        cursor,
        notes,
    })
}

fn nothing(resume: &Resume, notes: Vec<Note>) -> Generated {
    Generated {
        events: Vec::new(),
        cursor: resume.cursor.clone(),
        notes,
    }
}

/// The calendar date of a moment in `tz`, as a day number. Two moments on the
/// same local date give the same number, even on a day with 23 or 25 hours.
pub fn local_date_days(tz: Tz, at_ms: i64) -> Result<i32, ScheduleError> {
    let moment = DateTime::from_timestamp_millis(at_ms).ok_or(ScheduleError::TimeOutOfRange)?;
    Ok(moment.with_timezone(&tz).date_naive().num_days_from_ce())
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
        tz: Tz,
        /// Indexes into `playable` already used in the current pass.
        done: HashSet<usize>,
        /// The rest of the current pass, in play order.
        queue: VecDeque<usize>,
        last: Option<usize>,
        /// Passes drawn so far in this run.
        salt: u32,
    },
}

impl<'a> Picker<'a> {
    fn sequential(items: Vec<&'a MediaItem>, resume: &Resume, notes: &mut Vec<Note>) -> Self {
        let position = resume
            .cursor
            .as_deref()
            .and_then(|cursor| items.iter().position(|item| item.id.as_str() == cursor));
        if let (Some(cursor), None) = (&resume.cursor, position) {
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
        let lone_cursor = resume.cursor.iter().cloned().collect::<Vec<_>>();
        let history = if resume.played.is_empty() {
            &lone_cursor
        } else {
            &resume.played
        };

        // Replay the history through the same pass rule the generator follows.
        // An id that is not in the pool is forgotten.
        let mut done = HashSet::new();
        let mut last = None;
        for id in history {
            let Some(&i) = index.get(id.as_str()) else {
                continue;
            };
            if done.contains(&i) {
                // History that repeats inside a pass: a new pass began here.
                done.clear();
            }
            done.insert(i);
            if done.len() == playable.len() {
                done.clear();
            }
            last = Some(i);
        }
        Self::Shuffle {
            channel: params.channel,
            tz: params.tz,
            playable,
            done,
            queue: VecDeque::new(),
            last,
            salt: 0,
        }
    }

    fn next(&mut self, at_ms: i64) -> Result<&'a MediaItem, ScheduleError> {
        match self {
            Self::Sequential { items, next } => loop {
                let item = items[*next % items.len()];
                *next = (*next + 1) % items.len();
                if item.runtime_ms > 0 {
                    return Ok(item);
                }
            },
            Self::Shuffle {
                playable,
                channel,
                tz,
                done,
                queue,
                last,
                salt,
            } => {
                if queue.is_empty() {
                    if done.len() >= playable.len() {
                        done.clear();
                    }
                    let days = local_date_days(*tz, at_ms)?;
                    let mut rng = SplitMix64::new(rng::seed(*channel, days, *salt));
                    *salt += 1;
                    let mut order = rng::shuffled_indices(playable.len(), &mut rng);
                    // A new pass must not open with what just played.
                    if done.is_empty() && order.len() > 1 && Some(order[0]) == *last {
                        order.swap(0, 1);
                    }
                    queue.extend(order.into_iter().filter(|i| !done.contains(i)));
                }
                let chosen = queue
                    .pop_front()
                    .expect("a pass has at least one item left");
                done.insert(chosen);
                *last = Some(chosen);
                Ok(playable[chosen])
            }
        }
    }
}
