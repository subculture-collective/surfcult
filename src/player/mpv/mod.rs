//! [`MpvPlayer`]: the [`Player`] that drives a real MPV process over its JSON
//! IPC socket.
//!
//! # The process
//!
//! [`MpvPlayer::start`] runs MPV idle, with no config file, no terminal input
//! and no youtube-dl hook, and connects to its socket in a private directory
//! (mode 700). [`MpvConfig::output_args`] chooses the video and audio output;
//! the default is the Pi setup from ADR 0001, decision 2, which asks for
//! hardware decoding with `--hwdec=drm,v4l2m2m`. MPV tries each decoder in
//! that list and decodes in software when neither works, so no fallback code
//! is needed here. `auto-safe` is not used because the pair was measured on
//! the Pi and `auto-safe`'s choice there was not.
//!
//! # Programs and the Jellyfin key
//!
//! The player does not know Jellyfin. A [`StreamResolver`] turns a media id
//! and start position into a [`Stream`]: a URL, the HTTP headers it needs and
//! where in the media the stream begins. The headers are set as MPV's
//! `http-header-fields` property just before the program is appended, so the
//! key travels only through the socket: never in MPV's arguments, never in
//! the URL and never in a file. (ADR 0001 suggested a config file passed with
//! `--include`; the property keeps the key off the disk as well as out of the
//! process list. A per-entry option does not work: MPV's prefetch opens the
//! stream without it.)
//!
//! A program is appended to the playlist behind the clip, with
//! `--prefetch-playlist=yes` so MPV opens it while the clip plays. It is not
//! preloaded paused: one MPV process cannot hold a second file paused (ADR
//! 0001, decision 5).
//!
//! # The stand-by card
//!
//! [`Player::show_standby`] plays [`MpvConfig::standby_source`], colour bars
//! generated live by FFmpeg's `lavfi`, with a black band and
//! [`MpvConfig::standby_text`] across the middle. The band and text are an
//! ASS subtitle file ([`card`]) that the player writes into the runtime
//! directory and MPV draws through libass, so the card does not need an
//! FFmpeg built with `drawtext`. MPV blends the subtitles into the picture
//! before the shaders run, so the CRT shader covers them too. The file is
//! the subtitle track of the card's playlist entry only, so it leaves with
//! the card. If the card cannot be shown, the player shows static instead.
//!
//! # Display filters
//!
//! [`MpvConfig::filters`] are on everything on screen and
//! [`MpvConfig::standby_filters`] are added while the card shows, each
//! filter once. The CRT shader ([`CRT_SHADER`]) is written into the runtime
//! directory and set through MPV's `glsl-shaders` property: the global list
//! as soon as MPV is up (also after a restart), the card's list while it
//! shows. The glitch is stepped in [`Player::poll_event`]: for a moment MPV
//! shows a second subtitle file, the card with a flicker, over the card's
//! own as its secondary subtitle track, then removes it.
//!
//! # Events
//!
//! - `end-file` with reason `eof` for a clip becomes
//!   [`PlayerEvent::ClipEnded`]. Static, the stand-by card and programs
//!   never produce one, and nor does an entry that a later command replaced
//!   or stopped.
//! - The first `playback-restart` of a program becomes
//!   [`PlayerEvent::ProgramStarted`], with MPV's `time-pos` plus the stream's
//!   offset. Later ones come from seeks and are dropped.
//!
//! # The watchdog
//!
//! [`Player::poll_event`] checks that MPV is alive. At most once per
//! [`LIVENESS_INTERVAL`] it also asks MPV for its `pid` property, so a hung
//! MPV is found even when the tuner sends no commands. When the process has
//! exited, the socket closed, or a command or that check got no answer within
//! [`MpvConfig::command_timeout`] (a hung MPV is treated as a dead one), the
//! player kills what is left and starts a new MPV, at most once per
//! [`RESTART_DELAY`]. A restart blocks `poll_event` for up to
//! [`MpvConfig::start_timeout`] while the new MPV opens its socket, and while
//! MPV keeps failing to open it that repeats about once a second. When the
//! new one is up it reports
//! [`PlayerEvent::Restarted`], and the tuner tunes the current channel again.
//! The player keeps no replay log of its own: the schedule and the channel
//! are the tuner's, so re-tuning from them puts the program back at its live
//! position rather than where it was when MPV died. Commands sent while MPV
//! is down fail with a [`PlayerError`].

