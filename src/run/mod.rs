//! The TV: the loop that connects the remote, the tuner, the schedule, the
//! player and the banner. `surfcult run` drives it (see [`app::run`]);
//! `docs/reference/run.md` describes it for operators.
//!
//! [`Tv`] holds everything except the [`Store`], which the caller owns and
//! passes to each call, so the loop is the store's only writer apart from
//! the sync results it applies. One [`Tv::step`] does what is due at one
//! moment:
//!
//! 1. reads the keys pressed since the last step ([`Input`]);
//! 2. reads the player's events, hands them to the tuner and the driver, and
//!    ticks the tuner;
//! 3. when the tuner's sequence has settled, on a program or on the stand-by
//!    card, hands the screen to the playback driver and shows the banner;
//! 4. lets the driver move the screen to the next event when one starts;
//! 5. clears the banner when its time is up;
//! 6. keeps every channel's schedule filled when that is due.
//!
//! Nothing here reads the clock or sleeps: every call takes `now_ms`, so
//! tests drive the loop with a virtual clock, a
//! [`FakePlayer`](crate::player::FakePlayer) and a [`FakeInput`](crate::input::FakeInput).
//!
//! # Schedules
//!
//! Each channel's sources are resolved into program pools from the library
//! cache in the store and its schedule is extended at startup, after every
//! sync that was written, and every hour, but only while the clock is
//! trusted (checked again every minute while it is not). A sync that changed
//! the runtime of an item in a channel's pools regenerates that channel
//! instead, which keeps the next 30 minutes. A channel with a source that
//! cannot be resolved (nothing synced yet, or a name the library does not
//! have) is logged with the reason and skipped; it shows the stand-by card
//! once its stored events run out, and the other channels keep working.

pub mod app;
mod clock;
mod driver;
mod now;
mod streams;
#[cfg(test)]
mod tests;

use std::collections::HashMap;

use crate::clips::{Clip, ClipLibrary};
use crate::config::{Channel, Config, Source};
use crate::input::{Input, Key};
use crate::jellyfin::{JellyfinError, SyncReport};
use crate::library::{MediaItem, resolve_pool};
use crate::overlay::{self, DEFAULT_BANNER_MS};
use crate::player::{Player, PlayerEvent};
use crate::schedule::{self, Extended};
use crate::store::Store;
use crate::tuner::{State, Timing, Tuner};

pub use clock::{CLOCK_ENV, trusted as clock_trusted};
use driver::{Context, Driver};
pub use now::now_lines;
pub use streams::{JellyfinResolver, Streams, Transcodes, jellyfin as jellyfin_streams};

/// A channel's program pools, one per source, as `schedule::extend` takes
/// them.
type Pools = Vec<(Source, Vec<MediaItem>)>;

/// The loop's durations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// The tune-in sequence's durations, also used for the driver's start-up
    /// costs and late tolerance.
    pub timing: Timing,
    /// How long the banner stays up.
    pub banner_ms: i64,
    /// How often schedules are extended when nothing else asks for it.
    pub upkeep_every_ms: i64,
    /// How often an untrusted clock is checked again.
    pub clock_retry_ms: i64,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            timing: Timing::default(),
            banner_ms: DEFAULT_BANNER_MS,
            upkeep_every_ms: 60 * 60 * 1000,
            clock_retry_ms: 60 * 1000,
        }
    }
}

/// What schedule upkeep remembers between runs.
#[derive(Default)]
struct Upkeep {
    due_ms: i64,
    /// The pools each channel was last scheduled from, to see what a sync
    /// changed.
    pools: HashMap<u32, Pools>,
    /// The last problem logged for each channel, so it is logged once.
    problems: HashMap<u32, String>,
    clock_logged: bool,
}

/// The TV. See the module documentation.
pub struct Tv<P, I> {
    config: Config,
    /// Channel numbers in ascending order, for Up and Down.
    numbers: Vec<u32>,
    tuner: Tuner<P>,
    input: I,
    driver: Driver,
    clips: ClipLibrary,
    streams: Box<dyn Streams>,
    clock_trusted: Box<dyn FnMut() -> bool>,
    settings: Settings,
    channel: Option<u32>,
    banner_pending: bool,
    banner_until_ms: Option<i64>,
    upkeep: Upkeep,
    quit: bool,
}

