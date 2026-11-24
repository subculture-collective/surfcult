//! The tuner: what happens on screen when the viewer changes channel.
//!
//! # The sequence
//!
//! Times are in milliseconds after the key press. `L` is the length of the
//! tune-in clip.
//!
//! ```text
//! t=0        tune(channel)        player: show static
//! t=500      tick                 player: play the clip
//!                                 player: queue the program for time 500+L
//!                                         at its offset, behind the clip
//!                                 (the player opens it early while the clip plays)
//! t=500+L    clip ends            the queued program starts
//! t=500+L+d  program started      if it started late, one seek to the live position
//! ```
//!
//! The program is queued rather than preloaded paused, because one MPV process
//! cannot hold a second file paused (ADR 0001, decision 5). The program and
//! its offset come from [`program_from`](crate::timeline::program_from) at the
//! moment the clip ends, not when the key was pressed. If a break is on then,
//! the program after the break is queued at its event's `media_offset_ms`.
//!
//! # No clock, no I/O of its own
//!
//! [`Tuner`] never reads the system clock and never sleeps. Every method that
//! depends on time takes `now_ms`, in Unix epoch milliseconds, and a caller
//! drives it with four things:
//!
//! - [`Tuner::tune`] when the viewer picks a channel;
//! - [`Tuner::tick`] when the time returned by [`Tuner::next_wakeup`] arrives;
//! - [`Tuner::clip_ended`] and [`Tuner::program_started`] when the player says
//!   so. [`Tuner::pump`] reads those from [`Player::poll_event`] and ticks.
//!
//! The tuner owns its [`Player`], the screen. It does not own the schedule or
//! the library: [`Tuner::tick`] and [`Tuner::pump`] take a [`Store`] to read
//! the channel's events and a [`MediaSource`] to look up each program's
//! codecs. In production the caller passes the same `&store` for both and
//! keeps ownership of it, so it can extend schedules and sync the library
//! between calls. Tests use [`FakePlayer`](crate::player::FakePlayer), an
//! in-memory store and a
//! [`FakeMediaSource`](crate::library::FakeMediaSource), and move a virtual
//! clock by hand.
//!
//! [`Tuner::state`] says where the sequence is. A caller never has to wait for
//! the program to start: a slow transcode just keeps the state at
//! [`State::AwaitingProgram`]. Giving up on a program that never starts is
//! left to failure handling (issue #34).
//!
//! # Which clip
//!
//! The clip is chosen from the tune-in pool when the clip starts. It is the
//! shortest clip at least as long as the expected start-up cost of the
//! program, or the longest clip if none is that long. The cost is
//! [`Timing::direct_start_ms`] (about 1 s) when the item direct-plays and
//! [`Timing::transcode_start_ms`] (about 7 s) when
//! [`needs_transcode`](crate::library::profile::needs_transcode) says Jellyfin
//! must transcode it (ADR 0001).
//!
//! The program depends on the clip's length and the clip depends on the
//! program, so the tuner tries twice. It plans with the direct-play cost. If
//! the program found that way needs a transcode, it plans again with the
//! transcode cost and keeps that plan, even if the program found the second
//! time would have direct-played. A longer clip costs nothing but a few
//! seconds of tune-in.
//!
//! # When the clip ends inside a break
//!
//! If a break, filler or ident event is on at the time the clip ends, the clip
//! stands in for the break: no second clip is played. The tuner queues the
//! first program event after the break (a break may be several events in a
//! row) at the offset where that event starts.
//! [`timeline::program_from`](crate::timeline::program_from) owns this rule,
//! so the guide can ask the same question. That program starts when the clip
//! ends, which is earlier than the schedule says, by at most the time left in
//! the break. The tuner does not correct this: it only corrects a program that
//! is behind. The next event boundary puts the screen back on the schedule.
//!
//! A filler stretch can be far longer than a break: a blocks channel plays its
//! filler pool through time no block covers. If the program after the run
//! starts more than [`Timing::max_early_ms`] (10 minutes) after the clip
//! ends, the tuner does not queue it hours early. It leaves static up with
//! [`Reason::NoProgramSoon`]. The tuner cannot play filler clips itself yet.
//! A stretch that runs past the hour of schedule the tuner reads has no
//! program in view at all and ends on [`Reason::Gap`] with no next start.
//!
//! # A late start
//!
//! When the player reports the program started, the tuner looks up the live
//! position for that moment. If the program is behind it by more than
//! [`Timing::late_tolerance_ms`] (500 ms), the tuner issues one seek to the
//! live position. It never seeks a second time, never seeks backwards, and
//! does not seek when the live event is no longer that program (a break, a gap
//! or another item).
//!
//! # Cancelling
//!
//! A second [`Tuner::tune`] replaces the sequence in progress. It shows static,
//! which also clears the player's queue, and drops the plan, so nothing from
//! the first sequence is issued afterwards. A [`Player`] must not report the
//! end of a clip that a command replaced; see its documentation.
//!
//! # When there is nothing to show
//!
//! The tuner never panics. It leaves static on screen and sets
//! [`State::Failed`] with a [`Reason`] when
//!
//! - the tune-in pool has no clip ([`Reason::NoTuneInClip`]);
//! - the schedule has no events from now to an hour after the clip ends
//!   ([`Reason::NoEvents`]);
//! - nothing is scheduled when the clip ends, or nothing follows the break
//!   ([`Reason::Gap`]);
//! - a break, filler or ident run is on when the clip ends and the next
//!   program starts more than [`Timing::max_early_ms`] later
//!   ([`Reason::NoProgramSoon`]);
//! - the schedule or the media source fails ([`Reason::Schedule`],
//!   [`Reason::Media`]);
//! - the player rejects a command ([`Reason::Player`]).
//!
//! Static stays up until the next [`Tuner::tune`]. The tuner does not retry.
//!
//! # When the player restarts
//!
//! If the player reports [`PlayerEvent::Restarted`] (MPV died and its
//! watchdog started a new one), the tuner tunes the current channel again, as
//! if the viewer had picked it: static, a tune-in clip, then the program at
//! its live position. This also applies after a failure, since a dead MPV is
//! a common reason for one. The schedule lives in the store, so nothing about
//! it is lost.

