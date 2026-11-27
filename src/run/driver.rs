//! The playback driver: keeps the screen on the tuned channel's schedule
//! after the tuner's sequence is over.
//!
//! The tuner gets the program on screen at its live position and stops
//! there. The driver follows the channel's stored events from then on. At
//! each event boundary it shows what the schedule says:
//!
//! - a program event plays its media at the live position (a program cut by
//!   breaks continues at the next segment's `media_offset_ms`); a segment
//!   that continues the media on screen without a break needs no command;
//! - a break, filler or ident event plays the local clip whose canonical path
//!   is the event's media id ([`clip_media_id`](crate::schedule::clip_media_id));
//! - time with no event (a gap, dead air, or off the air after a sign-off)
//!   shows the stand-by card, and the next event starts on time: a program
//!   after the card is started its expected start-up time early (about 1 s
//!   for direct play, 7 s for a transcode), since the card can be cut.
//!
//! # Queue or replace
//!
//! A clip's length is known exactly (ffprobe), so when a clip ends at its
//! event's end the driver queues the next item behind it as soon as the clip
//! starts, and MPV opens it while the clip plays and starts it when the clip
//! ends, with no gap. A program is replaced at its boundary instead: a
//! segment cut by a break never ends where its file does, and a program's
//! real length is not known until the player has it (fitting a mismatch is
//! later work). Replacing a program with a local clip or the card is
//! immediate; replacing it with another program shows nothing for that
//! program's start-up time.
//!
//! A clip shorter than its event, such as a break's pad that repeats one
//! ident, loops until the next item replaces it at the boundary, so the
//! screen never runs out of video. Nothing is queued behind it.
//!
//! # Position
//!
//! The position always comes from the schedule at the current time, never
//! from where the viewer left the channel, so switching away and back never
//! restarts a program. When a program the driver started reports that it
//! started more than the tuner's late tolerance behind its live position,
//! the driver seeks to the live position once, as the tuner does.
//!
//! # Failures
//!
//! A player or Jellyfin error is logged, the stand-by card shows, and the
//! driver moves on at the next boundary. Each started program and clip is
//! recorded in the playback history as `played`, or `failed` when the player
//! refused it.

use std::time::Duration;

use crate::clips::{Clip, ClipLibrary};
use crate::library::profile::needs_transcode;
use crate::library::{MediaId, MediaSource};
use crate::player::Player;
use crate::schedule::{clip_media_id, fit, record_fit};
use crate::store::Store;
use crate::timeline::{Event, EventKind, Lookup, lookup, program_from};
use crate::tuner::Timing;

use super::Streams;

/// How far ahead the driver reads the channel's events.
const READ_SPAN_MS: i64 = 3 * 60 * 60 * 1000;

/// The events are read again when the time looked up comes this close to
/// the end of what was read.
const REFRESH_AHEAD_MS: i64 = 60 * 60 * 1000;

/// While the card shows with nothing scheduled after it, the events are read
/// again this often, so a schedule written meanwhile starts playing.
const RECHECK_MS: i64 = 60 * 1000;

/// A clip that ends within this much of its event's end gets the next item
/// queued behind it.
pub(super) const QUEUE_TOLERANCE_MS: i64 = 1_000;

/// What the screen shows, as the driver understands it.
#[derive(Debug, Clone, PartialEq, Eq)]
enum On {
    /// This event, as the schedule says.
    Event(Event),
    /// The stand-by card.
    Standby,
    /// Something the schedule does not account for, such as a program the
    /// tuner started that is no longer on.
    Unknown,
}

struct Following {
    channel: u32,
    events: Vec<Event>,
    /// The events are read up to here.
    read_until_ms: i64,
    stale: bool,
    on: On,
    /// When the schedule moves on from what is on screen: the event's end,
    /// or the start of the next event after a gap. `None` when nothing more
    /// is scheduled.
    until_ms: Option<i64>,
    /// When to act: `until_ms`, or earlier for a program after the card.
    fire_ms: Option<i64>,
    /// The next event, already queued behind the clip on screen.
    queued: Option<Event>,
    /// A program the driver started that has not reported its start.
    awaiting: Option<MediaId>,
    recheck_ms: i64,
}

/// The things a step uses that the loop owns.
pub(super) struct Context<'a, P> {
    pub player: &'a mut P,
    pub store: &'a mut Store,
    pub clips: &'a ClipLibrary,
    pub streams: &'a mut dyn Streams,
}

pub(super) struct ProgramStartContext<'a, 'b, P> {
    pub(super) player: &'a mut P,
    pub(super) clips: &'b ClipLibrary,
    pub(super) store: &'a mut Store,
    pub(super) filler: &'b [Clip],
}

