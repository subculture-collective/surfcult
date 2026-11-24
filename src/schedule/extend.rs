//! Keeps a channel's week filled: reads the stored schedule, calls `generate`
//! or `generate_blocks` and writes the new events in one transaction.

use std::collections::HashSet;

use super::blocks::spans;
use super::{
    BreakPlan, Lane, Layout, Note, Params, ROTATION_MEMORY_MS, Resume, ScheduleError,
    cooling_target, generate, generate_blocks,
};
use crate::clips::Pool;
use crate::config::{Channel, Mode, Plan, Source, Tz};
use crate::library::{MediaId, MediaItem};
use crate::store::Store;

/// How far ahead of `now` a channel's schedule is kept filled: seven days.
pub const HORIZON_MS: i64 = 7 * 24 * 60 * 60 * 1000;

/// What [`extend`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Extended {
    /// The caller said the clock is not trusted, so nothing was read or written.
    ClockNotTrusted,
    /// The stored schedule already reaches seven days past `now`.
    UpToDate { until_ms: i64 },
    /// Events for `[from_ms, to_ms)` were generated and `written` of them stored.
    Generated {
        from_ms: i64,
        to_ms: i64,
        written: usize,
        notes: Vec<Note>,
    },
}

/// Fills `channel`'s schedule up to `now_ms` plus seven days.
///
/// `pools` holds the program pool of each source the channel's plan names
/// ([`Plan::sources`]), each resolved with
/// [`library::resolve_pool`](crate::library::resolve_pool). A source that is
/// not there counts as an empty pool. `timezone` is the config's time zone,
/// in which a blocks channel's times are read; other channels ignore it.
///
/// `clips` are the scanned clip pools, usually
/// [`ClipLibrary::pools`](crate::clips::ClipLibrary::pools). The channel's
/// break pool and its `filler` pool are looked up there by name; a pool that
/// is not there counts as empty. A channel without breaks or blocks ignores
/// `clips`.
///
/// Does nothing unless `clock_trusted` is true: a Pi whose clock has not
/// synchronized must not store a wrong week. Starts at the later of `now_ms`
/// and the end of the last stored event, so repeated calls extend the schedule
/// without gaps. Where to continue comes from the channel's stored program
/// events, and the new events are its only write, so a crash cannot leave the
/// schedule and the resume point disagreeing.
pub fn extend(
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
    let n = channel.number;

    // Events that end after `now_ms`; the last of them is where to continue.
    let last_end = store
        .events_between(n, now_ms, i64::MAX)?
        .last()
        .map(|event| event.end_ms);
    let from_ms = last_end.map_or(now_ms, |end| end.max(now_ms));
    if from_ms >= horizon_ms {
        return Ok(Extended::UpToDate { until_ms: from_ms });
    }

    let clips_of = |name: &str| {
        clips
            .iter()
            .find(|pool| pool.name() == name)
            .map_or(&[][..], Pool::clips)
    };
    let pool_of = |source: &Source| {
        pools
            .iter()
            .find(|(pooled, _)| pooled == source)
            .map_or(&[][..], |(_, items)| items.as_slice())
    };
    let breaks = channel.breaks.as_ref().map(|breaks| BreakPlan {
        every_ms: millis(breaks.every),
        length_ms: millis(breaks.length),
        clips: clips_of(&breaks.pool),
        pad: channel.filler.as_deref().map_or(&[][..], clips_of),
    });
    // The rotation continues after the last break clip already stored. A clip
    // that started more than ROTATION_MEMORY_MS before `from_ms` cannot affect
    // the result: every new break starts at or after `from_ms`, so the
    // rotation starts again at the first clip either way. Reading only that
    // window keeps a channel that never stored a break from walking its whole
    // history.
    let last_break = match breaks {
        Some(_) => {
            store.last_break_before(n, from_ms.saturating_sub(ROTATION_MEMORY_MS), from_ms)?
        }
        None => None,
    };

    let generated = match &channel.plan {
        Plan::Fixed {
            source,
            mode,
            weights,
        } => {
            let pool = pool_of(source);
            // Sequential and weighted need only the last earlier program.
            // Shuffle needs the programs back to the N - k distinct pool items
            // cooling down.
            let recent = if *mode == Mode::Shuffle {
                let lane = Lane {
                    mode: *mode,
                    weights,
                    pool,
                };
                recent_history(store, n, &[lane], from_ms)?
            } else {
                store.last_program_before(n, from_ms)?.into_iter().collect()
            };
            let params = Params {
                channel: n,
                mode: *mode,
                weights,
                breaks,
            };
            let resume = Resume { recent, last_break };
            generate(&params, pool, &resume, from_ms, horizon_ms)?
        }
        Plan::Blocks { blocks, default } => {
            let mut lanes: Vec<Lane<'_>> = blocks
                .iter()
                .map(|block| Lane {
                    mode: block.mode,
                    weights: &block.weights,
                    pool: pool_of(&block.source),
                })
                .collect();
            let default = default.as_ref().map(|lineup| {
                lanes.push(Lane {
                    mode: lineup.mode,
                    weights: &lineup.weights,
                    pool: pool_of(&lineup.source),
                });
                lanes.len() - 1
            });
            let recent = recent_history(store, n, &lanes, from_ms)?;
            let layout = Layout {
                channel: n,
                breaks,
                spans: spans(blocks, timezone, from_ms, horizon_ms)?,
                lanes,
                default,
                filler: channel.filler.as_deref().map_or(&[][..], clips_of),
            };
            let resume = Resume { recent, last_break };
            generate_blocks(&layout, &resume, from_ms, horizon_ms)?
        }
    };
    if !generated.events.is_empty() {
        store.write_events(n, &generated.events)?;
    }
    Ok(Extended::Generated {
        from_ms,
        to_ms: horizon_ms,
        written: generated.events.len(),
        notes: generated.notes,
    })
}