mod banner;
mod card;
mod glitch;
mod ipc;
#[cfg(test)]
mod tests;

use std::ffi::OsString;
use std::fmt;
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use super::{
    DEFAULT_GLITCH_EVERY, DEFAULT_STANDBY_FILTERS, Filter, Overlay, Player, PlayerError,
    PlayerEvent, standby_filters,
};
use crate::config::Display;
use crate::library::MediaId;
use crate::overlay::Banner;
use glitch::{Flicker, Glitch, Step};
use ipc::{IpcError, Session};

/// MPV's output flags for the Pi 4B on its TV (ADR 0001, decision 2).
///
/// `--fbo-format=rgba8` keeps MPV's intermediate frames at 8 bits per
/// channel. With the default 16-bit format the V3D GPU dropped 29% of the
/// stand-by card's frames even for an empty shader, and 8 bits made that 0%
/// (Pi check of 2026-11-24, `docs/research/2026-11-24-pi-hardware-checks.md`).
/// The pictures are SDR, so nothing is lost.
///
/// Sound goes to the first HDMI port, `vc4hdmi0`, through ALSA. Without it MPV
/// takes ALSA's default, which on the Pi is the analog headphone jack, so the
/// TV is silent (found on the first run of `surfcult run`, 2026-11-24). A Pi
/// whose TV is on the other port sets `SURFCULT_AUDIO_DEVICE`
/// (`alsa/plughw:CARD=vc4hdmi1,DEV=0`).
pub const PI_OUTPUT_ARGS: [&str; 9] = [
    "--profile=fast",
    "--vo=gpu",
    "--gpu-context=drm",
    "--drm-mode=1920x1080@60",
    "--hwdec=drm,v4l2m2m",
    "--fbo-format=rgba8",
    "--force-window=yes",
    "--ao=alsa",
    "--audio-device=alsa/plughw:CARD=vc4hdmi0,DEV=0",
];

/// TV static: grey noise generated live by FFmpeg's `lavfi` at a low
/// resolution and scaled up by MPV, so it costs little CPU. The generator
/// never ends, so no file loops.
pub const STATIC_SOURCE: &str =
    "av://lavfi:nullsrc=s=320x180:r=25,geq=lum='random(1)*255':cb=128:cr=128";

/// The stand-by card's picture: colour bars matched to the owner's design,
/// a `lavfi` colour source with one `drawbox` per colour region
/// (`assets/standby/bars.lavfi`, made by `tools/standby/gen.py`). FFmpeg
/// generates it live at 640x360 and MPV scales it up on the GPU. 30 frames a
/// second give the CRT shader's flicker and rolling band frames to move on.
/// Silent: the source has no audio.
pub const STANDBY_SOURCE: &str =
    include_str!("../../../assets/standby/bars.lavfi").trim_ascii_end();

/// The CRT shader ([`Filter::Crt`]), from `assets/shaders/crt.glsl`.
/// [`MpvPlayer::start`] writes it into the runtime directory, because MPV
/// loads shaders from files.
pub const CRT_SHADER: &str = include_str!("../../../assets/shaders/crt.glsl");

/// The message across the middle of the stand-by card. This text, in any
/// case, is drawn from outlines of the Oswald typeface; any other text is
/// set in DejaVu Sans Bold.
pub const STANDBY_TEXT: &str = "PLEASE STANDBY";

/// The shortest time between two starts of MPV.
pub const RESTART_DELAY: Duration = Duration::from_secs(1);