pub(super) struct Driver {
    timing: Timing,
    following: Option<Following>,
}

impl Driver {
    pub(super) fn new(timing: Timing) -> Self {
        Self {
            timing,
            following: None,
        }
    }

    pub(super) fn following(&self) -> bool {
        self.following.is_some()
    }

    /// Stops following. The caller has started a new tune.
    pub(super) fn leave(&mut self) {
        self.following = None;
    }

    /// The schedule changed in the store: read it again on the next step.
    pub(super) fn invalidate(&mut self) {
        if let Some(f) = self.following.as_mut() {
            f.stale = true;
        }
    }

    /// The tuner's program is on screen. Follows `channel` from the event
    /// the tuner queued: the program on now, or the program after the break
    /// that is on now (the tune-in clip stood in for the break).
    pub(super) fn join_live(&mut self, channel: u32, now_ms: i64, store: &mut Store) {
        let mut f = Following::new(channel);
        f.refresh(now_ms, store);
        let started = match program_from(&f.events, now_ms) {
            Lookup::On { event, .. } => Some(event.clone()),
            Lookup::Gap { .. } => None,
        };
        match started {
            Some(event) => {
                record(store, channel, &event.media_id, now_ms, "played");
                f.until_ms = Some(event.end_ms);
                f.on = On::Event(event);
            }
            None => {
                f.on = On::Unknown;
                f.until_ms = Some(now_ms);
            }
        }
        self.following = Some(f);
        self.set_fire(&*store);
    }

    /// The tuner's sequence ended without a program and the stand-by card is
    /// on screen. Follows `channel` from now: what is on now plays at once,
    /// and a gap keeps the card until the next event.
    pub(super) fn join_standby(&mut self, channel: u32, now_ms: i64, store: &mut Store) {
        let mut f = Following::new(channel);
        f.refresh(now_ms, store);
        f.on = On::Standby;
        f.until_ms = Some(now_ms);
        self.following = Some(f);
        self.set_fire(&*store);
    }

    /// A program reported its start at `position_ms`. If the driver started
    /// it and it is behind its live position by more than the late
    /// tolerance, seeks there once.
    pub(super) fn program_started(
        &mut self,
        now_ms: i64,
        position_ms: u64,
        duration_ms: u64,
        cx: &mut ProgramStartContext<'_, '_, impl Player>,
    ) {
        let Some(f) = self.following.as_mut() else {
            return;
        };
        let Some(media) = f.awaiting.take() else {
            return;
        };
        if let Lookup::On {
            event, offset_ms, ..
        } = lookup(&f.events, now_ms)
            && event.kind == EventKind::Program
            && event.media_id == media
        {
            if offset_ms > position_ms.saturating_add(self.timing.late_tolerance_ms)
                && let Err(e) = cx.player.seek(offset_ms)
            {
                eprintln!(
                    "surfcult: channel {}: cannot seek to the live position: {e}",
                    f.channel
                );
            }
            if duration_ms > 0 {
                let fitted = fit(event, duration_ms, cx.filler);
                if let Err(e) = record_fit(cx.store, f.channel, event, fitted.fit, now_ms) {
                    eprintln!(
                        "surfcult: channel {}: cannot record program fit: {e}",
                        f.channel
                    );
                }
                if fitted.events.len() > 1 {
                    if let Some(index) = f.events.iter().position(|stored| stored == event) {
                        f.events
                            .splice(index..index + 1, fitted.events.iter().cloned());
                    }
                    if let Some(filler_event) =
                        fitted.events.iter().find(|e| e.kind == EventKind::Filler)
                        && let Some(clip) = clip_for(cx.clips, &filler_event.media_id)
                    {
                        match cx.player.queue_clip(&clip.path) {
                            Ok(()) => f.queued = Some(filler_event.clone()),
                            Err(e) => eprintln!(
                                "surfcult: channel {}: cannot queue filler: {e}",
                                f.channel
                            ),
                        }
                    }
                }
            }
        }
    }

    /// Does what is due at `now_ms`.
    pub(super) fn step(&mut self, now_ms: i64, cx: &mut Context<'_, impl Player>) {
        let Some(f) = self.following.as_mut() else {
            return;
        };
        let at = f.until_ms.map_or(now_ms, |until| until.max(now_ms));
        let recheck = f.on == On::Standby && f.until_ms.is_none() && now_ms >= f.recheck_ms;
        if f.stale || recheck || at.saturating_add(REFRESH_AHEAD_MS) >= f.read_until_ms {
            f.refresh(now_ms, cx.store);
            if f.on == On::Standby {
                // A schedule written meanwhile may start sooner.
                f.until_ms = match lookup(&f.events, now_ms) {
                    Lookup::On { .. } => Some(now_ms),
                    Lookup::Gap { next_start_ms } => next_start_ms,
                };
            }
            self.set_fire(&*cx.store);
        }
        let due = self
            .following
            .as_ref()
            .and_then(|f| f.fire_ms)
            .is_some_and(|fire| now_ms >= fire);
        if due {
            self.advance(now_ms, cx);
        }
    }

