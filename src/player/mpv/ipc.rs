//! One connection to MPV's JSON IPC socket, and what MPV's playlist holds.
//!
//! MPV speaks one JSON object per line. A command is
//! `{"command": [...], "request_id": N}` and its reply carries the same
//! `request_id`. Events (`{"event": ...}`) can arrive at any time, also while
//! a reply is awaited; they are queued and translated in order by
//! [`Session::poll`].

use std::collections::{HashMap, VecDeque};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::Stream;
use super::banner::{BANNER_OSD_ID, RES};
use crate::player::PlayerEvent;

/// Why a command did not work.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum IpcError {
    /// The connection is unusable: closed, timed out or garbled. The MPV
    /// process behind it has to be replaced.
    Lost(String),
    /// MPV answered and refused the command. The connection still works.
    Rejected(String),
}

impl IpcError {
    pub(super) fn message(&self) -> &str {
        match self {
            Self::Lost(m) | Self::Rejected(m) => m,
        }
    }
}

/// What a playlist entry is, by MPV's `playlist_entry_id`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Entry {
    /// Static or the stand-by card. Neither reports anything.
    Static,
    Clip,
    Program {
        /// The media position the stream begins at.
        stream_offset_ms: u64,
        /// The media position it was asked to start at.
        start_ms: u64,
        /// Whether its start has been reported.
        started: bool,
    },
}

pub(super) struct Session {
    stream: UnixStream,
    buf: Vec<u8>,
    next_request: u64,
    timeout: Duration,
    /// Raw MPV events not yet translated.
    events: VecDeque<Value>,
    /// The entries this session put in MPV's playlist and that are still in
    /// it. A command that replaces the playlist forgets the old ones, so
    /// their `end-file` events are ignored.
    entries: HashMap<i64, Entry>,
    /// The entry on screen, from the last `start-file` event.
    current: Option<i64>,
    /// Whether the stand-by card is up.
    standby: bool,
    /// The subtitle track id of the card's flicker, while one shows.
    flicker: Option<i64>,
    /// The `glsl-shaders` list outside the stand-by card.
    global_shaders: Vec<String>,
    /// The `glsl-shaders` list while the stand-by card shows.
    standby_shaders: Vec<String>,
    /// The `glsl-shaders` list MPV has now, as far as this session knows.
    shaders: Vec<String>,
    /// Whether the channel banner is up.
    banner: bool,
}

