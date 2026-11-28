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
//! 5. clears the banner when its time is up, and shows, fades or hides the
//!    SCTV watermark ([`watermark`]);
//! 6. keeps every channel's schedule filled when that is due.
//!
//! Nothing here reads the clock or sleeps: every call takes `now_ms`, so
//! tests drive the loop with a virtual clock, a
//! [`FakePlayer`](crate::player::FakePlayer) and a [`FakeInput`](crate::input::FakeInput).
//!
//! # The clock
//!
//! Until the system clock is trusted ([`clock_trusted`]) the TV shows the
//! clock slate ([`Slate::ClockNotSet`]), tunes nothing and writes no
//! schedule: a Pi without network time may think it is 1970, and both the
//! stored events and a new week would be read at the wrong time. Only the
//! quit key works. The clock is checked again every minute; once it is
//! trusted the TV starts as it would have: schedules are filled and the
//! lowest channel is tuned. A clock that later loses its synchronization
//! does not bring the slate back; the schedules just wait for it.
//!
//! # Schedules
//!
//! Each channel's sources are resolved into program pools from the library
//! cache in the store and its schedule is extended at startup, after every
//! sync that was written, and every hour, but only while the clock is
//! trusted (checked again every minute while it is not). A sync that changed
//! the runtime of an item a channel's schedule plays, or removed one,
//! regenerates that channel instead, which keeps the next 30 minutes, and
//! records each program it replaced for a removed item as a substitution. A
//! channel with a source that cannot be resolved (nothing synced yet, or a
//! name the library does not have) is logged with the reason and skipped; it
//! shows the stand-by card once its stored events run out, and the other
//! channels keep working.

pub mod app;
mod clock;
mod covers;
mod driver;
mod guide;
mod menu;
mod now;
mod streams;
#[cfg(test)]
mod tests;
mod watermark;

use std::collections::hash_map::Entry;
use std::collections::{BTreeSet, HashMap, HashSet};

use guide::{Guide, clock_with_seconds};
use menu::{CRT_SETTING, Menu, crt_name, next_crt};
use watermark::Watermark;

use crate::clips::{Clip, ClipLibrary};
use crate::config::{Channel, Config, Picture, Source};
use crate::input::{Input, Key};
use crate::jellyfin::{JellyfinError, SyncReport};
use crate::library::{LibraryChanges, MediaId, MediaItem, MediaSource, resolve_pool};
use crate::overlay::{self, DEFAULT_BANNER_MS};
use crate::paint::now_playing::NowPlaying;
use crate::paint::watermark::Form;
use crate::paint::{self, Crt, Image};
use crate::player::{Overlay, Player, PlayerEvent, Slate, VideoArea};
use crate::schedule::{self, Extended};
use crate::server::AdminCommand;
use crate::store::Store;
use crate::tuner::{State, Timing, Tuner};

pub use clock::{CLOCK_ENV, trusted as clock_trusted};
pub use covers::{COVER_SIDE, Covers, JellyfinCovers};
use driver::{Context, Driver, ProgramStartContext};
pub use now::{day_lines, now_lines};
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
    /// How long a channel number waits for its next digit.
    pub entry_ms: i64,
    /// The CRT pass on the interface.
    pub crt: Crt,
    /// Whether the SCTV watermark shows.
    pub watermark: bool,
}

/// The overlay layer a music channel's picture is drawn in: the cover card,
/// or the now-playing line over the visualizer. It is under every other.
pub const PICTURE_LAYER: u8 = 0;
/// At most this many covers are kept; past it the kept ones are dropped.
const KEPT_COVERS: usize = 16;
/// The overlay layer the SCTV watermark is drawn in.
pub const WATERMARK_LAYER: u8 = 8;
/// The overlay layer the program guide is drawn in.
pub const GUIDE_LAYER: u8 = 1;
/// The overlay layer of the guide's clock cell, drawn over the guide.
pub const GUIDE_CLOCK_LAYER: u8 = 2;
/// The overlay layer of the guide's channel rows, which scroll.
pub const GUIDE_ROWS_LAYER: u8 = 3;
/// The overlay layer the Picture menu is drawn in.
pub const MENU_LAYER: u8 = 4;

impl Default for Settings {
    fn default() -> Self {
        Self {
            timing: Timing::default(),
            banner_ms: DEFAULT_BANNER_MS,
            upkeep_every_ms: 60 * 60 * 1000,
            clock_retry_ms: 60 * 1000,
            entry_ms: 2000,
            crt: Crt::default(),
            watermark: true,
        }
    }
}