    /// Moves the screen to what the schedule has at the boundary.
    fn advance(&mut self, now_ms: i64, cx: &mut Context<'_, impl Player>) {
        let Some(f) = self.following.as_mut() else {
            return;
        };
        let at = f.until_ms.map_or(now_ms, |until| until.max(now_ms));
        let target = match lookup(&f.events, at) {
            Lookup::On {
                event, offset_ms, ..
            } => Some((event.clone(), offset_ms)),
            Lookup::Gap { next_start_ms } => {
                f.queued = None;
                if f.on != On::Standby {
                    cx.streams.stop_all();
                    show_standby(f.channel, cx.player);
                    f.on = On::Standby;
                }
                f.until_ms = next_start_ms;
                None
            }
        };
        if let Some((event, offset_ms)) = target {
            if f.queued.take().as_ref() == Some(&event) {
                // The player started it when the clip before it ended.
                record(cx.store, f.channel, &event.media_id, now_ms, "played");
                let started_ms = event.start_ms;
                f.until_ms = Some(event.end_ms);
                f.on = On::Event(event.clone());
                if event.kind != EventKind::Program
                    && let Some(clip) = clip_for(cx.clips, &event.media_id)
                {
                    f.queue_successor(&event, started_ms, clip.duration, cx);
                }
            } else {
                f.show(event, offset_ms, now_ms, cx);
            }
        }
        self.set_fire(&*cx.store);
    }

    /// Sets when the driver next acts: at the boundary, or a program's
    /// start-up time before it when the card is on screen.
    fn set_fire(&mut self, media: &impl MediaSource) {
        let timing = self.timing;
        let Some(f) = self.following.as_mut() else {
            return;
        };
        f.fire_ms = match (&f.on, f.until_ms) {
            (On::Standby, Some(until)) => match lookup(&f.events, until) {
                Lookup::On { event, .. }
                    if event.kind == EventKind::Program && event.start_ms == until =>
                {
                    let cost = match media.resolve(&event.media_id) {
                        Ok(item) if needs_transcode(&item) => timing.transcode_start_ms,
                        _ => timing.direct_start_ms,
                    };
                    Some(until.saturating_sub_unsigned(cost))
                }
                _ => Some(until),
            },
            (_, until) => until,
        };
    }
}

impl Following {
    fn new(channel: u32) -> Self {
        Self {
            channel,
            events: Vec::new(),
            read_until_ms: i64::MIN,
            stale: false,
            on: On::Unknown,
            until_ms: None,
            fire_ms: None,
            queued: None,
            awaiting: None,
            recheck_ms: i64::MIN,
        }
    }

    /// Reads the channel's events from `now_ms` on. A read that fails keeps
    /// the events read before and is tried again a minute later.
    fn refresh(&mut self, now_ms: i64, store: &Store) {
        self.stale = false;
        self.recheck_ms = now_ms.saturating_add(RECHECK_MS);
        let to_ms = now_ms.saturating_add(READ_SPAN_MS);
        match store.events_between(self.channel, now_ms, to_ms) {
            Ok(events) => {
                self.events = events;
                self.read_until_ms = to_ms;
            }
            Err(e) => {
                eprintln!(
                    "surfcult: channel {}: cannot read the schedule: {e}",
                    self.channel
                );
                self.read_until_ms = now_ms
                    .saturating_add(REFRESH_AHEAD_MS)
                    .saturating_add(RECHECK_MS);
            }
        }
    }