mod plan;
#[cfg(test)]
mod tests;

use std::fmt;

use crate::clips::Clip;
use crate::library::profile::needs_transcode;
use crate::library::{MediaId, MediaSource};
use crate::player::{Player, PlayerError, PlayerEvent};
use crate::store::{Store, StoreError};
use crate::timeline::{Event, EventKind, Lookup, lookup};

use plan::{Target, choose_clip, millis, program_at};

/// How far past the end of the clip the tuner reads the schedule. It covers a
/// whole break after the clip and the live-position lookup when the program
/// starts.
const SCHEDULE_WINDOW_MS: i64 = 60 * 60 * 1000;

/// The durations the sequence uses. [`Timing::default`] is what the design
/// and ADR 0001 say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    /// How long static shows before the clip starts.
    pub static_ms: i64,
    /// Expected time from queueing to picture for a program that direct-plays.
    pub direct_start_ms: u64,
    /// The same for a program Jellyfin must transcode.
    pub transcode_start_ms: u64,
    /// How far behind the live position a program may start without a seek.
    pub late_tolerance_ms: u64,
    /// How long before its scheduled start a program after a break may be
    /// queued. A longer filler stretch is not treated as a break.
    pub max_early_ms: u64,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            static_ms: 500,
            direct_start_ms: 1_000,
            transcode_start_ms: 7_000,
            late_tolerance_ms: 500,
            max_early_ms: 10 * 60 * 1000,
        }
    }
}

/// Why a sequence ended on static instead of a program.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Reason {
    /// The tune-in pool has no clip.
    NoTuneInClip,
    /// The schedule has no events in the window.
    NoEvents,
    /// Nothing is scheduled when the clip ends, or nothing follows the break
    /// the clip stands in for. `next_start_ms` is when the next event starts,
    /// if the schedule has one.
    Gap { next_start_ms: Option<i64> },
    /// A break, filler or ident is on when the clip ends and the program
    /// after it starts at `next_program_ms`, more than
    /// [`Timing::max_early_ms`] later.
    NoProgramSoon { next_program_ms: i64 },
    /// The schedule could not be read.
    Schedule(StoreError),
    /// The media source could not describe the program.
    Media(String),
    /// The player rejected a command.
    Player(String),
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoTuneInClip => f.write_str("the tune-in pool has no clip"),
            Self::NoEvents => f.write_str("the channel has no scheduled events"),
            Self::Gap {
                next_start_ms: Some(at),
            } => write!(f, "nothing is scheduled until {at}"),
            Self::Gap {
                next_start_ms: None,
            } => f.write_str("nothing is scheduled"),
            Self::NoProgramSoon { next_program_ms } => {
                write!(f, "no program starts until {next_program_ms}")
            }
            Self::Schedule(error) => write!(f, "cannot read the schedule: {error}"),
            Self::Media(why) => write!(f, "cannot look up the program: {why}"),
            Self::Player(why) => write!(f, "the player failed: {why}"),
        }
    }
}

