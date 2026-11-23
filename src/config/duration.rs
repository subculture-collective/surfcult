use std::time::Duration;

/// Parses a whole number followed by `s`, `m` or `h`: `90s`, `11m`, `1h`.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .ok_or_else(|| format!("{text:?} needs a unit: s, m or h"))?;
    let (digits, unit) = text.split_at(split);
    let amount: u64 = digits
        .parse()
        .map_err(|_| format!("{text:?} is not a duration like 30m"))?;
    let seconds_per_unit = match unit {
        "s" => 1,
        "m" => 60,
        "h" => 3600,
        _ => return Err(format!("{text:?} has unknown unit {unit:?}; use s, m or h")),
    };
    amount
        .checked_mul(seconds_per_unit)
        .map(Duration::from_secs)
        .ok_or_else(|| format!("{text:?} is too long"))
}

/// Parses `HH:MM` (24-hour) into minutes after midnight. `24:00` is allowed as
/// the end of the day.
pub fn parse_minute_of_day(text: &str) -> Result<u16, String> {
    let bad = || format!("{text:?} is not a time like 06:30");
    let (hours, minutes) = text.trim().split_once(':').ok_or_else(bad)?;
    let hours: u16 = hours.parse().map_err(|_| bad())?;
    let minutes: u16 = minutes.parse().map_err(|_| bad())?;
    if minutes >= 60 || hours > 24 || (hours == 24 && minutes != 0) {
        return Err(bad());
    }
    Ok(hours * 60 + minutes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        assert_eq!(parse_duration("90s"), Ok(Duration::from_secs(90)));
        assert_eq!(parse_duration("11m"), Ok(Duration::from_secs(660)));
        assert_eq!(parse_duration("1h"), Ok(Duration::from_secs(3600)));
    }

    #[test]
    fn rejects_bad_durations() {
        for bad in ["", "30", "m", "1.5h", "5d", "-5m", "99999999999999999999s"] {
            assert!(parse_duration(bad).is_err(), "{bad:?} should be rejected");
        }
    }

    #[test]
    fn parses_times_of_day() {
        assert_eq!(parse_minute_of_day("00:00"), Ok(0));
        assert_eq!(parse_minute_of_day("6:30"), Ok(390));
        assert_eq!(parse_minute_of_day("24:00"), Ok(1440));
    }

    #[test]
    fn rejects_bad_times_of_day() {
        for bad in ["", "0630", "25:00", "24:01", "12:60", "ab:cd"] {
            assert!(
                parse_minute_of_day(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }
}