impl Session {
    /// Connects to the socket at `path`. Every command waits at most
    /// `timeout` for its reply.
    pub(super) fn connect(path: &Path, timeout: Duration) -> std::io::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_write_timeout(Some(timeout))?;
        Ok(Self {
            stream,
            buf: Vec::new(),
            next_request: 0,
            timeout,
            events: VecDeque::new(),
            entries: HashMap::new(),
            current: None,
            standby: false,
            flicker: None,
            global_shaders: Vec::new(),
            standby_shaders: Vec::new(),
            shaders: Vec::new(),
            banner: false,
        })
    }

    /// Sets the shader files for everything on screen (`global`) and for the
    /// stand-by card (`standby`), and gives MPV the global ones. A new MPV
    /// starts with none, so an empty `global` sends nothing.
    pub(super) fn use_shaders(
        &mut self,
        global: Vec<String>,
        standby: Vec<String>,
    ) -> Result<(), IpcError> {
        self.global_shaders = global;
        self.standby_shaders = standby;
        if self.shaders != self.global_shaders {
            self.set_shaders(self.global_shaders.clone())?;
        }
        Ok(())
    }

    /// Whether the stand-by card is up.
    pub(super) fn standby_up(&self) -> bool {
        self.standby
    }

    /// Plays `source`, replacing the playlist. `loop-file=inf` restarts a
    /// source that ends; a `lavfi` generator never does.
    pub(super) fn show_static(&mut self, source: &str) -> Result<(), IpcError> {
        self.replace(source, json!({ "loop-file": "inf" }), Entry::Static)
    }

    /// Switches MPV to the stand-by shaders and plays `source` as
    /// [`Session::show_static`] does, with the ASS file `subtitles` (the
    /// card's band and text) as its subtitle track. `blend-subtitles=yes`
    /// puts the track through the shaders, and
    /// `secondary-sub-ass-override=no` keeps a flicker's drawings (MPV
    /// strips the styling of secondary subtitles by default). These options
    /// belong to the card's playlist entry, so MPV drops them with it:
    /// whatever replaces the card shows no subtitles. On an error the card
    /// may be partly set up; the caller replaces it.
    pub(super) fn show_standby(&mut self, source: &str, subtitles: &str) -> Result<(), IpcError> {
        self.hide_banner()?;
        self.leave_standby()?;
        if self.shaders != self.standby_shaders {
            self.set_shaders(self.standby_shaders.clone())?;
        }
        let options = json!({
            "loop-file": "inf",
            "sub-files-append": subtitles,
            "blend-subtitles": "yes",
            "secondary-sub-ass-override": "no",
        });
        let id = self.loadfile(source, "replace", options)?;
        self.entries.clear();
        self.entries.insert(id, Entry::Static);
        self.standby = true;
        Ok(())
    }

    /// Shows the ASS file `subtitles` over the card as MPV's secondary
    /// subtitle track, for a flicker. The card's own track stays selected
    /// underneath, so no frame shows the bars without the band. (Rewriting
    /// that track and `sub-reload` left one or two frames without it.) Does
    /// nothing when the card is not up or already flickers.
    pub(super) fn start_flicker(&mut self, subtitles: &str) -> Result<(), IpcError> {
        if !self.standby || self.flicker.is_some() {
            return Ok(());
        }
        self.command(json!(["sub-add", subtitles, "auto"]))?;
        // MPV 0.40 and 0.41 return no track id from sub-add.
        let tracks = self.command(json!(["get_property", "track-list"]))?;
        let id = tracks
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["type"] == "sub" && t["external-filename"] == subtitles)
            .filter_map(|t| t["id"].as_i64())
            .max()
            .ok_or_else(|| IpcError::Rejected("MPV did not add the flicker's subtitles".into()))?;
        self.flicker = Some(id);
        self.command(json!(["set_property", "secondary-sid", id]))
            .map(drop)
    }

    /// Hides and removes the flicker's track, if there is one.
    pub(super) fn end_flicker(&mut self) -> Result<(), IpcError> {
        let Some(id) = self.flicker.take() else {
            return Ok(());
        };
        self.command(json!(["set_property", "secondary-sid", "no"]))?;
        self.command(json!(["sub-remove", id])).map(drop)
    }

    fn set_shaders(&mut self, shaders: Vec<String>) -> Result<(), IpcError> {
        self.command(json!(["set_property", "glsl-shaders", shaders]))?;
        self.shaders = shaders;
        Ok(())
    }

    /// Plays the local file at `path`, replacing the playlist.
    pub(super) fn play_clip(&mut self, path: &Path) -> Result<(), IpcError> {
        let path = clip_path(path)?;
        self.replace(&path, json!({}), Entry::Clip)
    }

    /// Plays the local file at `path` over and over, replacing the playlist.
    pub(super) fn loop_clip(&mut self, path: &Path) -> Result<(), IpcError> {
        let path = clip_path(path)?;
        self.replace(&path, json!({ "loop-file": "inf" }), Entry::Clip)
    }

    /// Queues the local file at `path` behind the current entry.
    pub(super) fn queue_clip(&mut self, path: &Path) -> Result<(), IpcError> {
        self.append_clip(path, json!({}))
    }

    /// Queues the local file at `path` behind the current entry, to play
    /// over and over once it starts.
    pub(super) fn queue_looping_clip(&mut self, path: &Path) -> Result<(), IpcError> {
        self.append_clip(path, json!({ "loop-file": "inf" }))
    }

    fn append_clip(&mut self, path: &Path, options: Value) -> Result<(), IpcError> {
        let path = clip_path(path)?;
        let id = self.loadfile(&path, "append", options)?;
        self.entries.insert(id, Entry::Clip);
        Ok(())
    }

    /// Removes the banner and leaves the stand-by card, then loads `url` in
    /// place of the playlist.
    fn replace(&mut self, url: &str, options: Value, entry: Entry) -> Result<(), IpcError> {
        self.hide_banner()?;
        self.leave_standby()?;
        let id = self.loadfile(url, "replace", options)?;
        self.entries.clear();
        self.entries.insert(id, entry);
        Ok(())
    }

    /// Forgets the stand-by card and gives MPV the global shaders if it has
    /// others. The card's subtitles, a flicker's included, need no command:
    /// they leave with its playlist entry.
    fn leave_standby(&mut self) -> Result<(), IpcError> {
        self.standby = false;
        self.flicker = None;
        if self.shaders != self.global_shaders {
            self.set_shaders(self.global_shaders.clone())?;
        }
        Ok(())
    }

    /// Appends `stream` to the playlist, to start at media position
    /// `start_ms`.
    ///
    /// Its HTTP headers are set as MPV's global `http-header-fields`
    /// property first, so they reach MPV only through this socket. They
    /// cannot be the entry's own option: MPV's playlist prefetch opens the
    /// stream before per-entry options apply, and the request goes out
    /// without them (seen with MPV 0.41). A stream without headers clears the
    /// property, so a key is never sent to another server.
    pub(super) fn queue_program(&mut self, stream: &Stream, start_ms: u64) -> Result<(), IpcError> {
        let fields = header_fields(&stream.headers).map_err(IpcError::Rejected)?;
        self.leave_standby()?;
        self.command(json!(["set_property", "http-header-fields", fields]))?;
        let (options, entry) = program_entry(stream, start_ms);
        let id = self.loadfile(&stream.url, "append", options)?;
        self.entries.insert(id, entry);
        Ok(())
    }

    /// Plays `stream` from media position `start_ms` in place of the
    /// playlist, with its headers set as [`Session::queue_program`] sets them.
    pub(super) fn play_program(&mut self, stream: &Stream, start_ms: u64) -> Result<(), IpcError> {
        let fields = header_fields(&stream.headers).map_err(IpcError::Rejected)?;
        self.hide_banner()?;
        self.leave_standby()?;
        self.command(json!(["set_property", "http-header-fields", fields]))?;
        let (options, entry) = program_entry(stream, start_ms);
        let id = self.loadfile(&stream.url, "replace", options)?;
        self.entries.clear();
        self.entries.insert(id, entry);
        Ok(())
    }

    /// Shows the ASS events `ass` as the banner, an `osd-overlay` on a
    /// 1920x1080 layout that MPV scales to the screen.
    pub(super) fn show_banner(&mut self, ass: &str) -> Result<(), IpcError> {
        self.command(json!({
            "name": "osd-overlay",
            "id": BANNER_OSD_ID,
            "format": "ass-events",
            "data": ass,
            "res_x": RES.0,
            "res_y": RES.1,
            "z": 10,
        }))?;
        self.banner = true;
        Ok(())
    }

    /// Removes the banner, if it is up.
    pub(super) fn hide_banner(&mut self) -> Result<(), IpcError> {
        if !self.banner {
            return Ok(());
        }
        self.banner = false;
        self.command(json!({
            "name": "osd-overlay",
            "id": BANNER_OSD_ID,
            "format": "none",
            "data": "",
        }))
        .map(drop)
    }

    /// Seeks the entry on screen to media position `position_ms`.
    pub(super) fn seek(&mut self, position_ms: u64) -> Result<(), IpcError> {
        let offset = match self.current.and_then(|id| self.entries.get(&id)) {
            Some(Entry::Program {
                stream_offset_ms, ..
            }) => *stream_offset_ms,
            _ => 0,
        };
        let target = position_ms.saturating_sub(offset) as f64 / 1000.0;
        self.command(json!(["seek", target, "absolute"])).map(drop)
    }

    /// Stops playback and empties the playlist.
    pub(super) fn stop(&mut self) -> Result<(), IpcError> {
        self.hide_banner()?;
        self.leave_standby()?;
        self.command(json!(["stop"]))?;
        self.entries.clear();
        self.current = None;
        Ok(())
    }

    /// Shows the BGRA bitmap in `file` as overlay `id`.
    pub(super) fn overlay_add(
        &mut self,
        id: u8,
        x: i32,
        y: i32,
        file: &str,
        width: u32,
        height: u32,
    ) -> Result<(), IpcError> {
        let stride = u64::from(width) * 4;
        self.command(json!([
            "overlay-add",
            id,
            x,
            y,
            file,
            0,
            "bgra",
            width,
            height,
            stride
        ]))
        .map(drop)
    }

    pub(super) fn overlay_remove(&mut self, id: u8) -> Result<(), IpcError> {
        self.command(json!(["overlay-remove", id])).map(drop)
    }

    /// Asks MPV for its `pid` property, to show it still answers. Events read
    /// meanwhile are queued for [`Session::poll`].
    pub(super) fn ping(&mut self) -> Result<(), IpcError> {
        self.command(json!(["get_property", "pid"])).map(drop)
    }

    /// Reads one MPV property, for tests that check what MPV is doing.
    #[cfg(test)]
    pub(super) fn get_property(&mut self, name: &str) -> Result<Value, IpcError> {
        self.command(json!(["get_property", name]))
    }

    /// The next [`PlayerEvent`], from the events MPV has sent so far, or
    /// `None` when none of them means anything to the tuner. Never waits for
    /// MPV, except for the one property read that reports a program start.
    pub(super) fn poll(&mut self) -> Result<Option<PlayerEvent>, IpcError> {
        self.read_available()?;
        while let Some(event) = self.events.pop_front() {
            if let Some(translated) = self.translate(&event)? {
                return Ok(Some(translated));
            }
        }
        Ok(None)
    }

    fn translate(&mut self, event: &Value) -> Result<Option<PlayerEvent>, IpcError> {
        let entry_id = event.get("playlist_entry_id").and_then(Value::as_i64);
        match event.get("event").and_then(Value::as_str) {
            Some("start-file") => {
                self.current = entry_id;
                Ok(None)
            }
            Some("end-file") => {
                let entry = entry_id.and_then(|id| self.entries.remove(&id));
                let on_screen = self.current == entry_id;
                if on_screen {
                    self.current = None;
                }
                let playable = matches!(entry, Some(Entry::Clip | Entry::Program { .. }));
                Ok(match event.get("reason").and_then(Value::as_str) {
                    Some("eof") if entry == Some(Entry::Clip) => Some(PlayerEvent::ClipEnded),
                    Some("error") if on_screen && playable => Some(PlayerEvent::Failed),
                    _ => None,
                })
            }
            // Playback (re)started after a load or a seek. Only the first one
            // of a program is its start.
            Some("playback-restart") => {
                let Some(id) = self.current else {
                    return Ok(None);
                };
                let Some(Entry::Program {
                    stream_offset_ms,
                    start_ms,
                    started: started @ false,
                }) = self.entries.get_mut(&id)
                else {
                    return Ok(None);
                };
                *started = true;
                let (offset, fallback) = (*stream_offset_ms, *start_ms);
                let position_ms = match self.command(json!(["get_property", "time-pos"])) {
                    Ok(Value::Number(n)) => n
                        .as_f64()
                        .filter(|s| *s >= 0.0)
                        .map_or(fallback, |s| (s * 1000.0).round() as u64 + offset),
                    Ok(_) | Err(IpcError::Rejected(_)) => fallback,
                    Err(lost) => return Err(lost),
                };
                let duration_ms = match self.command(json!(["get_property", "duration"])) {
                    Ok(Value::Number(n)) => n
                        .as_f64()
                        .filter(|s| *s >= 0.0)
                        .map(|s| (s * 1000.0).round() as u64)
                        .unwrap_or(0),
                    Ok(_) | Err(IpcError::Rejected(_)) => 0,
                    Err(lost) => return Err(lost),
                };
                Ok(Some(PlayerEvent::ProgramStarted {
                    position_ms,
                    duration_ms: duration_ms.saturating_add(offset),
                }))
            }
            _ => Ok(None),
        }
    }

    /// `loadfile url flags -1 options`, returning the new entry's id.
    fn loadfile(&mut self, url: &str, flags: &str, options: Value) -> Result<i64, IpcError> {
        let data = self.command(json!(["loadfile", url, flags, -1, options]))?;
        data.get("playlist_entry_id")
            .and_then(Value::as_i64)
            .ok_or_else(|| {
                IpcError::Rejected("MPV did not return a playlist entry id (needs MPV 0.38)".into())
            })
    }

    /// Sends one command and waits for its reply. Error messages name the
    /// command but never its arguments, which can hold an HTTP header.
    fn command(&mut self, args: Value) -> Result<Value, IpcError> {
        let name = args[0]
            .as_str()
            .or_else(|| args["name"].as_str())
            .unwrap_or("command")
            .to_owned();
        self.next_request += 1;
        let request_id = self.next_request;
        let mut line = json!({ "command": args, "request_id": request_id }).to_string();
        line.push('\n');
        self.stream
            .write_all(line.as_bytes())
            .map_err(|e| IpcError::Lost(format!("cannot send {name} to MPV: {e}")))?;

        let deadline = Instant::now() + self.timeout;
        loop {
            while let Some(message) = self.next_message() {
                if message.get("request_id").and_then(Value::as_u64) == Some(request_id) {
                    return match message.get("error").and_then(Value::as_str) {
                        Some("success") => Ok(message.get("data").cloned().unwrap_or(Value::Null)),
                        error => Err(IpcError::Rejected(format!(
                            "MPV rejected {name}: {}",
                            error.unwrap_or("no reason given")
                        ))),
                    };
                }
                if message.get("event").is_some() {
                    self.events.push_back(message);
                }
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(IpcError::Lost(format!("MPV did not answer {name}")));
            }
            self.read_blocking(left, &name)?;
        }
    }

    /// The next complete line as JSON. Lines that are not JSON are skipped.
    fn next_message(&mut self) -> Option<Value> {
        while let Some(end) = self.buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=end).collect();
            if let Ok(value) = serde_json::from_slice(&line) {
                return Some(value);
            }
        }
        None
    }

    fn read_blocking(&mut self, wait: Duration, name: &str) -> Result<(), IpcError> {
        self.stream
            .set_read_timeout(Some(wait))
            .map_err(|e| IpcError::Lost(e.to_string()))?;
        let mut chunk = [0; 8192];
        match self.stream.read(&mut chunk) {
            Ok(0) => Err(IpcError::Lost("MPV closed the IPC socket".into())),
            Ok(n) => {
                self.buf.extend_from_slice(&chunk[..n]);
                Ok(())
            }
            Err(e) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                Err(IpcError::Lost(format!("MPV did not answer {name}")))
            }
            Err(e) if e.kind() == ErrorKind::Interrupted => Ok(()),
            Err(e) => Err(IpcError::Lost(format!("cannot read from MPV: {e}"))),
        }
    }

    /// Reads whatever MPV has sent without waiting and queues its events.
    fn read_available(&mut self) -> Result<(), IpcError> {
        let lost = |e: std::io::Error| IpcError::Lost(format!("cannot read from MPV: {e}"));
        self.stream.set_nonblocking(true).map_err(lost)?;
        let mut chunk = [0; 8192];
        let result = loop {
            match self.stream.read(&mut chunk) {
                Ok(0) => break Err(IpcError::Lost("MPV closed the IPC socket".into())),
                Ok(n) => self.buf.extend_from_slice(&chunk[..n]),
                Err(e) if e.kind() == ErrorKind::WouldBlock => break Ok(()),
                Err(e) if e.kind() == ErrorKind::Interrupted => {}
                Err(e) => break Err(lost(e)),
            }
        };
        self.stream.set_nonblocking(false).map_err(lost)?;
        result?;
        while let Some(message) = self.next_message() {
            if message.get("event").is_some() {
                self.events.push_back(message);
            }
        }
        Ok(())
    }
}

