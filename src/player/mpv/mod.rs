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
//! generated live by FFmpeg's `lavfi`, and draws a black band with
//! [`MpvConfig::standby_text`] across the middle as an MPV `osd-overlay` in
//! ASS. MPV draws the text itself through libass, so the card does not need
//! an FFmpeg built with `drawtext`. The text is drawn three times for a
//! chromatic offset: red shifted left, cyan shifted right and white on top.
//! The overlay is removed before anything else is shown, queued or stopped.
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

use super::{Overlay, Player, PlayerError, PlayerEvent};
use crate::library::MediaId;
use ipc::{IpcError, Session};

/// MPV's output flags for the Pi 4B on its TV (ADR 0001, decision 2).
pub const PI_OUTPUT_ARGS: [&str; 6] = [
    "--profile=fast",
    "--vo=gpu",
    "--gpu-context=drm",
    "--drm-mode=1920x1080@60",
    "--hwdec=drm,v4l2m2m",
    "--force-window=yes",
];

/// TV static: grey noise generated live by FFmpeg's `lavfi` at a low
/// resolution and scaled up by MPV, so it costs little CPU. The generator
/// never ends, so no file loops.
pub const STATIC_SOURCE: &str =
    "av://lavfi:nullsrc=s=320x180:r=25,geq=lum='random(1)*255':cb=128:cr=128";

/// The stand-by card's picture: SMPTE HD colour bars generated live by
/// FFmpeg's `lavfi`. The picture never changes, so one frame a second is
/// enough and keeps MPV almost idle. Silent: the source has no audio.
pub const STANDBY_SOURCE: &str = "av://lavfi:smptehdbars=s=1920x1080:r=1";

/// The message across the middle of the stand-by card.
pub const STANDBY_TEXT: &str = "PLEASE STAND BY";

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

/// How to run MPV.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MpvConfig {
    /// The MPV executable.
    pub binary: PathBuf,
    /// A directory only this user can open (mode 700). It holds the IPC
    /// socket and the overlay bitmaps. It is created if missing; an existing
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
    /// screen.
    pub standby_text: String,
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
            command_timeout: Duration::from_secs(5),
            start_timeout: Duration::from_secs(10),
        }
    }

    fn socket(&self) -> PathBuf {
        self.runtime_dir.join("mpv.sock")
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
        let mut player = Self {
            config,
            resolver,
            child: None,
            session: None,
            last_start: Instant::now(),
            last_ping: Instant::now(),
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
        let session = loop {
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
        let text = self.config.standby_text.clone();
        self.with_session(|s| s.show_standby(&source, &text))
    }

    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.with_session(|s| s.play_clip(path))
    }

    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        let stream = self.resolver.stream(media, start_ms)?;
        self.with_session(|s| s.queue_program(&stream, start_ms))
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

    fn poll_event(&mut self) -> Option<PlayerEvent> {
        if let Some(event) = self.check_alive() {
            return Some(event);
        }
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
