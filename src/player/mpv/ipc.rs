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
        })
    }

    /// Plays `source` in a loop, replacing the playlist.
    pub(super) fn show_static(&mut self, source: &str) -> Result<(), IpcError> {
        let options = json!({ "loop-file": "inf" });
        let id = self.loadfile(source, "replace", options)?;
        self.entries.clear();
        self.entries.insert(id, Entry::Static);
        Ok(())
    }

    /// Plays the local file at `path`, replacing the playlist.
    pub(super) fn play_clip(&mut self, path: &Path) -> Result<(), IpcError> {
        let absolute = std::path::absolute(path)
            .map_err(|e| IpcError::Rejected(format!("bad clip path: {e}")))?;
        let path = absolute
            .to_str()
            .ok_or_else(|| IpcError::Rejected("the clip path is not UTF-8".into()))?;
        let id = self.loadfile(path, "replace", json!({}))?;
        self.entries.clear();
        self.entries.insert(id, Entry::Clip);
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
        let name = args[0].as_str().unwrap_or("command").to_owned();
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