impl<P: Player, I: Input> Tv<P, I> {
    /// A TV that plays `config`'s channels on `player`, reads keys from
    /// `input` and takes tune-in clips from the scanned pool
    /// `config.tune_in_pool` in `clips`. `streams` stops the transcodes of
    /// programs that leave the screen; `clock_trusted` says whether
    /// schedules may be written ([`clock_trusted`] in production).
    pub fn new(
        config: Config,
        player: P,
        input: I,
        clips: ClipLibrary,
        streams: Box<dyn Streams>,
        clock_trusted: Box<dyn FnMut() -> bool>,
        settings: Settings,
    ) -> Self {
        let tune_in: Vec<Clip> = clips
            .pool(&config.tune_in_pool)
            .map(|pool| pool.clips().to_vec())
            .unwrap_or_default();
        if tune_in.is_empty() {
            eprintln!(
                "surfcult: the tune-in pool {:?} has no clips; a channel change goes straight to the program",
                config.tune_in_pool
            );
        }
        let mut numbers: Vec<u32> = config.channels.iter().map(|c| c.number).collect();
        numbers.sort_unstable();
        Self {
            config,
            numbers,
            tuner: Tuner::new(player, tune_in, settings.timing),
            input,
            driver: Driver::new(settings.timing),
            clips,
            streams,
            clock_trusted,
            settings,
            channel: None,
            banner_pending: false,
            banner_until_ms: None,
            upkeep: Upkeep::default(),
            quit: false,
        }
    }

    /// Shows the stand-by card, fills the schedules and tunes the lowest
    /// channel.
    pub fn start(&mut self, now_ms: i64, store: &mut Store) {
        if let Err(e) = self.tuner.player_mut().show_standby() {
            eprintln!("surfcult: cannot show the stand-by card: {e}");
        }
        self.keep_schedules(now_ms, store, false);
        if let Some(&first) = self.numbers.first() {
            self.tune(first, now_ms);
        } else {
            eprintln!("surfcult: the config has no channels");
        }
    }

    /// Does what is due at `now_ms`. See the module documentation.
    pub fn step(&mut self, now_ms: i64, store: &mut Store) {
        for key in self.input.poll(now_ms) {
            self.key(key, now_ms);
        }
        self.pump(now_ms, store);
        self.settle(now_ms, store);
        let mut cx = Context {
            player: self.tuner.player_mut(),
            store,
            clips: &self.clips,
            streams: self.streams.as_mut(),
        };
        self.driver.step(now_ms, &mut cx);
        self.banner(now_ms, store);
        if now_ms >= self.upkeep.due_ms {
            self.keep_schedules(now_ms, store, false);
        }
    }

    /// The sync worker's newest result was applied to the store at `now_ms`.
    /// A written library extends every schedule, or regenerates a channel
    /// whose items changed runtime. A failed sync is logged; the cache keeps
    /// the last good library.
    pub fn synced(
        &mut self,
        now_ms: i64,
        store: &mut Store,
        result: Result<SyncReport, JellyfinError>,
    ) {
        match result {
            Ok(report) => {
                eprintln!(
                    "surfcult: library synced: {} items, {} collections, {} skipped",
                    report.items, report.collections, report.skipped
                );
                self.keep_schedules(now_ms, store, true);
            }
            Err(e) => {
                eprintln!("surfcult: the library sync failed; keeping the cached library: {e}")
            }
        }
    }

    /// Whether the viewer asked to quit.
    pub fn quit_requested(&self) -> bool {
        self.quit
    }

    /// Stops the transcodes and the player. Dropping an `MpvPlayer` then
    /// ends MPV.
    pub fn shut_down(&mut self) {
        self.streams.stop_all();
        if let Err(e) = self.tuner.stop() {
            eprintln!("surfcult: cannot stop the player: {e}");
        }
    }