/// The shortest time between two liveness checks of a running MPV.
pub const LIVENESS_INTERVAL: Duration = Duration::from_secs(3);

/// Flags every MPV gets, before [`MpvConfig::output_args`].
const BASE_ARGS: [&str; 7] = [
    "--no-config",
    "--idle=yes",
    "--input-terminal=no",
    "--ytdl=no",
    "--keep-open=no",
    "--prefetch-playlist=yes",
    "--msg-level=all=error",
];

/// Unix socket paths are limited to 108 bytes, including the final zero.
const MAX_SOCKET_PATH: usize = 107;

/// The global shader files, the stand-by card's shader files and the glitch
/// timer, from [`MpvConfig::filters_setup`].
type FilterSetup = (Vec<String>, Vec<String>, Option<Glitch>);

/// How to run MPV.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpvConfig {
    /// The MPV executable.
    pub binary: PathBuf,
    /// A directory only this user can open (mode 700). It holds the IPC
    /// socket, the overlay bitmaps, the CRT shader and the stand-by card's
    /// subtitle file. It is created if missing; an existing
    /// directory that others can open is refused.
    pub runtime_dir: PathBuf,
    /// Video and audio output flags, after the fixed ones.
    pub output_args: Vec<String>,
    /// What [`Player::show_static`] plays. It is loaded with
    /// `loop-file=inf`, which matters only for a source that ends; the
    /// default is a generator that never does.
    pub static_source: String,
    /// What [`Player::show_standby`] plays under its message, loaded like
    /// `static_source`.
    pub standby_source: String,
    /// The message [`Player::show_standby`] draws across the middle of the
    /// screen. [`STANDBY_TEXT`] is drawn from the Oswald outlines; any other
    /// text in DejaVu Sans Bold, which must be installed.
    pub standby_text: String,
    /// Display filters on everything on screen.
    pub filters: Vec<Filter>,
    /// Display filters the stand-by card adds while it shows.
    pub standby_filters: Vec<Filter>,
    /// The mean time between two glitches of the stand-by card's text.
    pub glitch_every: Duration,
    /// How long a command may wait for MPV's answer.
    pub command_timeout: Duration,
    /// How long a new MPV may take to open its socket.
    pub start_timeout: Duration,
}

impl MpvConfig {
    /// The Pi setup: `mpv` from `PATH`, [`PI_OUTPUT_ARGS`],
    /// [`STATIC_SOURCE`], [`STANDBY_SOURCE`] and [`STANDBY_TEXT`].
    pub fn new(runtime_dir: impl Into<PathBuf>) -> Self {
        Self {
            binary: PathBuf::from("mpv"),
            runtime_dir: runtime_dir.into(),
            output_args: PI_OUTPUT_ARGS.iter().map(|a| a.to_string()).collect(),
            static_source: STATIC_SOURCE.into(),
            standby_source: STANDBY_SOURCE.into(),
            standby_text: STANDBY_TEXT.into(),
            filters: Vec::new(),
            standby_filters: DEFAULT_STANDBY_FILTERS.to_vec(),
            glitch_every: DEFAULT_GLITCH_EVERY,
            command_timeout: Duration::from_secs(5),
            start_timeout: Duration::from_secs(10),
        }
    }

    /// This config with the filters and glitch rate of `channels.toml`'s
    /// `[display]` table.
    pub fn with_display(self, display: &Display) -> Self {
        Self {
            filters: display.filters.clone(),
            standby_filters: display.standby_filters.clone(),
            glitch_every: display.glitch_every,
            ..self
        }
    }

    fn socket(&self) -> PathBuf {
        self.runtime_dir.join("mpv.sock")
    }

    /// The stand-by card's subtitle file, or the one of its flicker.
    fn standby_file(&self, flicker: bool) -> PathBuf {
        let name = if flicker {
            "standby-flicker.ass"
        } else {
            "standby.ass"
        };
        self.runtime_dir.join(name)
    }

