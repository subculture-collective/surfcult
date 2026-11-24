//! The nightly sign-off: turns a channel's `sign_off` times into epoch
//! windows and picks its clip. See "Sign-off" in the module docs of
//! `schedule` for the rules.

use std::path::Path;

use chrono::{Days, NaiveDate, TimeZone, Utc};

use super::ScheduleError;
use super::blocks::local_ms;
use super::breaks::usable;
use crate::clips::Clip;
use crate::config::{SignOff, Tz};
use crate::library::MediaId;

/// One night off the air, `[start_ms, end_ms)`: the sign-off clip starts at
/// `start_ms` and the channel is back on the air at `end_ms`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Night {
    pub start_ms: i64,
    pub end_ms: i64,
}

/// How many days before the window are looked at, so the night before it
/// is always among the nights and the time after it has a known start.
const MARGIN_DAYS: u64 = 2;

/// The nights from two days before `start_ms` to a day after `end_ms`, in time
/// order and not overlapping. A night that the clock change leaves empty is
/// left out.
pub(crate) fn nights(
    sign_off: &SignOff,
    tz: Tz,
    start_ms: i64,
    end_ms: i64,
) -> Result<Vec<Night>, ScheduleError> {
    let mut date = local_date(tz, start_ms)?
        .checked_sub_days(Days::new(MARGIN_DAYS))
        .ok_or(ScheduleError::TimeOutOfRange)?;
    let last = local_date(tz, end_ms.max(start_ms))?
        .checked_add_days(Days::new(1))
        .ok_or(ScheduleError::TimeOutOfRange)?;
    let next_morning = sign_off.until_minute <= sign_off.at_minute;
    let mut found: Vec<Night> = Vec::new();
    while date <= last {
        let start = local_ms(tz, date, sign_off.at_minute)?;
        let back_on = if next_morning {
            date.succ_opt().ok_or(ScheduleError::TimeOutOfRange)?
        } else {
            date
        };
        let end = local_ms(tz, back_on, sign_off.until_minute)?;
        // Local times map to instants in order, so nights cannot overlap;
        // the clamp keeps that true whatever the clocks do.
        let start = found.last().map_or(start, |night| start.max(night.end_ms));
        if end > start {
            found.push(Night {
                start_ms: start,
                end_ms: end,
            });
        }
        date = date.succ_opt().ok_or(ScheduleError::TimeOutOfRange)?;
    }
    Ok(found)
}

/// The sign-off clip in `clips`: the usable clip whose file name is `name`
/// when there is one, else the pool's first usable clip. `None` when the pool
/// has no usable clip.
pub(crate) fn clip(clips: &[Clip], name: Option<&str>) -> Option<(MediaId, u64)> {
    let usable = usable(clips);
    let named = name.and_then(|name| {
        usable.iter().find(|(id, _)| {
            Path::new(id.as_str())
                .file_name()
                .is_some_and(|file| file == name)
        })
    });
    named.or(usable.first()).cloned()
}

/// The local date at `ms`.
fn local_date(tz: Tz, ms: i64) -> Result<NaiveDate, ScheduleError> {
    Utc.timestamp_millis_opt(ms)
        .single()
        .map(|utc| utc.with_timezone(&tz).date_naive())
        .ok_or(ScheduleError::TimeOutOfRange)
}