    /// Puts `event` on screen at media position `offset_ms`, replacing what
    /// is there.
    fn show(
        &mut self,
        event: Event,
        offset_ms: u64,
        now_ms: i64,
        cx: &mut Context<'_, impl Player>,
    ) {
        let previous = std::mem::replace(&mut self.on, On::Event(event.clone()));
        self.until_ms = Some(event.end_ms);
        let channel = self.channel;
        if event.kind == EventKind::Program {
            if let On::Event(before) = &previous
                && continues(before, &event)
            {
                return;
            }
            cx.streams.stop_all();
            match cx.player.play_program(&event.media_id, offset_ms) {
                Ok(()) => {
                    self.awaiting = Some(event.media_id.clone());
                    record(cx.store, channel, &event.media_id, now_ms, "played");
                }
                Err(e) => {
                    eprintln!(
                        "surfcult: channel {channel}: cannot play {}: {e}",
                        event.media_id
                    );
                    show_standby(channel, cx.player);
                    record(cx.store, channel, &event.media_id, now_ms, "failed");
                }
            }
            return;
        }
        cx.streams.stop_all();
        let played = match clip_for(cx.clips, &event.media_id) {
            None => Err(format!(
                "the clip {} is not in the scanned clip pools",
                event.media_id
            )),
            Some(clip) if shorter_than(clip, &event) => cx
                .player
                .loop_clip(&clip.path)
                .map(|()| clip.duration)
                .map_err(|e| format!("cannot play the clip {}: {e}", event.media_id)),
            Some(clip) => cx
                .player
                .play_clip(&clip.path)
                .map(|()| clip.duration)
                .map_err(|e| format!("cannot play the clip {}: {e}", event.media_id)),
        };
        match played {
            Ok(duration) => {
                record(cx.store, channel, &event.media_id, now_ms, "played");
                self.queue_successor(&event, now_ms, duration, cx);
            }
            Err(why) => {
                eprintln!("surfcult: channel {channel}: {why}");
                show_standby(channel, cx.player);
                record(cx.store, channel, &event.media_id, now_ms, "failed");
            }
        }
    }

    /// Queues the event after the clip event `event` behind the clip, which
    /// started at `started_ms` and lasts `duration`, when the clip ends at
    /// the event's end and the next event follows without a gap. Otherwise
    /// the next item replaces the clip at the boundary.
    fn queue_successor(
        &mut self,
        event: &Event,
        started_ms: i64,
        duration: Duration,
        cx: &mut Context<'_, impl Player>,
    ) {
        let duration_ms = i64::try_from(duration.as_millis()).unwrap_or(i64::MAX);
        let clip_end = started_ms.saturating_add(duration_ms);
        if clip_end.abs_diff(event.end_ms) > QUEUE_TOLERANCE_MS.unsigned_abs() {
            return;
        }
        let Lookup::On { event: next, .. } = lookup(&self.events, event.end_ms) else {
            return;
        };
        if next.start_ms != event.end_ms {
            return;
        }
        let next = next.clone();
        let queued = if next.kind == EventKind::Program {
            cx.player
                .queue_program(&next.media_id, next.media_offset_ms)
                .map(|()| self.awaiting = Some(next.media_id.clone()))
        } else {
            match clip_for(cx.clips, &next.media_id) {
                Some(clip) if shorter_than(clip, &next) => cx.player.queue_looping_clip(&clip.path),
                Some(clip) => cx.player.queue_clip(&clip.path),
                // The boundary shows the card and logs why.
                None => return,
            }
        };
        match queued {
            Ok(()) => self.queued = Some(next),
            Err(e) => eprintln!(
                "surfcult: channel {}: cannot queue {}; it starts at the boundary instead: {e}",
                self.channel, next.media_id
            ),
        }
    }
}

/// Whether program event `next` carries on the media of `before` where it
/// stopped, so the file on screen just keeps playing.
fn continues(before: &Event, next: &Event) -> bool {
    let length = u64::try_from(before.end_ms - before.start_ms).unwrap_or(0);
    before.kind == EventKind::Program
        && before.media_id == next.media_id
        && before.end_ms == next.start_ms
        && before.media_offset_ms.saturating_add(length) == next.media_offset_ms
}

/// The scanned clip whose id is `media_id`.
fn clip_for<'a>(clips: &'a ClipLibrary, media_id: &MediaId) -> Option<&'a Clip> {
    clips
        .pools()
        .iter()
        .flat_map(|pool| pool.clips())
        .find(|clip| clip_media_id(clip).as_ref() == Some(media_id))
}

fn show_standby(channel: u32, player: &mut impl Player) {
    if let Err(e) = player.show_standby() {
        eprintln!("surfcult: channel {channel}: cannot show the stand-by card: {e}");
    }
}

fn record(store: &mut Store, channel: u32, media_id: &MediaId, at_ms: i64, outcome: &str) {
    if let Err(e) = store.record_playback(channel, media_id, at_ms, outcome) {
        eprintln!("surfcult: channel {channel}: cannot record playback history: {e}");
    }
}

/// Whether `clip` ends before `event` does, by more than the queue
/// tolerance: a pad or gap event that repeats one clip. Such a clip loops
/// until the event's end replaces it, so the screen never goes empty.
fn shorter_than(clip: &Clip, event: &Event) -> bool {
    let clip_ms = i64::try_from(clip.duration.as_millis()).unwrap_or(i64::MAX);
    event.end_ms.saturating_sub(event.start_ms) > clip_ms.saturating_add(QUEUE_TOLERANCE_MS)
}