/// A configured duration in whole milliseconds. Config durations are whole
/// seconds or more, so nothing is lost.
fn millis(duration: std::time::Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// Reads `channel`'s programs before `before_ms`, newest first, until every
/// lane has what it resumes from, or the history ends: a shuffle lane the
/// N - k distinct pool items it keeps cooling down, and a sequential lane
/// one program from its pool. A weighted lane needs none. Every run therefore
/// sees what an unbroken run would.
///
/// Pages of twice the pools' total length leave room for programs cut into
/// several events and ids from other pools or no longer in any, so one page
/// is usually enough. The read goes further back when the recent rows are
/// mostly such ids, and reads the whole history when it never satisfies some
/// lane, for example a block whose pool has not played yet.
fn recent_history(
    store: &Store,
    channel: u32,
    lanes: &[Lane<'_>],
    before_ms: i64,
) -> Result<Vec<MediaId>, ScheduleError> {
    // For each lane: the ids it counts, how many distinct ones it needs, and
    // those found so far.
    let mut needs: Vec<(HashSet<&str>, usize, HashSet<&str>)> = lanes
        .iter()
        .filter_map(|lane| match lane.mode {
            Mode::Shuffle => Some(cooling_target(lane.pool)),
            Mode::Sequential => {
                let ids: HashSet<&str> = lane.pool.iter().map(|item| item.id.as_str()).collect();
                let target = ids.len().min(1);
                Some((ids, target))
            }
            Mode::Weighted => None,
        })
        .map(|(ids, target)| (ids, target, HashSet::new()))
        .collect();
    let total: usize = lanes.iter().map(|lane| lane.pool.len()).sum();
    let page = u32::try_from(total.saturating_mul(2).max(1)).unwrap_or(u32::MAX);
    let mut recent = Vec::new();
    let mut bound = before_ms;
    while needs.iter().any(|(_, target, found)| found.len() < *target) {
        let rows = store.recent_programs_before(channel, bound, page)?;
        let full = rows.len() == page as usize;
        for (start_ms, id) in rows {
            bound = start_ms;
            for (ids, _, found) in &mut needs {
                if let Some(&pooled) = ids.get(id.as_str()) {
                    found.insert(pooled);
                }
            }
            recent.push(id);
        }
        if !full {
            break;
        }
    }
    Ok(recent)
}