    /// The channel tuned last.
    pub fn channel(&self) -> Option<u32> {
        self.channel
    }

    pub fn player(&self) -> &P {
        self.tuner.player()
    }

    pub fn player_mut(&mut self) -> &mut P {
        self.tuner.player_mut()
    }

    pub fn input_mut(&mut self) -> &mut I {
        &mut self.input
    }

    pub fn tuner_state(&self) -> &State {
        self.tuner.state()
    }

    fn key(&mut self, key: Key, now_ms: i64) {
        match key {
            Key::Next | Key::Previous => {
                if let Some(target) = self.neighbour(key == Key::Next) {
                    self.tune(target, now_ms);
                }
            }
            Key::Banner => self.banner_pending = self.driver.following(),
            Key::Quit => self.quit = true,
        }
    }

    /// The channel after (or before) the current one, wrapping around.
    fn neighbour(&self, up: bool) -> Option<u32> {
        let count = self.numbers.len();
        let at = self
            .channel
            .and_then(|c| self.numbers.iter().position(|&n| n == c));
        let index = match (at, up) {
            (_, _) if count == 0 => return None,
            (None, _) => 0,
            (Some(i), true) => (i + 1) % count,
            (Some(i), false) => (i + count - 1) % count,
        };
        Some(self.numbers[index])
    }

    fn tune(&mut self, channel: u32, now_ms: i64) {
        self.streams.stop_all();
        self.driver.leave();
        self.banner_pending = false;
        // Static replaces the screen, and the banner with it.
        self.banner_until_ms = None;
        self.channel = Some(channel);
        self.tuner.tune(channel, now_ms);
    }

    /// Hands the player's events to the tuner and the driver, then ticks the
    /// tuner.
    fn pump(&mut self, now_ms: i64, store: &Store) {
        while let Some(event) = self.tuner.player_mut().poll_event() {
            match event {
                PlayerEvent::Restarted => {
                    eprintln!("surfcult: the player was restarted; tuning the channel again");
                    self.streams.stop_all();
                    self.driver.leave();
                    self.banner_pending = false;
                    self.banner_until_ms = None;
                }
                PlayerEvent::ProgramStarted { position_ms } if self.driver.following() => {
                    self.driver
                        .program_started(now_ms, position_ms, self.tuner.player_mut());
                }
                _ => {}
            }
            self.tuner.handle(event, now_ms);
            if event == PlayerEvent::Restarted
                && *self.tuner.state() == State::Idle
                && let Err(e) = self.tuner.player_mut().show_standby()
            {
                eprintln!("surfcult: cannot show the stand-by card: {e}");
            }
        }
        self.tuner.tick(now_ms, store, store);
    }

    /// When the tuner's sequence has settled, gives the screen to the driver
    /// and asks for the banner.
    fn settle(&mut self, now_ms: i64, store: &mut Store) {
        if self.driver.following() {
            return;
        }
        match self.tuner.state().clone() {
            State::Live { channel } => {
                self.driver.join_live(channel, now_ms, store);
                self.banner_pending = true;
            }
            State::Failed { channel, reason } => {
                eprintln!("surfcult: channel {channel}: {reason}");
                if !reason.shows_standby()
                    && let Err(e) = self.tuner.player_mut().show_standby()
                {
                    eprintln!("surfcult: channel {channel}: cannot show the stand-by card: {e}");
                }
                self.driver.join_standby(channel, now_ms, store);
                self.banner_pending = true;
            }
            State::Idle
            | State::Static { .. }
            | State::ClipPlaying { .. }
            | State::AwaitingProgram { .. } => {}
        }
    }