    /// The file MPV loads the shader of `filter` from, if it has one.
    fn shader_file(&self, filter: Filter) -> Option<PathBuf> {
        match filter {
            Filter::Crt => Some(self.runtime_dir.join("crt.glsl")),
            Filter::Glitch => None,
        }
    }

    /// The `glsl-shaders` lists outside the stand-by card and on it, and the
    /// glitch timer if the card glitches. The card's filters are the global
    /// ones followed by its own, each once ([`standby_filters`]).
    fn filters_setup(&self) -> Result<FilterSetup, PlayerError> {
        let global = standby_filters(&self.filters, &[]);
        let on_card = standby_filters(&self.filters, &self.standby_filters);
        let glitch = on_card
            .contains(&Filter::Glitch)
            .then(|| Glitch::new(self.glitch_every, glitch::SEED));
        Ok((
            self.shader_list(&global)?,
            self.shader_list(&on_card)?,
            glitch,
        ))
    }

    /// The `glsl-shaders` list for `filters`.
    fn shader_list(&self, filters: &[Filter]) -> Result<Vec<String>, PlayerError> {
        filters
            .iter()
            .filter_map(|&f| self.shader_file(f))
            .map(|path| {
                path.into_os_string()
                    .into_string()
                    .map_err(|_| PlayerError("the runtime directory is not UTF-8".into()))
            })
            .collect()
    }
}

/// Where MPV reads a program from.
#[derive(Clone, PartialEq, Eq)]
pub struct Stream {
    /// A URL or local path. It must not hold a key.
    pub url: String,
    /// HTTP headers to send with every request for `url`, such as
    /// `Authorization: MediaBrowser Token="KEY"`.
    pub headers: Vec<(String, String)>,
    /// The media position, in milliseconds, at which the stream begins: 0 for
    /// a file or a direct-play stream, the start position for a transcode
    /// that Jellyfin starts there.
    pub stream_offset_ms: u64,
}

impl Stream {
    /// A stream that begins at the start of the media.
    pub fn new(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            headers: Vec::new(),
            stream_offset_ms: 0,
        }
    }
}

/// Prints header names only, so a key never lands in a log.
impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let names: Vec<&str> = self.headers.iter().map(|(n, _)| n.as_str()).collect();
        f.debug_struct("Stream")
            .field("url", &self.url)
            .field("headers", &names)
            .field("stream_offset_ms", &self.stream_offset_ms)
            .finish()
    }
}

/// Turns a program into a [`Stream`]. A closure
/// `Fn(&MediaId, u64) -> Result<Stream, PlayerError>` is one.
pub trait StreamResolver {
    /// The stream that plays `media` from `start_ms` into it.
    fn stream(&self, media: &MediaId, start_ms: u64) -> Result<Stream, PlayerError>;
}

impl<F> StreamResolver for F
where
    F: Fn(&MediaId, u64) -> Result<Stream, PlayerError>,
{
    fn stream(&self, media: &MediaId, start_ms: u64) -> Result<Stream, PlayerError> {
        self(media, start_ms)
    }
}

/// A [`Player`] backed by one MPV process. See the module documentation.
pub struct MpvPlayer<R> {
    config: MpvConfig,
    resolver: R,
    child: Option<Child>,
    session: Option<Session>,
    last_start: Instant,
    last_ping: Instant,
    /// The shader files outside the stand-by card and on it.
    global_shaders: Vec<String>,
    standby_shaders: Vec<String>,
    /// The stand-by card's glitch, when the card has it.
    glitch: Option<Glitch>,
}

