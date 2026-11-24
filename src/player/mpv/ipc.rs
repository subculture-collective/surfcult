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
use super::glitch::{STRIPS, Tear};
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

/// The `osd-overlay` id of the stand-by card's band and text. These ids are
/// separate from the `overlay-add` layers that [`Session::overlay_add`] uses.
const STANDBY_OVERLAY: u32 = 1;

/// The stand-by card's layout, in pixels of a 1920x1080 screen. MPV scales
/// it to the real one.
const CARD_WIDTH: u32 = 1920;
const CARD_HEIGHT: u32 = 1080;
const BAND_TOP: u32 = 430;
const BAND_BOTTOM: u32 = 650;
/// How far the red and cyan copies of the text sit from the white one.
const CHROMA_SHIFT: u32 = 8;

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
    /// The text of the stand-by card, while its overlay is up.
    standby: Option<String>,
    /// The `glsl-shaders` list outside the stand-by card.
    global_shaders: Vec<String>,
    /// The `glsl-shaders` list while the stand-by card shows.
    standby_shaders: Vec<String>,
    /// The `glsl-shaders` list MPV has now, as far as this session knows.
    shaders: Vec<String>,
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
            standby: None,
            global_shaders: Vec::new(),
            standby_shaders: Vec::new(),
            shaders: Vec::new(),
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

    /// Whether the stand-by card's band and text are up.
    pub(super) fn standby_up(&self) -> bool {
        self.standby.is_some()
    }

    /// Plays `source`, replacing the playlist. `loop-file=inf` restarts a
    /// source that ends; a `lavfi` generator never does.
    pub(super) fn show_static(&mut self, source: &str) -> Result<(), IpcError> {
        self.replace(source, json!({ "loop-file": "inf" }), Entry::Static)
    }

    /// Switches MPV to the stand-by shaders, plays `source` as
    /// [`Session::show_static`] does and draws the card's band and `text`
    /// over it. On an error the card may be partly drawn; the caller replaces
    /// it.
    pub(super) fn show_standby(&mut self, source: &str, text: &str) -> Result<(), IpcError> {
        self.leave_standby()?;
        if self.shaders != self.standby_shaders {
            self.set_shaders(self.standby_shaders.clone())?;
        }
        let id = self.loadfile(source, "replace", json!({ "loop-file": "inf" }))?;
        self.entries.clear();
        self.entries.insert(id, Entry::Static);
        self.draw_standby(&standby_ass(text))?;
        self.standby = Some(text.to_owned());
        Ok(())
    }

    /// Redraws the stand-by card's band torn by `tear`, or as normal with
    /// `None`. Does nothing when the card is not up.
    pub(super) fn redraw_standby(&mut self, tear: Option<&Tear>) -> Result<(), IpcError> {
        let Some(text) = self.standby.clone() else {
            return Ok(());
        };
        let ass = match tear {
            Some(tear) => torn_standby_ass(&text, tear),
            None => standby_ass(&text),
        };
        self.draw_standby(&ass)
    }

    /// Draws `ass` as the stand-by card's overlay, replacing what it showed.
    fn draw_standby(&mut self, ass: &str) -> Result<(), IpcError> {
        self.command(json!({
            "name": "osd-overlay",
            "id": STANDBY_OVERLAY,
            "format": "ass-events",
            "data": ass,
            "res_x": CARD_WIDTH,
            "res_y": CARD_HEIGHT,
            "z": 0,
        }))
        .map(drop)
    }

    fn set_shaders(&mut self, shaders: Vec<String>) -> Result<(), IpcError> {
        self.command(json!(["set_property", "glsl-shaders", shaders]))?;
        self.shaders = shaders;
        Ok(())
    }

    /// Plays the local file at `path`, replacing the playlist.
    pub(super) fn play_clip(&mut self, path: &Path) -> Result<(), IpcError> {
        let absolute = std::path::absolute(path)
            .map_err(|e| IpcError::Rejected(format!("bad clip path: {e}")))?;
        let path = absolute
            .to_str()
            .ok_or_else(|| IpcError::Rejected("the clip path is not UTF-8".into()))?;
        self.replace(path, json!({}), Entry::Clip)
    }

    /// Leaves the stand-by card, then loads `url` in place of the playlist.
    fn replace(&mut self, url: &str, options: Value, entry: Entry) -> Result<(), IpcError> {
        self.leave_standby()?;
        let id = self.loadfile(url, "replace", options)?;
        self.entries.clear();
        self.entries.insert(id, entry);
        Ok(())
    }

    /// Removes the stand-by card's overlay if it is up, and gives MPV the
    /// global shaders if it has others. MPV refuses `osd-overlay` without
    /// `data`, even for the format `none`.
    fn leave_standby(&mut self) -> Result<(), IpcError> {
        if self.standby.is_some() {
            self.command(json!({
                "name": "osd-overlay",
                "id": STANDBY_OVERLAY,
                "format": "none",
                "data": "",
            }))?;
            self.standby = None;
        }
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
        let start = start_ms.saturating_sub(stream.stream_offset_ms);
        let options = json!({ "start": seconds(start) });
        let id = self.loadfile(&stream.url, "append", options)?;
        self.entries.insert(
            id,
            Entry::Program {
                stream_offset_ms: stream.stream_offset_ms,
                start_ms,
                started: false,
            },
        );
        Ok(())
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
                if self.current == entry_id {
                    self.current = None;
                }
                let eof = event.get("reason").and_then(Value::as_str) == Some("eof");
                Ok((eof && entry == Some(Entry::Clip)).then_some(PlayerEvent::ClipEnded))
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
                Ok(Some(PlayerEvent::ProgramStarted { position_ms }))
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

/// The stand-by card as ASS events for a 1920x1080 screen, one per line: a
/// black band across the middle, then `text` three times in bold condensed
/// capitals, red shifted left, cyan shifted right and white on top. Later
/// events are drawn over earlier ones.
pub(super) fn standby_ass(text: &str) -> String {
    let text = ass_escape(&text.to_uppercase());
    let (mid_x, mid_y) = (CARD_WIDTH / 2, (BAND_TOP + BAND_BOTTOM) / 2);
    let band = format!(
        r"{{\an7\pos(0,0)\bord0\shad0\1c&H000000&\p1}}m 0 {BAND_TOP} l {CARD_WIDTH} {BAND_TOP} {CARD_WIDTH} {BAND_BOTTOM} 0 {BAND_BOTTOM}{{\p0}}"
    );
    // DejaVu Sans is on the Pi; `\fscx80` narrows it to a condensed face.
    // ASS colours are &HBBGGRR&.
    let layer = |x: u32, colour: &str| {
        format!(
            r"{{\an5\pos({x},{mid_y})\fnDejaVu Sans\b1\fs150\fscx80\bord0\shad0\1c&H{colour}&}}{text}"
        )
    };
    [
        band,
        layer(mid_x - CHROMA_SHIFT, "0000FF"),
        layer(mid_x + CHROMA_SHIFT, "FFFF00"),
        layer(mid_x, "FFFFFF"),
    ]
    .join("\n")
}

/// [`standby_ass`] torn by `tear`: the band is cut into horizontal strips
/// that each move sideways by their offset, and the red and cyan copies of
/// the text sit `tear.chroma` pixels from the white one. Each strip shows
/// only its own rows of the text, through a `\clip`.
pub(super) fn torn_standby_ass(text: &str, tear: &Tear) -> String {
    let text = ass_escape(&text.to_uppercase());
    let mid_y = (BAND_TOP + BAND_BOTTOM) / 2;
    let height = BAND_BOTTOM - BAND_TOP;
    let (width, mid_x) = (CARD_WIDTH as i32, (CARD_WIDTH / 2) as i32);
    let chroma = tear.chroma as i32;
    let mut events = Vec::new();
    for (i, &dx) in tear.strips.iter().enumerate() {
        let top = BAND_TOP + height * i as u32 / STRIPS as u32;
        let bottom = BAND_TOP + height * (i as u32 + 1) / STRIPS as u32;
        events.push(format!(
            r"{{\an7\pos(0,0)\bord0\shad0\1c&H000000&\p1}}m {dx} {top} l {} {top} {} {bottom} {dx} {bottom}{{\p0}}",
            width + dx,
            width + dx
        ));
        for (shift, colour) in [(-chroma, "0000FF"), (chroma, "FFFF00"), (0, "FFFFFF")] {
            events.push(format!(
                r"{{\an5\pos({},{mid_y})\clip(0,{top},{CARD_WIDTH},{bottom})\fnDejaVu Sans\b1\fs150\fscx80\bord0\shad0\1c&H{colour}&}}{text}",
                mid_x + dx + shift
            ));
        }
    }
    events.join("\n")
}

/// `text` as literal ASS text: a backslash cannot start an escape, a brace
/// cannot open an override block, and a line break becomes `\N`.
fn ass_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            // A word joiner after the backslash keeps it from escaping the
            // next character, as MPV does for its own OSD text.
            '\\' => escaped.push_str("\\\u{2060}"),
            '{' => escaped.push_str(r"\{"),
            '}' => escaped.push_str(r"\}"),
            '\n' => escaped.push_str(r"\N"),
            '\r' => {}
            c => escaped.push(c),
        }
    }
    escaped
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
