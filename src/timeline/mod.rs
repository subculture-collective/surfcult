//! Pure lookups over a channel's schedule events. No I/O, no clock: every
//! function takes the time it should answer for.
//!
//! Times are Unix epoch milliseconds (`i64`). Events are half-open intervals
//! `[start_ms, end_ms)`, so at an event boundary the later event is the one on.

/// What an event puts on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EventKind {
    Program,
    Break,
    Filler,
    Ident,
}

/// One scheduled stretch of one media item. A program cut by breaks is several
/// events with increasing `media_offset_ms`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Event {
    pub start_ms: i64,
    pub end_ms: i64,
    pub media_id: String,
    /// Where in the media this event starts playing.
    pub media_offset_ms: u64,
    /// The media's duration when the event was generated.
    pub media_duration_ms: u64,
    pub kind: EventKind,
}

/// The answer to "what is on at this time?".
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lookup<'a> {
    /// `event` is on, and playing it at this time means media position
    /// `offset_ms`.
    On { event: &'a Event, offset_ms: u64 },
    /// No event covers the time. `next_start_ms` is when the next event starts,
    /// if there is one.
    Gap { next_start_ms: Option<i64> },
}

/// Finds the event containing `at_ms` and the media offset at that time.
///
/// `events` must be sorted by start time and must not overlap. An event
/// includes its start and excludes its end.
pub fn lookup(events: &[Event], at_ms: i64) -> Lookup<'_> {
    let after = events.partition_point(|event| event.start_ms <= at_ms);
    if let Some(event) = after.checked_sub(1).map(|i| &events[i])
        && at_ms < event.end_ms
    {
        // `at_ms >= start_ms`, so the difference is not negative.
        let elapsed = (at_ms - event.start_ms) as u64;
        return Lookup::On {
            event,
            offset_ms: event.media_offset_ms + elapsed,
        };
    }
    Lookup::Gap {
        next_start_ms: events.get(after).map(|event| event.start_ms),
    }
}

/// What will be on `delay_ms` after `now_ms`: the event and offset a tune-in
/// should load so the program is in the right place when the clip ends.
pub fn look_ahead(events: &[Event], now_ms: i64, delay_ms: u64) -> Lookup<'_> {
    let delay = i64::try_from(delay_ms).unwrap_or(i64::MAX);
    lookup(events, now_ms.saturating_add(delay))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(start_ms: i64, end_ms: i64, media: &str, media_offset_ms: u64) -> Event {
        Event {
            start_ms,
            end_ms,
            media_id: media.to_string(),
            media_offset_ms,
            media_duration_ms: 1_320_000,
            kind: EventKind::Program,
        }
    }

    /// Show `a` 0..10s, a break 10..12s, show `a` again 12..20s resuming at 10s,
    /// then a gap, then `b` from 30s.
    fn schedule() -> Vec<Event> {
        let mut brk = event(10_000, 12_000, "ad", 0);
        brk.kind = EventKind::Break;
        vec![
            event(0, 10_000, "a", 0),
            brk,
            event(12_000, 20_000, "a", 10_000),
            event(30_000, 40_000, "b", 0),
        ]
    }

    fn on(result: Lookup<'_>) -> (&str, u64) {
        match result {
            Lookup::On { event, offset_ms } => (event.media_id.as_str(), offset_ms),
            other => panic!("expected an event, got {other:?}"),
        }
    }

    #[test]
    fn finds_the_event_and_offset_at_a_time() {
        let events = schedule();
        assert_eq!(on(lookup(&events, 0)), ("a", 0));
        assert_eq!(on(lookup(&events, 4_500)), ("a", 4_500));
        assert_eq!(on(lookup(&events, 39_999)), ("b", 9_999));
    }

    #[test]
    fn offset_adds_the_events_media_offset() {
        let events = schedule();
        assert_eq!(on(lookup(&events, 15_000)), ("a", 13_000));
    }

    #[test]
    fn boundary_belongs_to_the_later_event() {
        let events = schedule();
        assert_eq!(on(lookup(&events, 10_000)), ("ad", 0));
        assert_eq!(on(lookup(&events, 12_000)), ("a", 10_000));
    }

    #[test]
    fn look_ahead_crosses_event_boundaries() {
        let events = schedule();
        // 9.5 s now, 3 s delay: lands 0.5 s into the second `a` event.
        assert_eq!(on(look_ahead(&events, 9_500, 3_000)), ("a", 10_500));
        // Spanning several events at once.
        assert_eq!(on(look_ahead(&events, 0, 31_000)), ("b", 1_000));
        // Zero delay is the same as a plain lookup.
        assert_eq!(look_ahead(&events, 4_000, 0), lookup(&events, 4_000));
    }

    #[test]
    fn a_time_with_no_event_is_a_gap_with_the_next_start() {
        let events = schedule();
        assert_eq!(
            lookup(&events, 25_000),
            Lookup::Gap {
                next_start_ms: Some(30_000)
            }
        );
        assert_eq!(
            lookup(&events, -5),
            Lookup::Gap {
                next_start_ms: Some(0)
            }
        );
    }

    #[test]
    fn the_end_of_the_last_event_is_a_gap_with_nothing_after() {
        let events = schedule();
        assert_eq!(
            lookup(&events, 40_000),
            Lookup::Gap {
                next_start_ms: None
            }
        );
    }

    #[test]
    fn an_empty_schedule_is_a_gap() {
        assert_eq!(
            lookup(&[], 1_000),
            Lookup::Gap {
                next_start_ms: None
            }
        );
        assert_eq!(
            look_ahead(&[], 1_000, 5_000),
            Lookup::Gap {
                next_start_ms: None
            }
        );
    }

    #[test]
    fn look_ahead_does_not_overflow() {
        let events = schedule();
        assert_eq!(
            look_ahead(&events, i64::MAX, u64::MAX),
            Lookup::Gap {
                next_start_ms: None
            }
        );
    }
}