impl<R> MpvPlayer<R> {
    /// The process id of the running MPV, if one is running.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// Kills MPV and forgets the connection. The next poll starts a new one.
    fn shut_down(&mut self) {
        self.session = None;
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    fn overlay_file(&self, id: u8) -> PathBuf {
        self.config.runtime_dir.join(format!("overlay-{id}.bgra"))
    }

    /// Writes the stand-by card's subtitle file, or with `flicker` the
    /// flicker's, and returns its path. A new file is renamed over the old
    /// one, so MPV never reads half of it.
    fn write_card(&self, flicker: Option<&Flicker>) -> Result<String, PlayerError> {
        let file = self.config.standby_file(flicker.is_some());
        let partial = file.with_extension("partial");
        fs::write(&partial, card::card_ass(&self.config.standby_text, flicker))
            .and_then(|()| fs::rename(&partial, &file))
            .map_err(|e| PlayerError(format!("cannot write the stand-by card: {e}")))?;
        file.into_os_string()
            .into_string()
            .map_err(|_| PlayerError("the runtime directory is not UTF-8".into()))
    }
}

impl<R: StreamResolver> MpvPlayer<R> {
    /// Starts MPV and connects to it.
    pub fn start(config: MpvConfig, resolver: R) -> Result<Self, PlayerError> {
        prepare_runtime_dir(&config.runtime_dir)?;
        let socket = config.socket();
        if socket.as_os_str().len() > MAX_SOCKET_PATH {
            return Err(PlayerError(format!(
                "the MPV socket path {} is longer than {MAX_SOCKET_PATH} bytes",
                socket.display()
            )));
        }
        let (global_shaders, standby_shaders, glitch) = config.filters_setup()?;
        if let Some(file) = config.shader_file(Filter::Crt) {
            fs::write(&file, CRT_SHADER)
                .map_err(|e| PlayerError(format!("cannot write the CRT shader: {e}")))?;
        }
        let mut player = Self {
            config,
            resolver,
            child: None,
            session: None,
            last_start: Instant::now(),
            last_ping: Instant::now(),
            global_shaders,
            standby_shaders,
            glitch,
        };
        player.launch()?;
        Ok(player)
    }

    fn launch(&mut self) -> Result<(), PlayerError> {
        self.last_start = Instant::now();
        let socket = self.config.socket();
        match fs::remove_file(&socket) {
            Err(e) if e.kind() != ErrorKind::NotFound => {
                return Err(PlayerError(format!(
                    "cannot remove the old MPV socket: {e}"
                )));
            }
            _ => {}
        }
        let mut child = Command::new(&self.config.binary)
            .args(args(&self.config, &socket))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                PlayerError(format!(
                    "cannot start {}: {e}",
                    self.config.binary.display()
                ))
            })?;
        let deadline = Instant::now() + self.config.start_timeout;
        let mut session = loop {
            if let Ok(Some(status)) = child.try_wait() {
                return Err(PlayerError(format!("MPV exited at start: {status}")));
            }
            match Session::connect(&socket, self.config.command_timeout) {
                Ok(session) => break session,
                Err(e) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(PlayerError(format!("cannot connect to MPV: {e}")));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        // A new MPV has no shaders: give it the global ones.
        let shaders = (self.global_shaders.clone(), self.standby_shaders.clone());
        if let Err(e) = session.use_shaders(shaders.0, shaders.1) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PlayerError(format!(
                "cannot set MPV's shaders: {}",
                e.message()
            )));
        }
        if let Some(glitch) = self.glitch.as_mut() {
            glitch.disarm();
        }
        self.child = Some(child);
        self.session = Some(session);
        self.last_ping = Instant::now();
        Ok(())
    }

    /// Runs `f` on the session. A lost connection shuts MPV down, so the
    /// watchdog replaces it.
    fn with_session<T>(
        &mut self,
        f: impl FnOnce(&mut Session) -> Result<T, IpcError>,
    ) -> Result<T, PlayerError> {
        let Some(session) = self.session.as_mut() else {
            return Err(PlayerError(
                "MPV is not running; it is being restarted".into(),
            ));
        };
        f(session).map_err(|e| {
            if matches!(e, IpcError::Lost(_)) {
                self.shut_down();
            }
            PlayerError(e.message().to_owned())
        })
    }

    /// Asks a running MPV for its `pid` when [`LIVENESS_INTERVAL`] has
    /// passed since the last check. No answer shuts MPV down, so the next
    /// poll starts a new one.
    fn check_answers(&mut self) {
        if self.last_ping.elapsed() < LIVENESS_INTERVAL {
            return;
        }
        self.last_ping = Instant::now();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Err(IpcError::Lost(why)) = session.ping() {
            eprintln!("surfcult: MPV is not answering ({why}); restarting it");
            self.shut_down();
        }
    }

    /// Starts or ends a flicker of the stand-by card when one is due. A
    /// flicker is a second subtitle file that MPV shows over the card's.
    fn step_glitch(&mut self) {
        let Some(glitch) = self.glitch.as_mut() else {
            return;
        };
        if !self.session.as_ref().is_some_and(Session::standby_up) {
            glitch.disarm();
            return;
        }
        let redrawn = match glitch.step(Instant::now()) {
            Step::Nothing => return,
            Step::Flicker(flicker) => self
                .write_card(Some(&flicker))
                .and_then(|file| self.with_session(|s| s.start_flicker(&file))),
            Step::Settle => self.with_session(Session::end_flicker),
        };
        if let Err(e) = redrawn {
            eprintln!("surfcult: cannot redraw the stand-by card: {e}");
        }
    }

    /// The watchdog's half of [`Player::poll_event`]: notices a dead or hung
    /// MPV and starts a new one when [`RESTART_DELAY`] has passed.
    fn check_alive(&mut self) -> Option<PlayerEvent> {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => {
                    self.check_answers();
                    return None;
                }
                Ok(Some(status)) => eprintln!("surfcult: MPV exited ({status}); restarting it"),
                Err(e) => eprintln!("surfcult: cannot check MPV ({e}); restarting it"),
            }
            self.shut_down();
        }
        if self.last_start.elapsed() < RESTART_DELAY {
            return None;
        }
        match self.launch() {
            Ok(()) => Some(PlayerEvent::Restarted),
            Err(e) => {
                eprintln!("surfcult: {e}; trying again");
                None
            }
        }
    }
}