/// The `loadfile` options and the playlist entry of a program that plays
/// `stream` from media position `start_ms`.
fn program_entry(stream: &Stream, start_ms: u64) -> (Value, Entry) {
    let start = start_ms.saturating_sub(stream.stream_offset_ms);
    // `sid` off: a program never shows the subtitle track MPV would pick
    // from the file (the device profile offers every subtitle format only
    // so that Jellyfin does not transcode for them).
    let options = json!({ "start": seconds(start), "sid": "no" });
    let entry = Entry::Program {
        stream_offset_ms: stream.stream_offset_ms,
        start_ms,
        started: false,
    };
    (options, entry)
}

/// `path` made absolute, as the text MPV's `loadfile` takes.
fn clip_path(path: &Path) -> Result<String, IpcError> {
    let absolute =
        std::path::absolute(path).map_err(|e| IpcError::Rejected(format!("bad clip path: {e}")))?;
    absolute
        .into_os_string()
        .into_string()
        .map_err(|_| IpcError::Rejected("the clip path is not UTF-8".into()))
}

/// Milliseconds as the seconds string MPV's `start` option takes.
pub(super) fn seconds(ms: u64) -> String {
    format!("{}.{:03}", ms / 1000, ms % 1000)
}

/// The items of MPV's `http-header-fields` list, one `Name: value` each. A
/// header with a line break, or a name with a colon, is refused, because it
/// would let one header smuggle in another.
pub(super) fn header_fields(headers: &[(String, String)]) -> Result<Vec<String>, String> {
    headers
        .iter()
        .map(|(name, value)| {
            if name.is_empty() || name.contains([':', '\r', '\n']) || value.contains(['\r', '\n']) {
                Err(format!("invalid HTTP header {name:?}"))
            } else {
                Ok(format!("{name}: {value}"))
            }
        })
        .collect()
}