/// Where the tuner is in the sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    /// Nothing has been tuned, or the tuner was stopped.
    Idle,
    /// Static is on screen. The clip starts at `until_ms`.
    Static { channel: u32, until_ms: i64 },
    /// The tune-in clip is playing and the program is queued behind it.
    ClipPlaying { channel: u32 },
    /// The clip has ended. The program has not reported that it started.
    AwaitingProgram { channel: u32 },
    /// The program started. The sequence is over.
    Live { channel: u32 },
    /// The sequence ended on static, for this reason.
    Failed { channel: u32, reason: Reason },
}

/// What the tuner knows about the program it queued.
struct Plan {
    /// The channel's events from the start of the clip on.
    events: Vec<Event>,
    media_id: MediaId,
}

/// The result of planning a clip, before anything is sent to the player.
struct Planned {
    clip: Clip,
    events: Vec<Event>,
    target: Target,
    needs_transcode: bool,
}

pub struct Tuner<P> {
    player: P,
    tune_in: Vec<Clip>,
    timing: Timing,
    state: State,
    plan: Option<Plan>,
}

impl<P: Player> Tuner<P> {
    /// A tuner that picks tune-in clips from `tune_in`, which is usually
    /// [`Pool::clips`](crate::clips::Pool::clips) of the configured tune-in
    /// pool.
    pub fn new(player: P, tune_in: Vec<Clip>, timing: Timing) -> Self {
        Self {
            player,
            tune_in,
            timing,
            state: State::Idle,
            plan: None,
        }
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn player(&self) -> &P {
        &self.player
    }

    pub fn player_mut(&mut self) -> &mut P {
        &mut self.player
    }

    /// When [`Tuner::tick`] next has something to do, if it does.
    pub fn next_wakeup(&self) -> Option<i64> {
        match self.state {
            State::Static { until_ms, .. } => Some(until_ms),
            _ => None,
        }
    }

    /// The viewer picked `channel` at `now_ms`. Shows static and starts the
    /// sequence. A sequence already running is cancelled.
    pub fn tune(&mut self, channel: u32, now_ms: i64) {
        self.plan = None;
        self.state = match self.player.show_static() {
            Ok(()) => State::Static {
                channel,
                until_ms: now_ms.saturating_add(self.timing.static_ms),
            },
            Err(e) => failed(channel, e),
        };
    }

    /// Does what is due at `now_ms`: starts the clip once static has shown for
    /// long enough, reading the channel's events from `store` and the
    /// program's codecs from `media`. Calling it early or with nothing due
    /// does nothing.
    pub fn tick(&mut self, now_ms: i64, store: &Store, media: &impl MediaSource) {
        if let State::Static { channel, until_ms } = self.state
            && now_ms >= until_ms
        {
            self.state = match self.start_clip(channel, now_ms, store, media) {
                Ok(()) => State::ClipPlaying { channel },
                Err(reason) => State::Failed { channel, reason },
            };
        }
    }

    /// The tune-in clip reached its end. Ignored unless a clip is playing.
    pub fn clip_ended(&mut self) {
        if let State::ClipPlaying { channel } = self.state {
            self.state = State::AwaitingProgram { channel };
        }
    }

    /// The queued program is on screen at `position_ms`. If it is behind the
    /// live position by more than the tolerance, seeks to the live position
    /// once. Ignored unless a program is expected.
    pub fn program_started(&mut self, now_ms: i64, position_ms: u64) {
        let (State::ClipPlaying { channel } | State::AwaitingProgram { channel }) = self.state
        else {
            return;
        };
        let Some(plan) = self.plan.take() else {
            return;
        };
        let live = match lookup(&plan.events, now_ms) {
            Lookup::On {
                event, offset_ms, ..
            } if event.kind == EventKind::Program && event.media_id == plan.media_id => {
                Some(offset_ms)
            }
            _ => None,
        };
        let seek =
            live.filter(|live| *live > position_ms.saturating_add(self.timing.late_tolerance_ms));
        self.state = match seek.map(|live| self.player.seek(live)) {
            Some(Err(e)) => failed(channel, e),
            Some(Ok(())) | None => State::Live { channel },
        };
    }

    /// Passes one player event on to the matching method.
    pub fn handle(&mut self, event: PlayerEvent, now_ms: i64) {
        match event {
            PlayerEvent::ClipEnded => self.clip_ended(),
            PlayerEvent::ProgramStarted { position_ms } => {
                self.program_started(now_ms, position_ms)
            }
            PlayerEvent::Restarted => self.restarted(now_ms),
        }
    }

    /// The player lost everything on screen. Tunes the channel again from
    /// the start of the sequence, so the program comes back at its live
    /// position; the schedule is in the store and is not touched. Ignored
    /// when nothing is tuned.
    pub fn restarted(&mut self, now_ms: i64) {
        let channel = match &self.state {
            State::Idle => return,
            State::Static { channel, .. }
            | State::ClipPlaying { channel }
            | State::AwaitingProgram { channel }
            | State::Live { channel }
            | State::Failed { channel, .. } => *channel,
        };
        self.tune(channel, now_ms);
    }

    /// Hands every pending player event to [`Tuner::handle`], then ticks.
    pub fn pump(&mut self, now_ms: i64, store: &Store, media: &impl MediaSource) {
        while let Some(event) = self.player.poll_event() {
            self.handle(event, now_ms);
        }
        self.tick(now_ms, store, media);
    }

    /// Cancels any sequence and stops the player.
    pub fn stop(&mut self) -> Result<(), PlayerError> {
        self.plan = None;
        self.state = State::Idle;
        self.player.stop()
    }

    fn start_clip(
        &mut self,
        channel: u32,
        now_ms: i64,
        store: &Store,
        media: &impl MediaSource,
    ) -> Result<(), Reason> {
        let mut planned =
            self.plan_clip(channel, now_ms, self.timing.direct_start_ms, store, media)?;
        if planned.needs_transcode {
            planned = self.plan_clip(
                channel,
                now_ms,
                self.timing.transcode_start_ms,
                store,
                media,
            )?;
        }
        let Planned {
            clip,
            events,
            target,
            ..
        } = planned;
        self.player
            .play_clip(&clip.path)
            .map_err(|e| Reason::Player(e.to_string()))?;
        self.player
            .queue_program(&target.media_id, target.offset_ms)
            .map_err(|e| Reason::Player(e.to_string()))?;
        self.plan = Some(Plan {
            events,
            media_id: target.media_id,
        });
        Ok(())
    }

    /// Picks the clip for `start_cost_ms` and the program that goes behind it,
    /// for a clip that starts at `now_ms`. Sends nothing to the player.
    fn plan_clip(
        &self,
        channel: u32,
        now_ms: i64,
        start_cost_ms: u64,
        store: &Store,
        media: &impl MediaSource,
    ) -> Result<Planned, Reason> {
        let clip = choose_clip(&self.tune_in, start_cost_ms).ok_or(Reason::NoTuneInClip)?;
        let ends_at_ms = now_ms.saturating_add_unsigned(millis(clip));
        let events = store
            .events_between(
                channel,
                now_ms,
                ends_at_ms.saturating_add(SCHEDULE_WINDOW_MS),
            )
            .map_err(Reason::Schedule)?;
        let target = program_at(&events, ends_at_ms, self.timing.max_early_ms)?;
        let item = media
            .resolve(&target.media_id)
            .map_err(|e| Reason::Media(e.to_string()))?;
        Ok(Planned {
            clip: clip.clone(),
            events,
            target,
            needs_transcode: needs_transcode(&item),
        })
    }
}

fn failed(channel: u32, error: PlayerError) -> State {
    State::Failed {
        channel,
        reason: Reason::Player(error.to_string()),
    }
}