impl<R> Drop for MpvPlayer<R> {
    fn drop(&mut self) {
        self.shut_down();
        let _ = fs::remove_file(self.config.socket());
        if let Some(file) = self.config.shader_file(Filter::Crt) {
            let _ = fs::remove_file(file);
        }
        for flicker in [false, true] {
            let card = self.config.standby_file(flicker);
            let _ = fs::remove_file(card.with_extension("partial"));
            let _ = fs::remove_file(card);
        }
        for id in 0..64 {
            let _ = fs::remove_file(self.overlay_file(id));
        }
    }
}

impl<R: StreamResolver> Player for MpvPlayer<R> {
    fn show_static(&mut self) -> Result<(), PlayerError> {
        let source = self.config.static_source.clone();
        self.with_session(|s| s.show_static(&source))
    }

    fn show_standby(&mut self) -> Result<(), PlayerError> {
        let source = self.config.standby_source.clone();
        let result = self
            .write_card(None)
            .and_then(|file| self.with_session(|s| s.show_standby(&source, &file)));
        match &result {
            Ok(()) => {
                if let Some(glitch) = self.glitch.as_mut() {
                    glitch.arm(Instant::now());
                }
            }
            // The card's file could not be written, or MPV answered but
            // refused part of the card, which may have left bars without
            // their text. Static replaces them.
            Err(_) if self.session.is_some() => {
                if let Err(e) = self.show_static() {
                    eprintln!("surfcult: cannot show static after the stand-by card failed: {e}");
                }
            }
            Err(_) => {}
        }
        result
    }

    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.with_session(|s| s.play_clip(path))
    }

    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        let stream = self.resolver.stream(media, start_ms)?;
        self.with_session(|s| s.queue_program(&stream, start_ms))
    }

    fn play_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        let stream = self.resolver.stream(media, start_ms)?;
        self.with_session(|s| s.play_program(&stream, start_ms))
    }

    fn queue_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.with_session(|s| s.queue_clip(path))
    }

    fn loop_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.with_session(|s| s.loop_clip(path))
    }

    fn queue_looping_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.with_session(|s| s.queue_looping_clip(path))
    }

    fn seek(&mut self, position_ms: u64) -> Result<(), PlayerError> {
        self.with_session(|s| s.seek(position_ms))
    }

    fn stop(&mut self) -> Result<(), PlayerError> {
        self.with_session(Session::stop)
    }

    fn show_overlay(&mut self, id: u8, overlay: &Overlay) -> Result<(), PlayerError> {
        check_overlay(id, overlay)?;
        // MPV maps the file when the command runs. Writing a new file and
        // renaming it over the old one never shows a half-written bitmap.
        let file = self.overlay_file(id);
        let partial = file.with_extension("partial");
        fs::write(&partial, &overlay.bgra)
            .and_then(|()| fs::rename(&partial, &file))
            .map_err(|e| PlayerError(format!("cannot write the overlay bitmap: {e}")))?;
        let file = file
            .to_str()
            .ok_or_else(|| PlayerError("the runtime directory is not UTF-8".into()))?
            .to_owned();
        self.with_session(|s| {
            s.overlay_add(
                id,
                overlay.x,
                overlay.y,
                &file,
                overlay.width,
                overlay.height,
            )
        })
    }

    fn clear_overlay(&mut self, id: u8) -> Result<(), PlayerError> {
        if id > 63 {
            return Err(PlayerError(format!("overlay layer {id} is not 0 to 63")));
        }
        self.with_session(|s| s.overlay_remove(id))
    }

    fn show_banner(&mut self, banner: &Banner) -> Result<(), PlayerError> {
        let ass = banner::banner_ass(banner);
        self.with_session(|s| s.show_banner(&ass))
    }

    fn clear_banner(&mut self) -> Result<(), PlayerError> {
        self.with_session(Session::hide_banner)
    }

    fn poll_event(&mut self) -> Option<PlayerEvent> {
        if let Some(event) = self.check_alive() {
            return Some(event);
        }
        self.step_glitch();
        let session = self.session.as_mut()?;
        match session.poll() {
            Ok(event) => event,
            Err(IpcError::Rejected(_)) => None,
            Err(IpcError::Lost(why)) => {
                eprintln!("surfcult: lost MPV ({why}); restarting it");
                self.shut_down();
                None
            }
        }
    }
}

