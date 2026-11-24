//! Time-of-day blocks: turns a blocks channel's local wall-clock times into
//! epoch spans. See "Blocks" in the module docs of `schedule` for the rules.

use chrono::{Datelike, Days, LocalResult, NaiveDate, TimeZone, Utc, Weekday};

use super::ScheduleError;
use crate::config::{Block, Day, Tz};

/// A stretch of a blocks channel's time, `[start_ms, end_ms)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub start_ms: i64,
    pub end_ms: i64,
    /// The index of the block that covers it, or `None` for time no block
    /// covers.
    pub block: Option<usize>,
}

/// How many days before and after the window are looked at. Every block
/// recurs at least once a week, so the spans around the window have their
/// true starts and ends.
const MARGIN_DAYS: u64 = 8;

/// The spans covering `[start_ms, end_ms)`, in time order, with no gap and no
/// overlap. The first may start before `start_ms` and the last may end after
/// `end_ms`; each keeps its true start and end. `start_ms >= end_ms` gives
/// none.
pub(crate) fn spans(
    blocks: &[Block],
    tz: Tz,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Span>, ScheduleError> {
    if start_ms >= end_ms {
        return Ok(Vec::new());
    }
    let first = local_date(tz, start_ms)?
        .checked_sub_days(Days::new(MARGIN_DAYS))
        .ok_or(ScheduleError::TimeOutOfRange)?;
    let last = local_date(tz, end_ms)?
        .checked_add_days(Days::new(MARGIN_DAYS))
        .ok_or(ScheduleError::TimeOutOfRange)?;

    let mut found = Vec::new();
    let mut date = first;
    while date <= last {
        let day = day_of(date.weekday());
        for (i, block) in blocks.iter().enumerate() {
            if block.days.contains(&day) {
                let start = local_ms(tz, date, block.start_minute)?;
                let end = local_ms(tz, date, block.end_minute)?;
                found.push((start, end, i));
            }
        }
        date = date.succ_opt().ok_or(ScheduleError::TimeOutOfRange)?;
    }
    found.sort_unstable();

    let mut spans = Vec::new();
    let mut covered = local_ms(tz, first, 0)?;
    for (start, end, i) in found {
        // Blocks do not overlap in local time; clamping keeps that true for
        // any time the clocks change inside.
        let start = start.max(covered);
        if start > covered {
            spans.push(Span {
                start_ms: covered,
                end_ms: start,
                block: None,
            });
        }
        if end > start {
            spans.push(Span {
                start_ms: start,
                end_ms: end,
                block: Some(i),
            });
            covered = end;
        }
    }
    let tail = local_ms(tz, last, 24 * 60)?;
    if tail > covered {
        spans.push(Span {
            start_ms: covered,
            end_ms: tail,
            block: None,
        });
    }
    spans.retain(|span| span.end_ms > start_ms && span.start_ms < end_ms);
    Ok(spans)
}

/// The local date at `ms`.
fn local_date(tz: Tz, ms: i64) -> Result<NaiveDate, ScheduleError> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|utc| utc.with_timezone(&tz).date_naive())
        .ok_or(ScheduleError::TimeOutOfRange)
}

/// The instant of `minute` minutes after midnight on local `date`; 1440 is
/// midnight at the start of the next day.
///
/// A time that occurs twice, when the clocks go back, is its first
/// occurrence. A time that does not occur, when the clocks go forward, is the
/// moment they jump, the first local time after it.
pub(crate) fn local_ms(tz: Tz, date: NaiveDate, minute: u16) -> Result<i64, ScheduleError> {
    let date = if minute >= 24 * 60 {
        date.succ_opt().ok_or(ScheduleError::TimeOutOfRange)?
    } else {
        date
    };
    let minute = u32::from(minute % (24 * 60));
    let mut local = date
        .and_hms_opt(minute / 60, minute % 60, 0)
        .ok_or(ScheduleError::TimeOutOfRange)?;
    // No zone skips a whole day, so this finds the jump well within it.
    for _ in 0..24 * 60 {
        match tz.from_local_datetime(&local) {
            LocalResult::Single(at) | LocalResult::Ambiguous(at, _) => {
                return Ok(at.timestamp_millis());
            }
            LocalResult::None => {
                local = local
                    .checked_add_signed(chrono::TimeDelta::minutes(1))
                    .ok_or(ScheduleError::TimeOutOfRange)?;
            }
        }
    }
    Err(ScheduleError::TimeOutOfRange)
}

fn day_of(weekday: Weekday) -> Day {
    match weekday {
        Weekday::Mon => Day::Mon,
        Weekday::Tue => Day::Tue,
        Weekday::Wed => Day::Wed,
        Weekday::Thu => Day::Thu,
        Weekday::Fri => Day::Fri,
        Weekday::Sat => Day::Sat,
        Weekday::Sun => Day::Sun,
    }
}