    /// Shows the banner when it was asked for and clears it when its time is
    /// up.
    fn banner(&mut self, now_ms: i64, store: &Store) {
        if self.banner_pending {
            self.banner_pending = false;
            if let Some(channel) = self.channel.and_then(|n| self.channel_config(n)) {
                let banner = overlay::banner(channel, store, now_ms, self.config.timezone);
                match self.tuner.player_mut().show_banner(&banner) {
                    Ok(()) => {
                        self.banner_until_ms = Some(now_ms.saturating_add(self.settings.banner_ms))
                    }
                    Err(e) => eprintln!("surfcult: cannot show the banner: {e}"),
                }
            }
        }
        if self.banner_until_ms.is_some_and(|until| now_ms >= until) {
            self.banner_until_ms = None;
            if let Err(e) = self.tuner.player_mut().clear_banner() {
                eprintln!("surfcult: cannot clear the banner: {e}");
            }
        }
    }

    fn channel_config(&self, number: u32) -> Option<&Channel> {
        self.config.channels.iter().find(|c| c.number == number)
    }

    /// Extends every channel's schedule, or regenerates a channel whose
    /// items changed runtime when `after_sync` is set. Does nothing but wait
    /// while the clock is not trusted.
    fn keep_schedules(&mut self, now_ms: i64, store: &mut Store, after_sync: bool) {
        if !(self.clock_trusted)() {
            if !self.upkeep.clock_logged {
                eprintln!(
                    "surfcult: the clock is not synchronized; schedules are not written until it is"
                );
                self.upkeep.clock_logged = true;
            }
            self.upkeep.due_ms = now_ms.saturating_add(self.settings.clock_retry_ms);
            return;
        }
        self.upkeep.clock_logged = false;
        self.upkeep.due_ms = now_ms.saturating_add(self.settings.upkeep_every_ms);
        let pools_now = self.clips.pools();
        for channel in &self.config.channels {
            let n = channel.number;
            let pools = match resolve_pools(channel, store) {
                Ok(pools) => pools,
                Err(why) => {
                    if self.upkeep.problems.get(&n) != Some(&why) {
                        eprintln!("surfcult: {why}; its schedule is not extended");
                        self.upkeep.problems.insert(n, why);
                    }
                    continue;
                }
            };
            let changed = after_sync
                && self
                    .upkeep
                    .pools
                    .get(&n)
                    .is_some_and(|before| runtimes_changed(before, &pools));
            let tz = self.config.timezone;
            let result = if changed {
                eprintln!("surfcult: channel {n}: a runtime changed; regenerating its schedule");
                schedule::regenerate(store, channel, &pools, pools_now, tz, now_ms, true)
            } else {
                schedule::extend(store, channel, &pools, pools_now, tz, now_ms, true)
            };
            match result {
                Ok(Extended::Generated { .. } | Extended::UpToDate { .. }) => {
                    self.upkeep.problems.remove(&n);
                }
                Ok(Extended::ClockNotTrusted) => {}
                Err(e) => {
                    let why = format!("channel {n}: cannot extend the schedule: {e}");
                    if self.upkeep.problems.get(&n) != Some(&why) {
                        eprintln!("surfcult: {why}");
                        self.upkeep.problems.insert(n, why);
                    }
                }
            }
            self.upkeep.pools.insert(n, pools);
        }
        self.driver.invalidate();
    }
}

/// The program pool of each of `channel`'s sources, or every reason one
/// cannot be resolved, joined.
fn resolve_pools(channel: &Channel, store: &Store) -> Result<Pools, String> {
    let mut pools = Vec::new();
    let mut problems = Vec::new();
    for source in channel.plan.sources() {
        match resolve_pool(channel.number, source, store) {
            Ok(items) => pools.push((source.clone(), items)),
            Err(e) => problems.push(e.to_string()),
        }
    }
    if problems.is_empty() {
        Ok(pools)
    } else {
        Err(problems.join("; "))
    }
}

/// Whether an item in both `before` and `after` has a different runtime.
fn runtimes_changed(before: &Pools, after: &Pools) -> bool {
    let old: HashMap<_, u64> = before
        .iter()
        .flat_map(|(_, items)| items)
        .map(|item| (&item.id, item.runtime_ms))
        .collect();
    after
        .iter()
        .flat_map(|(_, items)| items)
        .any(|item| old.get(&item.id).is_some_and(|&ms| ms != item.runtime_ms))
}