/// What the picture layer shows.
struct PictureShown {
    media: MediaId,
    picture: Picture,
    /// The id the cover was asked for, on a cover card.
    cover: Option<MediaId>,
    /// Whether the card was drawn before its cover arrived.
    waiting: bool,
}

/// What schedule upkeep remembers between runs.
/// How often the loop takes finished background clip probes.
const CLIPS_EVERY_MS: i64 = 5_000;

#[derive(Default)]
struct Upkeep {
    due_ms: i64,
    /// Library changes from syncs that no schedule has followed yet.
    changes: LibraryChanges,
    /// Channels whose settings the web admin changed and whose schedule has
    /// not been regenerated for it yet.
    reconfigured: BTreeSet<u32>,
    /// The last problem logged for each channel, so it is logged once.
    problems: HashMap<u32, String>,
    clock_logged: bool,
    /// Each channel's program pools, resolved once and kept until a sync
    /// writes a new library or the channel's settings change. A music
    /// channel's genres can hold thousands of songs each, and resolving them
    /// at every upkeep would hold up the loop for seconds on the Pi.
    pools: HashMap<u32, Pools>,
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
    /// The channel number being typed: its value, its digits and when it
    /// is tuned if no digit follows.
    entry: Option<(u32, usize, i64)>,
    /// The program guide, while it is open.
    guide: Option<Guide>,
    /// The Picture menu, while it is open.
    menu: Option<Menu>,
    watermark: Watermark,
    /// What the watermark layer shows now: the form and opacity.
    watermark_shown: Option<(Form, f32)>,
    /// Each form drawn at full opacity with the CRT pass, made on first use.
    watermark_bitmaps: HashMap<Form, Overlay>,
    upkeep: Upkeep,
    /// The picture the player was last told to play programs with.
    picture: Option<Picture>,
    /// What the picture layer shows now.
    picture_shown: Option<PictureShown>,
    /// Where covers come from; none in tests unless a test gives one.
    covers: Option<Box<dyn Covers>>,
    /// Covers that arrived, by the id they were asked for.
    cover_images: HashMap<MediaId, Option<Image>>,
    /// Covers asked for that have not arrived.
    covers_asked: HashSet<MediaId>,
    /// When finished clip probes are next taken from the library.
    clips_due_ms: i64,
    /// The clip issues already logged, so each is logged once.
    clip_issues: HashSet<String>,
    /// The clock slate is up until the clock is trusted.
    waiting_for_clock: bool,
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
        if tune_in.is_empty() && clips.pending() == 0 {
            eprintln!(
                "surfcult: the tune-in pool {:?} has no clips; a channel change goes straight to the program",
                config.tune_in_pool
            );
        }
        if clips.pending() > 0 {
            eprintln!("surfcult: probing {} new clips", clips.pending());
        }
        let mut numbers: Vec<u32> = config.channels.iter().map(|c| c.number).collect();
        numbers.sort_unstable();
        let mut tv = Self {
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
            entry: None,
            guide: None,
            menu: None,
            watermark: Watermark::default(),
            watermark_shown: None,
            watermark_bitmaps: HashMap::new(),
            upkeep: Upkeep::default(),
            picture: None,
            picture_shown: None,
            covers: None,
            cover_images: HashMap::new(),
            covers_asked: HashSet::new(),
            clips_due_ms: 0,
            clip_issues: HashSet::new(),
            waiting_for_clock: false,
            quit: false,
        };
        tv.log_clip_issues();
        tv
    }

    /// Takes album covers for the cover card from `covers`. Without them
    /// the card shows the surfcult mark in place of a cover.
    pub fn set_covers(&mut self, covers: Box<dyn Covers>) {
        self.covers = Some(covers);
    }

    /// Shows the stand-by card, fills the schedules and tunes the lowest
    /// channel. While the clock is not trusted, shows the clock slate
    /// instead and does the rest once it is.
    pub fn start(&mut self, now_ms: i64, store: &mut Store) {
        self.load_viewer_settings(store);
        self.save_clip_probes(store);
        if !(self.clock_trusted)() {
            eprintln!(
                "surfcult: the clock is not synchronized; showing the clock slate until it is"
            );
            self.show_clock_slate();
            self.waiting_for_clock = true;
            self.upkeep.due_ms = now_ms.saturating_add(self.settings.clock_retry_ms);
            return;
        }
        if let Err(e) = self.tuner.player_mut().show_standby() {
            eprintln!("surfcult: cannot show the stand-by card: {e}");
        }
        self.begin(now_ms, store);
    }

    /// Fills the schedules and tunes the lowest channel.
    fn begin(&mut self, now_ms: i64, store: &mut Store) {
        self.keep_schedules(now_ms, store);
        if let Some(&first) = self.numbers.first() {
            self.tune(first, now_ms);
        } else {
            eprintln!("surfcult: the config has no channels");
        }
    }

    /// Does what is due at `now_ms`. See the module documentation.
    pub fn step(&mut self, now_ms: i64, store: &mut Store) {
        if self.waiting_for_clock {
            self.wait_for_clock(now_ms, store);
            return;
        }
        for key in self.input.poll(now_ms) {
            self.key(key, now_ms, store);
        }
        if let Some((number, _, due_ms)) = self.entry
            && now_ms >= due_ms
        {
            self.finish_entry(number, now_ms);
        }
        self.pump(now_ms, store);
        self.settle(now_ms, store);
        let mut cx = Context {
            player: self.tuner.player_mut(),
            store,
            clips: &self.clips,
            streams: self.streams.as_mut(),
            filler: filler(&self.config, &self.clips, self.channel),
        };
        self.driver.step(now_ms, &mut cx);
        self.banner(now_ms, store);
        self.draw_guide(now_ms, store);
        self.draw_menu();
        self.show_picture(store);
        self.show_watermark(now_ms);
        if now_ms >= self.clips_due_ms {
            self.clips_due_ms = now_ms.saturating_add(CLIPS_EVERY_MS);
            if self.clips.collect() {
                self.clips_changed(store);
            }
        }
        if now_ms >= self.upkeep.due_ms {
            self.keep_schedules(now_ms, store);
        }
    }

    /// A step while the clock slate is up: only the quit key works, and once
    /// the clock is trusted the TV starts.
    fn wait_for_clock(&mut self, now_ms: i64, store: &mut Store) {
        for key in self.input.poll(now_ms) {
            if key == Key::Quit {
                self.quit = true;
            }
        }
        while let Some(event) = self.tuner.player_mut().poll_event() {
            if event == PlayerEvent::Restarted {
                self.show_clock_slate();
            }
        }
        if now_ms < self.upkeep.due_ms {
            return;
        }
        if (self.clock_trusted)() {
            eprintln!("surfcult: the clock is synchronized; starting");
            self.waiting_for_clock = false;
            self.begin(now_ms, store);
        } else {
            self.upkeep.due_ms = now_ms.saturating_add(self.settings.clock_retry_ms);
        }
    }

    fn show_clock_slate(&mut self) {
        if let Err(e) = self.tuner.player_mut().show_slate(Slate::ClockNotSet) {
            eprintln!("surfcult: cannot show the clock slate: {e}");
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
                    "surfcult: library synced: {} items, {} collections, {} skipped{}",
                    report.items,
                    report.collections,
                    report.skipped,
                    if report.written { "" } else { ", unchanged" }
                );
                if report.written {
                    self.upkeep.pools.clear();
                }
                let changes = report.changes;
                if !changes.is_empty() {
                    eprintln!(
                        "surfcult: library changed: {} renamed, {} with a new runtime, {} gone",
                        changes.renamed.len(),
                        changes.resized.len(),
                        changes.vanished.len()
                    );
                }
                let pending = &mut self.upkeep.changes;
                pending.renamed.extend(changes.renamed);
                pending.resized.extend(changes.resized);
                pending.vanished.extend(changes.vanished);
                // An unchanged library leaves the schedules as they were;
                // the hourly upkeep keeps them filled.
                if report.written || !pending.is_empty() {
                    self.keep_schedules(now_ms, store);
                }
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

    /// The CRT filter in use.
    pub fn crt(&self) -> Crt {
        self.settings.crt
    }

    /// Carries out a request from the web admin, as the remote or the menu
    /// would. A channel the config does not have is ignored.
    pub fn admin(&mut self, command: AdminCommand, now_ms: i64, store: &mut Store) {
        match command {
            AdminCommand::Tune(channel) => {
                if self.numbers.contains(&channel) {
                    self.entry = None;
                    self.tune(channel, now_ms);
                }
            }
            AdminCommand::SetCrt(crt) => self.choose_crt(crt, store),
            AdminCommand::Reconfigure(config) => self.reconfigure(*config, now_ms, store),
        }
    }

    /// Takes the channels of `config`, the file the web admin just wrote,
    /// and regenerates the schedule of each channel whose settings changed
    /// from 30 minutes on ([`schedule::FREEZE_MS`]). The rest of `config` is
    /// the same as the running one; the server checks that.
    fn reconfigure(&mut self, config: Config, now_ms: i64, store: &mut Store) {
        for channel in &config.channels {
            if self.channel_config(channel.number) != Some(channel) {
                eprintln!(
                    "surfcult: channel {} changed in the web admin; regenerating its schedule",
                    channel.number
                );
                self.upkeep.reconfigured.insert(channel.number);
                self.upkeep.pools.remove(&channel.number);
            }
        }
        self.config.channels = config.channels;
        self.keep_schedules(now_ms, store);
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

    fn key(&mut self, key: Key, now_ms: i64, store: &mut Store) {
        if self.menu.is_some() && self.menu_key(key, store) {
            return;
        }
        if self.guide.is_some() && self.guide_key(key, now_ms) {
            return;
        }
        match key {
            Key::Digit(d) => {
                let (number, digits, _) = self.entry.unwrap_or((0, 0, 0));
                let number = number.saturating_mul(10).saturating_add(u32::from(d));
                let digits = digits + 1;
                // The widest channel number, and at least two digits, ends
                // the entry without waiting.
                let width = self.numbers.last().map_or(0, |n| n.to_string().len());
                if digits >= width.max(2) {
                    self.finish_entry(number, now_ms);
                } else {
                    self.entry = Some((number, digits, now_ms + self.settings.entry_ms));
                }
            }
            Key::Next | Key::Previous => {
                self.entry = None;
                if let Some(target) = self.neighbour(key == Key::Next) {
                    self.tune(target, now_ms);
                }
            }
            Key::Banner => self.banner_pending = self.driver.following(),
            Key::Quit => self.quit = true,
            Key::Guide => self.open_guide(now_ms),
            // Not used yet: the menu and the volume come later.
            Key::Menu => self.open_menu(),
            // Not used yet: the volume comes later.
            Key::Back | Key::Mute | Key::VolumeUp | Key::VolumeDown | Key::Left | Key::Right => {}
        }
    }

    /// Ends number entry: tunes `number` when there is such a channel and
    /// ignores it otherwise.
    fn finish_entry(&mut self, number: u32, now_ms: i64) {
        self.entry = None;
        if self.numbers.contains(&number) {
            self.tune(number, now_ms);
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
        self.watermark.leave();
        self.close_guide();
        self.close_menu();
        self.channel = Some(channel);
        let picture = self.channel_config(channel).and_then(|c| c.picture);
        if picture != self.picture {
            if let Err(e) = self.tuner.player_mut().set_picture(picture) {
                eprintln!("surfcult: cannot set the picture for channel {channel}: {e}");
            }
            self.picture = picture;
        }
        self.tuner.tune(channel, now_ms);
    }

    /// Hands the player's events to the tuner and the driver, then ticks the
    /// tuner.
    fn pump(&mut self, now_ms: i64, store: &mut Store) {
        while let Some(event) = self.tuner.player_mut().poll_event() {
            match event {
                PlayerEvent::Restarted => {
                    eprintln!("surfcult: the player was restarted; tuning the channel again");
                    self.streams.stop_all();
                    self.driver.leave();
                    self.banner_pending = false;
                    self.banner_until_ms = None;
                    // The restart took the overlays and the video area with
                    // it.
                    self.watermark.leave();
                    self.watermark_shown = None;
                    self.guide = None;
                    self.menu = None;
                }
                PlayerEvent::ProgramStarted {
                    position_ms,
                    duration_ms,
                } if self.driver.following() => {
                    let mut cx = ProgramStartContext {
                        player: self.tuner.player_mut(),
                        clips: &self.clips,
                        store,
                        filler: filler(&self.config, &self.clips, self.channel),
                    };
                    self.driver
                        .program_started(now_ms, position_ms, duration_ms, &mut cx);
                }
                PlayerEvent::Failed if self.driver.following() => {
                    let mut cx = Context {
                        player: self.tuner.player_mut(),
                        store,
                        clips: &self.clips,
                        streams: self.streams.as_mut(),
                        filler: filler(&self.config, &self.clips, self.channel),
                    };
                    self.driver.item_failed(now_ms, &mut cx);
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
                self.watermark.landed(now_ms);
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
                self.watermark.landed(now_ms);
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

    /// Applies the viewer's stored choices over the environment's defaults.
    /// A stored value that does not parse is logged and ignored.
    fn load_viewer_settings(&mut self, store: &Store) {
        match store.viewer_setting(CRT_SETTING) {
            Ok(Some(text)) => match Crt::parse(&text) {
                Some(crt) => self.apply_crt(crt),
                None => eprintln!("surfcult: ignoring the stored CRT filter {text:?}"),
            },
            Ok(None) => {}
            Err(e) => eprintln!("surfcult: cannot read the viewer's settings: {e}"),
        }
    }

    /// Uses `crt` for everything drawn from now on: the watermark, the guide
    /// and the menu here, and the banner in the player.
    fn apply_crt(&mut self, crt: Crt) {
        self.settings.crt = crt;
        self.watermark_bitmaps.clear();
        if let Err(e) = self.tuner.player_mut().set_crt(crt) {
            eprintln!("surfcult: cannot set the banner's CRT filter: {e}");
        }
    }

    /// The viewer chose `crt`, in the menu or the web admin: use it, store
    /// it, and draw the menu again if it is open.
    fn choose_crt(&mut self, crt: Crt, store: &mut Store) {
        self.apply_crt(crt);
        if let Err(e) = store.set_viewer_setting(CRT_SETTING, &crt_name(crt).to_lowercase()) {
            eprintln!("surfcult: cannot store the CRT filter: {e}");
        }
        if let Some(menu) = self.menu.as_mut() {
            menu.stale = true;
        }
    }

    /// A key while the menu is open. Returns whether the menu used it; a
    /// key it does not use goes on to the usual handling.
    fn menu_key(&mut self, key: Key, store: &mut Store) -> bool {
        match key {
            Key::Banner => {
                self.choose_crt(next_crt(self.settings.crt), store);
                true
            }
            // One setting so far: nothing to move between.
            Key::Next | Key::Previous => true,
            Key::Menu | Key::Back => {
                self.close_menu();
                true
            }
            // The guide key or a channel number closes the menu and does
            // what it usually does.
            Key::Guide | Key::Digit(_) => {
                self.close_menu();
                false
            }
            _ => false,
        }
    }

    /// Opens the Picture menu over the whole screen. The guide closes, the
    /// banner goes, and the watermark hides while the menu is open.
    fn open_menu(&mut self) {
        self.close_guide();
        self.menu = Some(Menu::new());
        if self.banner_until_ms.take().is_some()
            && let Err(e) = self.tuner.player_mut().clear_banner()
        {
            eprintln!("surfcult: cannot clear the banner: {e}");
        }
    }

    /// Closes the menu, if it is open.
    fn close_menu(&mut self) {
        if self.menu.take().is_some()
            && let Err(e) = self.tuner.player_mut().clear_overlay(MENU_LAYER)
        {
            eprintln!("surfcult: cannot clear the menu: {e}");
        }
    }

    /// Draws the menu when it is open and has changed.
    fn draw_menu(&mut self) {
        let crt = self.settings.crt;
        let Some(menu) = self.menu.as_mut().filter(|m| m.stale) else {
            return;
        };
        let mut bitmap = paint::menu::draw(&menu.view(crt));
        paint::crt::apply(&mut bitmap, crt);
        menu.stale = false;
        if let Err(e) = self.tuner.player_mut().show_overlay(MENU_LAYER, &bitmap) {
            eprintln!("surfcult: cannot show the menu: {e}");
        }
    }

    /// A key while the guide is open. Returns whether the guide used it;
    /// a key it does not use goes on to the usual handling.
    fn guide_key(&mut self, key: Key, now_ms: i64) -> bool {
        let count = self.numbers.len();
        // A key during the scroll only stops it.
        if self
            .guide
            .as_mut()
            .is_some_and(|guide| guide.touch(now_ms, count))
        {
            return true;
        }
        match key {
            Key::Next | Key::Previous => {
                if let Some(guide) = self.guide.as_mut()
                    && guide.step(key == Key::Next, count)
                {
                    guide.drawn_for = None;
                }
                true
            }
            Key::Left | Key::Right => {
                if let Some(guide) = self.guide.as_mut()
                    && guide.shift(key == Key::Right)
                {
                    guide.drawn_for = None;
                }
                true
            }
            Key::Banner => {
                let target = self
                    .guide
                    .as_ref()
                    .and_then(|g| self.numbers.get(g.focus()).copied());
                self.close_guide();
                if let Some(target) = target
                    && Some(target) != self.channel
                {
                    self.tune(target, now_ms);
                }
                true
            }
            Key::Guide | Key::Back => {
                self.close_guide();
                true
            }
            // A channel number closes the guide and is entered as usual.
            Key::Digit(_) => {
                self.close_guide();
                false
            }
            _ => false,
        }
    }

    /// Opens the guide on the channel being watched and moves the video
    /// into the top-right quarter. The banner goes; the watermark hides
    /// while the guide is open.
    fn open_guide(&mut self, now_ms: i64) {
        if self.numbers.is_empty() {
            return;
        }
        let index = self
            .channel
            .and_then(|c| self.numbers.iter().position(|&n| n == c))
            .unwrap_or(0);
        self.guide = Some(Guide::new(index, self.numbers.len(), now_ms));
        let player = self.tuner.player_mut();
        if self.banner_until_ms.take().is_some()
            && let Err(e) = player.clear_banner()
        {
            eprintln!("surfcult: cannot clear the banner: {e}");
        }
        if let Err(e) = player.set_video_area(VideoArea::TopRight) {
            eprintln!("surfcult: cannot move the video for the guide: {e}");
        }
    }

    /// Closes the guide, if it is open, and gives the video the whole
    /// screen again.
    fn close_guide(&mut self) {
        if self.guide.take().is_none() {
            return;
        }
        let player = self.tuner.player_mut();
        for layer in [GUIDE_LAYER, GUIDE_CLOCK_LAYER, GUIDE_ROWS_LAYER] {
            if let Err(e) = player.clear_overlay(layer) {
                eprintln!("surfcult: cannot clear the guide: {e}");
            }
        }
        if let Err(e) = player.set_video_area(VideoArea::Full) {
            eprintln!("surfcult: cannot give the video the whole screen: {e}");
        }
    }

    /// Draws the guide when it is open: the body and the rows when the
    /// focus moved or a new half hour began, the clock when its second
    /// changed, and the rows' place in their bitmap when they scroll.
    fn draw_guide(&mut self, now_ms: i64, store: &Store) {
        let Some(mut guide) = self.guide.take() else {
            return;
        };
        let timezone = self.config.timezone;
        let crt = self.settings.crt;
        let from = guide.from(now_ms);
        if guide.drawn_for != Some(from) {
            let channels: Vec<&Channel> = self
                .numbers
                .iter()
                .filter_map(|n| self.config.channels.iter().find(|c| c.number == *n))
                .collect();
            let view = guide.view(&channels, store, now_ms, timezone);
            let mut body = paint::guide::draw(&view);
            paint::crt::apply(&mut body, crt);
            let mut rows = paint::guide::draw_rows(&view.rows);
            paint::crt::apply(&mut rows, crt);
            let top = guide.top_row(now_ms, self.numbers.len());
            let player = self.tuner.player_mut();
            if let Err(e) = player.show_overlay(GUIDE_LAYER, &body) {
                eprintln!("surfcult: cannot show the guide: {e}");
            }
            let visible = paint::guide::VISIBLE.min(rows.height.saturating_sub(top));
            match player.show_overlay_rows(GUIDE_ROWS_LAYER, &rows, top, visible) {
                Ok(()) => guide.shown_row = Some(top),
                Err(e) => eprintln!("surfcult: cannot show the guide's rows: {e}"),
            }
            guide.drawn_for = Some(from);
        } else {
            let top = guide.top_row(now_ms, self.numbers.len());
            if guide.shown_row.is_some_and(|shown| shown != top) {
                let player = self.tuner.player_mut();
                match player.scroll_overlay(GUIDE_ROWS_LAYER, top, paint::guide::VISIBLE) {
                    Ok(()) => guide.shown_row = Some(top),
                    Err(e) => eprintln!("surfcult: cannot scroll the guide: {e}"),
                }
            }
        }
        let clock = clock_with_seconds(now_ms, timezone);
        if guide.clock_drawn.as_deref() != Some(clock.as_str()) {
            let mut cell = paint::guide::draw_clock(&clock);
            paint::crt::apply(&mut cell, crt);
            if let Err(e) = self
                .tuner
                .player_mut()
                .show_overlay(GUIDE_CLOCK_LAYER, &cell)
            {
                eprintln!("surfcult: cannot show the guide's clock: {e}");
            }
            guide.clock_drawn = Some(clock);
        }
        self.guide = Some(guide);
    }

    /// Shows the watermark in the form and at the opacity due at `now_ms`,
    /// or clears it, when that changed since the last step.
    fn show_watermark(&mut self, now_ms: i64) {
        if !self.settings.watermark {
            return;
        }
        let want = if self.driver.following() && self.guide.is_none() && self.menu.is_none() {
            self.watermark.at(now_ms, self.driver.on_break())
        } else {
            None
        };
        if want == self.watermark_shown {
            return;
        }
        let result = match want {
            Some((form, opacity)) => {
                let crt = self.settings.crt;
                let base = self.watermark_bitmaps.entry(form).or_insert_with(|| {
                    let mut bitmap = paint::watermark::draw(form);
                    paint::crt::apply(&mut bitmap, crt);
                    bitmap
                });
                let mut bitmap = base.clone();
                paint::fade(&mut bitmap, opacity * paint::watermark::OPACITY);
                self.tuner
                    .player_mut()
                    .show_overlay(WATERMARK_LAYER, &bitmap)
            }
            None => self.tuner.player_mut().clear_overlay(WATERMARK_LAYER),
        };
        if let Err(e) = result {
            eprintln!("surfcult: cannot show the watermark: {e}");
        }
        // Recorded even after an error, so a broken player is not asked
        // again on every step.
        self.watermark_shown = want;
    }

    /// On a music channel, shows the picture of the song on screen in its
    /// layer: the cover card, redrawn when the cover arrives, or the
    /// now-playing line over the visualizer. Clears it for anything else on
    /// screen, and while the guide or the menu is open.
    fn show_picture(&mut self, store: &Store) {
        if let Some(covers) = self.covers.as_mut() {
            for (id, image) in covers.take() {
                self.covers_asked.remove(&id);
                if self.cover_images.len() >= KEPT_COVERS {
                    self.cover_images.clear();
                }
                self.cover_images.insert(id, image);
            }
        }
        let picture = self
            .channel
            .and_then(|n| self.channel_config(n))
            .and_then(|c| c.picture);
        let want = match picture {
            Some(picture) if self.guide.is_none() && self.menu.is_none() => {
                self.driver.program().map(|media| (picture, media.clone()))
            }
            _ => None,
        };
        let Some((picture, media)) = want else {
            if self.picture_shown.take().is_some()
                && let Err(e) = self.tuner.player_mut().clear_overlay(PICTURE_LAYER)
            {
                eprintln!("surfcult: cannot clear the music picture: {e}");
            }
            return;
        };
        let current = self
            .picture_shown
            .as_ref()
            .filter(|shown| shown.media == media && shown.picture == picture);
        if let Some(shown) = current {
            let arrived = shown
                .cover
                .as_ref()
                .is_some_and(|id| self.cover_images.contains_key(id));
            if !shown.waiting || !arrived {
                return;
            }
        }
        let Ok(item) = store.resolve(&media) else {
            return;
        };
        let song = NowPlaying {
            artists: item.artists.join(", "),
            title: item.title,
            album: item.album,
        };
        let (mut overlay, cover, waiting) = match picture {
            Picture::Cover => {
                let id = item.album_id.unwrap_or(item.id);
                let image = self.cover_images.get(&id).cloned();
                let waiting = image.is_none() && self.covers.is_some();
                if waiting
                    && self.covers_asked.insert(id.clone())
                    && let Some(covers) = self.covers.as_mut()
                {
                    covers.request(&id);
                }
                let card = paint::now_playing::card(&song, image.flatten().as_ref());
                (card, Some(id), waiting)
            }
            Picture::Visualizer => (paint::now_playing::line(&song), None, false),
        };
        paint::crt::apply(&mut overlay, self.settings.crt);
        if let Err(e) = self
            .tuner
            .player_mut()
            .show_overlay(PICTURE_LAYER, &overlay)
        {
            eprintln!("surfcult: cannot show the music picture: {e}");
        }
        // Recorded even after an error, so a broken player is not asked
        // again on every step.
        self.picture_shown = Some(PictureShown {
            media,
            picture,
            cover,
            waiting,
        });
    }

    fn channel_config(&self, number: u32) -> Option<&Channel> {
        self.config.channels.iter().find(|c| c.number == number)
    }

    /// Extends every channel's schedule. After a sync changed the library, a
    /// channel whose schedule plays an item that changed runtime or left is
    /// regenerated instead ([`schedule::reschedule`]). Does nothing but wait
    /// while the clock is not trusted; the changes wait too.
    fn keep_schedules(&mut self, now_ms: i64, store: &mut Store) {
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
        let changes = std::mem::take(&mut self.upkeep.changes);
        // Clips added to or removed from a pool since the last upkeep count
        // from here on. Unchanged files are not probed again; new and
        // changed ones are probed in the background and join their pools
        // when done.
        let queued = self.clips.rescan();
        if queued > 0 {
            eprintln!("surfcult: probing {queued} new clips");
        }
        self.clips_changed(store);
        let pools_now = self.clips.pools();
        for channel in &self.config.channels {
            let n = channel.number;
            let pools = match self.upkeep.pools.entry(n) {
                Entry::Occupied(cached) => &*cached.into_mut(),
                Entry::Vacant(slot) => match resolve_pools(channel, store) {
                    Ok(pools) => &*slot.insert(pools),
                    Err(why) => {
                        if self.upkeep.problems.get(&n) != Some(&why) {
                            eprintln!("surfcult: {why}; its schedule is not extended");
                            self.upkeep.problems.insert(n, why);
                        }
                        continue;
                    }
                },
            };
            let tz = self.config.timezone;
            let reconfigured = self.upkeep.reconfigured.contains(&n);
            let result = if reconfigured {
                schedule::regenerate(store, channel, pools, pools_now, tz, now_ms, true)
            } else if changes.is_empty() {
                schedule::extend(store, channel, pools, pools_now, tz, now_ms, true)
            } else {
                schedule::reschedule(store, channel, pools, pools_now, tz, now_ms, &changes)
            };
            match result {
                Ok(Extended::Generated { .. } | Extended::UpToDate { .. }) => {
                    self.upkeep.problems.remove(&n);
                    self.upkeep.reconfigured.remove(&n);
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
        }
        self.driver.invalidate();
    }
}

impl<P: Player, I: Input> Tv<P, I> {
    /// Follows a change to the clip pools: stores the new probe results,
    /// logs new issues and gives the tuner the tune-in pool's clips.
    fn clips_changed(&mut self, store: &mut Store) {
        self.save_clip_probes(store);
        self.log_clip_issues();
        let tune_in = self
            .clips
            .pool(&self.config.tune_in_pool)
            .map(|pool| pool.clips().to_vec())
            .unwrap_or_default();
        self.tuner.set_tune_in(tune_in);
    }

    fn save_clip_probes(&mut self, store: &mut Store) {
        let changes = self.clips.take_changes();
        if !changes.is_empty()
            && let Err(e) = store.save_clip_probes(&changes)
        {
            eprintln!("surfcult: cannot store clip durations: {e}");
        }
    }

    /// Logs each clip issue the first time it appears. An issue that goes
    /// away and comes back is logged again.
    fn log_clip_issues(&mut self) {
        let now: HashSet<String> = self.clips.issues().map(ToString::to_string).collect();
        for issue in now.difference(&self.clip_issues) {
            eprintln!("surfcult: clip skipped: {issue}");
        }
        self.clip_issues = now;
    }
}

/// The filler pool's clips of channel `number`, empty when it has none.
fn filler<'a>(config: &Config, clips: &'a ClipLibrary, number: Option<u32>) -> &'a [Clip] {
    number
        .and_then(|number| config.channels.iter().find(|c| c.number == number))
        .and_then(|channel| channel.filler.as_deref())
        .and_then(|pool| clips.pool(pool))
        .map_or(&[], |pool| pool.clips())
}

/// The program pool of each of `channel`'s sources, or every reason one
/// cannot be resolved, joined.
fn resolve_pools(channel: &Channel, store: &Store) -> Result<Pools, String> {
    let mut pools = Vec::new();
    let mut problems = Vec::new();
    for source in channel.plan.sources() {
        match resolve_pool(channel.number, channel.plays(), source, store) {
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