/// The arguments MPV is started with. They never include a header or key:
/// those go through the socket.
fn args(config: &MpvConfig, socket: &Path) -> Vec<OsString> {
    let mut ipc = OsString::from("--input-ipc-server=");
    ipc.push(socket);
    BASE_ARGS
        .iter()
        .map(OsString::from)
        .chain([ipc])
        .chain(config.output_args.iter().map(OsString::from))
        .collect()
}

fn prepare_runtime_dir(dir: &Path) -> Result<(), PlayerError> {
    let error = |e: std::io::Error| {
        PlayerError(format!(
            "cannot use the runtime directory {}: {e}",
            dir.display()
        ))
    };
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
        .map_err(error)?;
    let mode = fs::metadata(dir).map_err(error)?.permissions().mode();
    if mode & 0o077 != 0 {
        return Err(PlayerError(format!(
            "the runtime directory {} is open to other users (mode {:o}); it must be 700",
            dir.display(),
            mode & 0o777
        )));
    }
    Ok(())
}

fn check_overlay(id: u8, overlay: &Overlay) -> Result<(), PlayerError> {
    if id > 63 {
        return Err(PlayerError(format!("overlay layer {id} is not 0 to 63")));
    }
    let expected = u64::from(overlay.width) * u64::from(overlay.height) * 4;
    if overlay.width == 0 || overlay.height == 0 || overlay.bgra.len() as u64 != expected {
        return Err(PlayerError(format!(
            "an overlay of {}x{} needs {expected} bytes of BGRA, not {}",
            overlay.width,
            overlay.height,
            overlay.bgra.len()
        )));
    }
    Ok(())
}
