//! The IPC protocol is tested against a fake MPV: a Unix socket in a test
//! thread that records each command and answers it the way MPV does. The
//! tests in `real` run the `mpv` binary itself, headless, on clips made by
//! `ffmpeg`; they skip with a note when either is missing.

use std::io::{BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tempfile::TempDir;

use super::ipc::{IpcError, Session, header_fields, seconds, standby_ass, torn_standby_ass};
use super::*;

struct FakeMpv {
    _dir: TempDir,
    socket: PathBuf,
    requests: Receiver<Value>,
    writer: Arc<Mutex<Option<UnixStream>>>,
    time_pos: Arc<Mutex<Value>>,
    reject: Arc<Mutex<Option<String>>>,
    /// When set, `reject` applies only to the command with this name.
    reject_only: Arc<Mutex<Option<String>>>,
    silent: Arc<AtomicBool>,
}

impl FakeMpv {
    /// Listens on a new socket. Every `loadfile` gets the next entry id,
    /// starting at 1; `get_property time-pos` answers `time_pos`; everything
    /// else succeeds with no data.
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("mpv.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        Self::serving(dir, socket, move |serve| {
            if let Ok((stream, _)) = listener.accept() {
                serve(stream);
            }
        })
    }

    /// A fake MPV for [`MpvPlayer::start`] and its restarts: `binary` in
    /// the runtime directory `run` is a script that stands in for MPV. Each
    /// time it runs, the fake listens on `run/mpv.sock` and answers one
    /// connection until it closes.
    fn launched() -> (Self, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        fs::DirBuilder::new().mode(0o700).create(&run).unwrap();
        let trigger = dir.path().join("started");
        let binary = dir.path().join("fake-mpv");
        fs::write(
            &binary,
            format!("#!/bin/sh\ntouch '{}'\nexec sleep 60\n", trigger.display()),
        )
        .unwrap();
        fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
        let socket = run.join("mpv.sock");
        let fake = Self::serving(dir, socket.clone(), move |serve| {
            loop {
                let deadline = Instant::now() + Duration::from_secs(30);
                while fs::remove_file(&trigger).is_err() {
                    if Instant::now() > deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                let _ = fs::remove_file(&socket);
                let listener = UnixListener::bind(&socket).unwrap();
                if let Ok((stream, _)) = listener.accept() {
                    serve(stream);
                }
            }
        });
        (fake, binary, run)
    }

    /// Runs `accept` in a thread. It hands each connection it accepts to the
    /// function it is given, which answers it until it closes.
    fn serving(
        dir: TempDir,
        socket: PathBuf,
        accept: impl FnOnce(&mut dyn FnMut(UnixStream)) + Send + 'static,
    ) -> Self {
        let (tx, requests) = mpsc::channel();
        let writer = Arc::new(Mutex::new(None));
        let time_pos = Arc::new(Mutex::new(json!(0.0)));
        let reject = Arc::new(Mutex::new(None::<String>));
        let reject_only = Arc::new(Mutex::new(None::<String>));
        let silent = Arc::new(AtomicBool::new(false));
        let (w, t, r, o, q) = (
            writer.clone(),
            time_pos.clone(),
            reject.clone(),
            reject_only.clone(),
            silent.clone(),
        );
        std::thread::spawn(move || {
            let mut serve = |stream: UnixStream| {
                *w.lock().unwrap() = Some(stream.try_clone().unwrap());
                let mut next_entry = 0;
                for line in BufReader::new(stream).lines() {
                    let Ok(line) = line else { return };
                    let request: Value = serde_json::from_str(&line).unwrap();
                    let id = request["request_id"].clone();
                    let command = &request["command"];
                    let name = command[0]
                        .as_str()
                        .or_else(|| command["name"].as_str())
                        .unwrap_or("")
                        .to_owned();
                    let _ = tx.send(request.clone());
                    if q.load(Ordering::SeqCst) {
                        continue;
                    }
                    let only = o.lock().unwrap().clone();
                    let error = r
                        .lock()
                        .unwrap()
                        .clone()
                        .filter(|_| only.is_none_or(|only| only == name));
                    let reply = if let Some(error) = error {
                        json!({ "request_id": id, "error": error })
                    } else if name == "loadfile" {
                        next_entry += 1;
                        json!({ "request_id": id, "error": "success",
                                "data": { "playlist_entry_id": next_entry } })
                    } else if request["command"] == json!(["get_property", "time-pos"]) {
                        json!({ "request_id": id, "error": "success", "data": t.lock().unwrap().clone() })
                    } else {
                        json!({ "request_id": id, "error": "success", "data": null })
                    };
                    send(&w, &reply);
                }
            };
            accept(&mut serve);
        });
        Self {
            _dir: dir,
            socket,
            requests,
            writer,
            time_pos,
            reject,
            reject_only,
            silent,
        }
    }

    fn connect(&self) -> Session {
        Session::connect(&self.socket, Duration::from_millis(500)).unwrap()
    }

    /// The commands received since the last call, without request ids.
    fn commands(&self) -> Vec<Value> {
        self.requests
            .try_iter()
            .map(|r| r["command"].clone())
            .collect()
    }

    /// Sends an MPV event.
    fn event(&self, event: Value) {
        for _ in 0..200 {
            if self.writer.lock().unwrap().is_some() {
                return send(&self.writer, &event);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("the session never connected");
    }

    fn close(&self) {
        if let Some(stream) = self.writer.lock().unwrap().as_ref() {
            stream.shutdown(Shutdown::Both).unwrap();
        }
    }
}

fn send(writer: &Mutex<Option<UnixStream>>, message: &Value) {
    let mut guard = writer.lock().unwrap();
    let stream = guard.as_mut().unwrap();
    let mut line = message.to_string();
    line.push('\n');
    let _ = stream.write_all(line.as_bytes());
}

fn start_file(id: i64) -> Value {
    json!({ "event": "start-file", "playlist_entry_id": id })
}

fn end_file(id: i64, reason: &str) -> Value {
    json!({ "event": "end-file", "playlist_entry_id": id, "reason": reason })
}

fn restart() -> Value {
    json!({ "event": "playback-restart" })
}

fn keyed_stream() -> Stream {
    Stream {
        url: "http://jellyfin/Videos/abc/stream?static=true".into(),
        headers: vec![("Authorization".into(), "MediaBrowser Token=\"KEY\"".into())],
        stream_offset_ms: 0,
    }
}

/// Polls until nothing is left.
fn drain(session: &mut Session) -> Vec<PlayerEvent> {
    let mut events = Vec::new();
    while let Some(event) = session.poll().unwrap() {
        events.push(event);
    }
    events
}

// Commands

#[test]
fn commands_become_mpv_commands() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.show_static("av://lavfi:noise").unwrap();
    session.play_clip(Path::new("/clips/tune-in.mp4")).unwrap();
    session.queue_program(&keyed_stream(), 125_250).unwrap();
    session.seek(130_000).unwrap();
    session
        .overlay_add(2, 10, 20, "/run/x/overlay-2.bgra", 4, 3)
        .unwrap();
    session.overlay_remove(2).unwrap();
    session.stop().unwrap();
    assert_eq!(
        fake.commands(),
        [
            json!(["loadfile", "av://lavfi:noise", "replace", -1, { "loop-file": "inf" }]),
            json!(["loadfile", "/clips/tune-in.mp4", "replace", -1, {}]),
            json!([
                "set_property",
                "http-header-fields",
                ["Authorization: MediaBrowser Token=\"KEY\""]
            ]),
            json!(["loadfile", "http://jellyfin/Videos/abc/stream?static=true", "append", -1, {
                "start": "125.250"
            }]),
            json!(["seek", 130.0, "absolute"]),
            json!([
                "overlay-add",
                2,
                10,
                20,
                "/run/x/overlay-2.bgra",
                0,
                "bgra",
                4,
                3,
                16
            ]),
            json!(["overlay-remove", 2]),
            json!(["stop"]),
        ]
    );
}

fn standby_overlay() -> Value {
    json!({
        "name": "osd-overlay",
        "id": 1,
        "format": "ass-events",
        "data": standby_ass("PLEASE STAND BY"),
        "res_x": 1920,
        "res_y": 1080,
        "z": 0
    })
}

fn standby_removed() -> Value {
    json!({ "name": "osd-overlay", "id": 1, "format": "none", "data": "" })
}

#[test]
fn the_standby_card_loads_the_bars_and_draws_its_band_and_text() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.show_standby(STANDBY_SOURCE, STANDBY_TEXT).unwrap();
    assert_eq!(
        fake.commands(),
        [
            json!(["loadfile", "av://lavfi:smptehdbars=s=640x360:r=30", "replace", -1, {
                "loop-file": "inf"
            }]),
            standby_overlay(),
        ]
    );
    let card = standby_ass("PLEASE STAND BY");
    let lines: Vec<&str> = card.lines().collect();
    assert_eq!(
        lines,
        [
            r"{\an7\pos(0,0)\bord0\shad0\1c&H000000&\p1}m 0 430 l 1920 430 1920 650 0 650{\p0}",
            r"{\an5\pos(952,540)\fnDejaVu Sans\b1\fs150\fscx80\bord0\shad0\1c&H0000FF&}PLEASE STAND BY",
            r"{\an5\pos(968,540)\fnDejaVu Sans\b1\fs150\fscx80\bord0\shad0\1c&HFFFF00&}PLEASE STAND BY",
            r"{\an5\pos(960,540)\fnDejaVu Sans\b1\fs150\fscx80\bord0\shad0\1c&HFFFFFF&}PLEASE STAND BY",
        ]
    );
}

#[test]
fn the_standby_text_is_capitalized_and_cannot_inject_ass() {
    let card = standby_ass("off {\\p1} air\\n\nnow");
    let text = card.lines().nth(3).unwrap().split_once('}').unwrap().1;
    assert_eq!(text, "OFF \\{\\\u{2060}P1\\} AIR\\\u{2060}N\\NNOW");
}

#[test]
fn whatever_replaces_the_standby_card_removes_its_overlay_first() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    type Replace = fn(&mut Session);
    let replacements: [(&str, Replace); 4] = [
        ("show_static", |s| {
            s.show_static("av://lavfi:noise").unwrap()
        }),
        ("play_clip", |s| {
            s.play_clip(Path::new("/clips/a.mp4")).unwrap()
        }),
        ("queue_program", |s| {
            s.queue_program(&Stream::new("http://jellyfin/x"), 0)
                .unwrap()
        }),
        ("stop", |s| s.stop().unwrap()),
    ];
    for (name, replace) in replacements {
        session.show_standby("av://lavfi:bars", "STAND BY").unwrap();
        fake.commands();
        replace(&mut session);
        let commands = fake.commands();
        assert_eq!(commands[0], standby_removed(), "{name}");
        assert!(!commands[1..].contains(&standby_removed()), "{name}");

        // The overlay is gone, so the next command removes nothing.
        replace(&mut session);
        assert!(!fake.commands().contains(&standby_removed()), "{name}");
    }
}

#[test]
fn a_stream_that_begins_at_an_offset_starts_and_seeks_relative_to_it() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    let stream = Stream {
        stream_offset_ms: 600_000,
        ..Stream::new("http://jellyfin/Videos/abc/stream.ts")
    };
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    session.queue_program(&stream, 600_000).unwrap();
    fake.event(start_file(2));
    *fake.time_pos.lock().unwrap() = json!(1.5);
    fake.event(restart());
    assert_eq!(
        drain(&mut session),
        [PlayerEvent::ProgramStarted {
            position_ms: 601_500
        }]
    );
    session.seek(610_000).unwrap();
    let commands = fake.commands();
    assert_eq!(
        commands[1],
        json!(["set_property", "http-header-fields", []])
    );
    assert_eq!(commands[2][4], json!({ "start": "0.000" }));
    assert_eq!(commands.last().unwrap(), &json!(["seek", 10.0, "absolute"]));
}

#[test]
fn header_fields_refuse_line_breaks_and_colons_in_names() {
    let headers = |name: &str, value: &str| vec![(name.to_owned(), value.to_owned())];
    let two = [
        ("A".to_owned(), "x,y".to_owned()),
        ("B".to_owned(), "2".to_owned()),
    ];
    assert_eq!(header_fields(&two).unwrap(), ["A: x,y", "B: 2"]);
    assert!(header_fields(&headers("A", "1\r\nB: 2")).is_err());
    assert!(header_fields(&headers("A: B", "1")).is_err());
    assert!(header_fields(&headers("", "1")).is_err());
}

#[test]
fn start_positions_are_written_in_seconds_with_milliseconds() {
    assert_eq!(seconds(0), "0.000");
    assert_eq!(seconds(1_005), "1.005");
    assert_eq!(seconds(3_600_250), "3600.250");
}

#[test]
fn a_rejected_command_names_the_command_and_not_its_arguments() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    *fake.reject.lock().unwrap() = Some("loading failed".into());
    let error = session.queue_program(&keyed_stream(), 0).unwrap_err();
    assert_eq!(
        error,
        IpcError::Rejected("MPV rejected set_property: loading failed".into())
    );
    assert!(!error.message().contains("KEY"));
}

#[test]
fn a_command_without_an_answer_loses_the_connection() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    fake.silent.store(true, Ordering::SeqCst);
    assert_eq!(
        session.seek(1_000),
        Err(IpcError::Lost("MPV did not answer seek".into()))
    );
}

#[test]
fn a_closed_socket_loses_the_connection() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.stop().unwrap();
    fake.close();
    assert_eq!(
        session.poll(),
        Err(IpcError::Lost("MPV closed the IPC socket".into()))
    );
}

/// An [`MpvPlayer`] connected to `fake`, with a `sleep` process standing in
/// for MPV. A restart would run a binary that does not exist, so it fails.
fn player_on(fake: &FakeMpv, dir: &Path) -> MpvPlayer<impl StreamResolver> {
    player_with(fake, dir, &[], &DEFAULT_STANDBY_FILTERS)
}

/// [`player_on`] with these global and stand-by filters, set up as
/// [`MpvPlayer::start`] sets them up.
fn player_with(
    fake: &FakeMpv,
    dir: &Path,
    filters: &[Filter],
    standby: &[Filter],
) -> MpvPlayer<impl StreamResolver> {
    let child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .unwrap();
    let config = MpvConfig {
        binary: dir.join("no-mpv"),
        filters: filters.to_vec(),
        standby_filters: standby.to_vec(),
        ..MpvConfig::new(dir)
    };
    let (global_shaders, standby_shaders, glitch) = config.filters_setup().unwrap();
    let mut session = fake.connect();
    session
        .use_shaders(global_shaders.clone(), standby_shaders.clone())
        .unwrap();
    MpvPlayer {
        config,
        resolver: |_: &MediaId, _: u64| Err(PlayerError("no programs here".into())),
        child: Some(child),
        session: Some(session),
        last_start: Instant::now(),
        last_ping: Instant::now(),
        global_shaders,
        standby_shaders,
        glitch,
    }
}

#[test]
fn the_watchdog_asks_mpv_for_its_pid_at_most_once_per_interval() {
    let fake = FakeMpv::new();
    let dir = tempfile::tempdir().unwrap();
    let mut player = player_on(&fake, dir.path());
    assert_eq!(player.poll_event(), None);
    assert!(fake.commands().is_empty());

    player.last_ping = Instant::now() - LIVENESS_INTERVAL;
    assert_eq!(player.poll_event(), None);
    assert_eq!(player.poll_event(), None);
    assert_eq!(fake.commands(), [json!(["get_property", "pid"])]);
    assert!(player.pid().is_some());
}

#[test]
fn a_running_mpv_that_stops_answering_is_shut_down_for_a_restart() {
    let fake = FakeMpv::new();
    let dir = tempfile::tempdir().unwrap();
    let mut player = player_on(&fake, dir.path());
    fake.silent.store(true, Ordering::SeqCst);
    player.last_ping = Instant::now() - LIVENESS_INTERVAL;
    assert_eq!(player.poll_event(), None);
    assert_eq!(fake.commands(), [json!(["get_property", "pid"])]);
    // The hung process is killed and the connection dropped, as after an
    // exit; the next poll after RESTART_DELAY starts a new MPV.
    assert_eq!(player.pid(), None);
    assert!(player.session.is_none());
}

// Events

#[test]
fn a_clip_that_plays_to_its_end_reports_clip_ended() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    fake.event(start_file(1));
    fake.event(end_file(1, "eof"));
    assert_eq!(drain(&mut session), [PlayerEvent::ClipEnded]);
}

#[test]
fn a_replaced_or_stopped_clip_reports_nothing() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    fake.event(start_file(1));
    // Replaced by a newer clip: MPV says `stop`, and the entry is forgotten
    // anyway, so even an `eof` for it would not count.
    session.play_clip(Path::new("/clips/b.mp4")).unwrap();
    fake.event(end_file(1, "stop"));
    fake.event(end_file(1, "eof"));
    fake.event(start_file(2));
    // Replaced by static, then static stopped.
    session.show_static("av://lavfi:noise").unwrap();
    fake.event(end_file(2, "stop"));
    fake.event(start_file(3));
    session.stop().unwrap();
    fake.event(end_file(3, "stop"));
    assert_eq!(drain(&mut session), []);
}

#[test]
fn only_clips_report_clip_ended() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.show_static("av://lavfi:noise").unwrap();
    fake.event(end_file(1, "eof"));
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    session.queue_program(&keyed_stream(), 0).unwrap();
    fake.event(end_file(3, "eof"));
    fake.event(end_file(3, "redirect"));
    fake.event(end_file(2, "error"));
    assert_eq!(drain(&mut session), []);
}

#[test]
fn the_queued_program_reports_its_start_once_with_its_position() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    session.queue_program(&keyed_stream(), 125_000).unwrap();
    // The clip's own playback-restart is not a program start.
    fake.event(start_file(1));
    fake.event(restart());
    fake.event(end_file(1, "eof"));
    fake.event(start_file(2));
    *fake.time_pos.lock().unwrap() = json!(125.0424);
    fake.event(restart());
    // A seek inside the program restarts playback again.
    fake.event(restart());
    assert_eq!(
        drain(&mut session),
        [
            PlayerEvent::ClipEnded,
            PlayerEvent::ProgramStarted {
                position_ms: 125_042
            }
        ]
    );
}

#[test]
fn a_program_start_without_a_position_reports_the_requested_start() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    session.queue_program(&keyed_stream(), 7_000).unwrap();
    fake.event(start_file(2));
    *fake.reject.lock().unwrap() = Some("property unavailable".into());
    fake.event(restart());
    assert_eq!(
        drain(&mut session),
        [PlayerEvent::ProgramStarted { position_ms: 7_000 }]
    );
}

#[test]
fn events_that_arrive_while_a_reply_is_awaited_are_kept() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    fake.event(end_file(1, "eof"));
    // The event is read while waiting for the seek's reply.
    session.seek(0).unwrap();
    assert_eq!(drain(&mut session), [PlayerEvent::ClipEnded]);
}

// The process

#[test]
fn the_arguments_hold_the_socket_and_outputs_but_no_header() {
    let config = MpvConfig::new("/run/user/1000/surfcult");
    let args: Vec<String> = args(&config, &config.socket())
        .into_iter()
        .map(|a| a.into_string().unwrap())
        .collect();
    assert!(args.contains(&"--input-ipc-server=/run/user/1000/surfcult/mpv.sock".into()));
    assert!(args.contains(&"--hwdec=drm,v4l2m2m".into()));
    assert!(args.contains(&"--profile=fast".into()));
    assert!(args.contains(&"--input-terminal=no".into()));
    assert!(args.contains(&"--idle=yes".into()));
    for arg in &args {
        assert!(!arg.contains("http-header"), "{arg}");
        assert!(!arg.contains("Authorization"), "{arg}");
        assert!(!arg.starts_with("--include"), "{arg}");
    }
}

#[test]
fn a_stream_debug_print_hides_header_values() {
    let printed = format!("{:?}", keyed_stream());
    assert!(printed.contains("Authorization"));
    assert!(!printed.contains("KEY"));
}

#[test]
fn a_runtime_directory_open_to_others_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let open = dir.path().join("open");
    fs::create_dir(&open).unwrap();
    fs::set_permissions(&open, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(prepare_runtime_dir(&open).is_err());

    let fresh = dir.path().join("a/b");
    prepare_runtime_dir(&fresh).unwrap();
    let mode = fs::metadata(&fresh).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o700);
}

#[test]
fn an_overlay_must_fit_its_pixels_and_layer() {
    let overlay = |width, height, len| Overlay {
        x: 0,
        y: 0,
        width,
        height,
        bgra: vec![0; len],
    };
    assert!(check_overlay(0, &overlay(2, 3, 24)).is_ok());
    assert!(check_overlay(63, &overlay(1, 1, 4)).is_ok());
    assert!(check_overlay(64, &overlay(1, 1, 4)).is_err());
    assert!(check_overlay(0, &overlay(2, 3, 23)).is_err());
    assert!(check_overlay(0, &overlay(0, 0, 0)).is_err());
}

// Display filters

fn shaders(list: &[&str]) -> Value {
    json!(["set_property", "glsl-shaders", list])
}

#[test]
fn the_standby_card_switches_to_its_shaders_and_leaving_it_restores_the_global_ones() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session
        .use_shaders(
            vec!["/run/g.glsl".into()],
            vec!["/run/g.glsl".into(), "/run/crt.glsl".into()],
        )
        .unwrap();
    assert_eq!(fake.commands(), [shaders(&["/run/g.glsl"])]);

    session.show_standby("av://lavfi:bars", "STAND BY").unwrap();
    let commands = fake.commands();
    assert_eq!(commands[0], shaders(&["/run/g.glsl", "/run/crt.glsl"]));
    assert_eq!(commands[1][0], json!("loadfile"));

    // Static, a clip, a program or stop: each leaves the card and restores
    // the global list once.
    session.play_clip(Path::new("/clips/a.mp4")).unwrap();
    let commands = fake.commands();
    assert_eq!(
        commands[..2],
        [standby_removed(), shaders(&["/run/g.glsl"])]
    );
    session.show_static("av://lavfi:noise").unwrap();
    assert!(
        !fake
            .commands()
            .iter()
            .any(|c| c[1] == json!("glsl-shaders"))
    );
}

#[test]
fn without_shaders_on_the_card_no_shader_command_is_sent() {
    let fake = FakeMpv::new();
    let mut session = fake.connect();
    session
        .use_shaders(vec!["/run/crt.glsl".into()], vec!["/run/crt.glsl".into()])
        .unwrap();
    fake.commands();
    session.show_standby("av://lavfi:bars", "STAND BY").unwrap();
    session.stop().unwrap();
    assert!(
        !fake
            .commands()
            .iter()
            .any(|c| c[1] == json!("glsl-shaders"))
    );
}

#[test]
fn the_player_gives_the_card_the_global_filters_then_its_own_once() {
    let dir = tempfile::tempdir().unwrap();
    let crt = dir.path().join("crt.glsl").to_str().unwrap().to_owned();
    // CRT globally and on the card: one copy, already on, so nothing changes.
    let fake = FakeMpv::new();
    let mut player = player_with(
        &fake,
        dir.path(),
        &[Filter::Crt],
        &[Filter::Crt, Filter::Glitch],
    );
    assert_eq!(fake.commands(), [shaders(&[&crt])]);
    player.show_standby().unwrap();
    assert!(
        !fake
            .commands()
            .iter()
            .any(|c| c[1] == json!("glsl-shaders"))
    );
    assert!(player.glitch.is_some());

    // The glitch globally and on the card runs once; there is no shader.
    let fake = FakeMpv::new();
    let player = player_with(&fake, dir.path(), &[Filter::Glitch], &[Filter::Glitch]);
    assert_eq!(
        (player.global_shaders.len(), player.standby_shaders.len()),
        (0, 0)
    );
    assert!(player.glitch.is_some());

    // Nothing on the card: no glitch timer.
    let fake = FakeMpv::new();
    let player = player_with(&fake, dir.path(), &[], &[]);
    assert!(player.glitch.is_none());
    assert!(fake.commands().is_empty());
}

#[test]
fn a_rejected_card_overlay_falls_back_to_static() {
    let fake = FakeMpv::new();
    let dir = tempfile::tempdir().unwrap();
    let mut player = player_on(&fake, dir.path());
    let crt = dir.path().join("crt.glsl").to_str().unwrap().to_owned();
    *fake.reject_only.lock().unwrap() = Some("osd-overlay".into());
    *fake.reject.lock().unwrap() = Some("invalid parameter".into());
    let error = player.show_standby().unwrap_err();
    assert_eq!(
        error,
        PlayerError("MPV rejected osd-overlay: invalid parameter".into())
    );
    let commands = fake.commands();
    assert_eq!(
        commands[..3],
        [
            shaders(&[&crt]),
            json!(["loadfile", STANDBY_SOURCE, "replace", -1, { "loop-file": "inf" }]),
            standby_overlay(),
        ]
    );
    // Static replaces the textless bars, with the global (empty) shaders.
    assert_eq!(
        commands[3..],
        [
            shaders(&[]),
            json!(["loadfile", STATIC_SOURCE, "replace", -1, { "loop-file": "inf" }]),
        ]
    );
    assert!(!player.session.as_ref().unwrap().standby_up());
}

#[test]
fn a_restart_gives_the_new_mpv_the_global_shaders() {
    let (fake, binary, run) = FakeMpv::launched();
    let config = MpvConfig {
        binary,
        filters: vec![Filter::Crt],
        ..MpvConfig::new(&run)
    };
    let crt = run.join("crt.glsl");
    let resolver = |_: &MediaId, _: u64| Err(PlayerError("no programs here".into()));
    let mut player = MpvPlayer::start(config, resolver).unwrap();
    let list = [crt.to_str().unwrap()];
    assert_eq!(fake.commands(), [shaders(&list)]);
    assert_eq!(fs::read_to_string(&crt).unwrap(), CRT_SHADER);

    player.show_standby().unwrap();
    fake.commands();
    let killed = std::process::Command::new("kill")
        .args(["-KILL", &player.pid().unwrap().to_string()])
        .status()
        .unwrap();
    assert!(killed.success());
    let deadline = Instant::now() + Duration::from_secs(10);
    while player.poll_event() != Some(PlayerEvent::Restarted) {
        assert!(Instant::now() < deadline, "no restart");
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(fake.commands(), [shaders(&list)]);
    // The card is gone with the old MPV, so leaving it sends nothing extra.
    player.show_static().unwrap();
    assert_eq!(fake.commands()[0][0], json!("loadfile"));
    drop(player);
    assert!(!crt.exists());
}

#[test]
fn the_glitch_tears_the_band_and_then_restores_it() {
    let fake = FakeMpv::new();
    let dir = tempfile::tempdir().unwrap();
    let mut player = player_on(&fake, dir.path());
    player.show_standby().unwrap();
    fake.commands();
    // Nothing is due yet.
    assert_eq!(player.poll_event(), None);
    assert!(fake.commands().is_empty());

    let past = Instant::now() - Duration::from_millis(1);
    player.glitch.as_mut().unwrap().state = glitch::State::Waiting(past);
    assert_eq!(player.poll_event(), None);
    let commands = fake.commands();
    assert_eq!(commands.len(), 1);
    let torn = commands[0]["data"].as_str().unwrap();
    assert_eq!(commands[0]["name"], json!("osd-overlay"));
    assert_eq!(commands[0]["id"], json!(1));
    assert!(torn.contains(r"\clip("), "{torn}");
    assert!(matches!(
        player.glitch.as_ref().unwrap().state,
        glitch::State::Tearing(_)
    ));

    player.glitch.as_mut().unwrap().state = glitch::State::Tearing(past);
    assert_eq!(player.poll_event(), None);
    assert_eq!(fake.commands(), [standby_overlay()]);
    assert!(matches!(
        player.glitch.as_ref().unwrap().state,
        glitch::State::Waiting(at) if at > Instant::now()
    ));
}

#[test]
fn nothing_glitches_when_the_card_is_not_up() {
    let fake = FakeMpv::new();
    let dir = tempfile::tempdir().unwrap();
    let mut player = player_on(&fake, dir.path());
    player.show_standby().unwrap();
    // The card is replaced in the middle of a glitch.
    player.glitch.as_mut().unwrap().state =
        glitch::State::Tearing(Instant::now() - Duration::from_millis(1));
    player.show_static().unwrap();
    fake.commands();
    assert_eq!(player.poll_event(), None);
    assert!(fake.commands().is_empty());
    assert_eq!(player.glitch.as_ref().unwrap().state, glitch::State::Off);

    // A due glitch with no card draws nothing either.
    player.glitch.as_mut().unwrap().state =
        glitch::State::Waiting(Instant::now() - Duration::from_millis(1));
    assert_eq!(player.poll_event(), None);
    assert!(fake.commands().is_empty());
}

#[test]
fn a_torn_band_moves_each_strip_and_pushes_the_colours_apart() {
    let tear = glitch::Tear {
        strips: [-40, 0, 12, 90],
        chroma: 30,
    };
    let card = torn_standby_ass("stand by", &tear);
    let lines: Vec<&str> = card.lines().collect();
    assert_eq!(lines.len(), 16);
    assert_eq!(
        lines[0],
        r"{\an7\pos(0,0)\bord0\shad0\1c&H000000&\p1}m -40 430 l 1880 430 1880 485 -40 485{\p0}"
    );
    assert_eq!(
        lines[1],
        r"{\an5\pos(890,540)\clip(0,430,1920,485)\fnDejaVu Sans\b1\fs150\fscx80\bord0\shad0\1c&H0000FF&}STAND BY"
    );
    assert!(lines[2].starts_with(r"{\an5\pos(950,540)\clip(0,430,1920,485)"));
    assert!(lines[3].starts_with(r"{\an5\pos(920,540)\clip(0,430,1920,485)"));
    assert!(lines[12].contains("m 90 595 l 2010 595 2010 650 90 650"));
}

/// Tests against the real `mpv` binary, headless (`--vo=null --ao=null`).
mod real {
    use std::io::Read;
    use std::net::TcpListener;
    use std::process::Command;
    use std::time::Instant;

    use super::*;

    /// Whether `mpv` and `ffmpeg` can run here. Prints why a test is skipped.
    fn tools() -> bool {
        let runs = |tool: &str| {
            Command::new(tool)
                .arg(if tool == "mpv" {
                    "--version"
                } else {
                    "-version"
                })
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        let missing: Vec<&str> = ["mpv", "ffmpeg"].into_iter().filter(|t| !runs(t)).collect();
        if !missing.is_empty() {
            eprintln!(
                "skipping the real MPV test: {} not installed",
                missing.join(" and ")
            );
        }
        missing.is_empty()
    }

    /// A test pattern clip of `seconds` in `dir`.
    fn make_clip(dir: &Path, name: &str, seconds: u32) -> PathBuf {
        let path = dir.join(name);
        let status = Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
            .arg("-i")
            .arg(format!("testsrc=duration={seconds}:size=160x120:rate=24"))
            .args(["-pix_fmt", "yuv420p"])
            .arg(&path)
            .status()
            .unwrap();
        assert!(status.success(), "ffmpeg failed");
        path
    }

    fn headless(runtime_dir: &Path) -> MpvConfig {
        MpvConfig {
            output_args: vec!["--vo=null".into(), "--ao=null".into()],
            ..MpvConfig::new(runtime_dir)
        }
    }

    /// Polls until `last` arrives or `seconds` pass, and returns every event.
    fn wait_for(
        player: &mut impl Player,
        seconds: u64,
        last: impl Fn(&PlayerEvent) -> bool,
    ) -> Vec<PlayerEvent> {
        let deadline = Instant::now() + Duration::from_secs(seconds);
        let mut events = Vec::new();
        while Instant::now() < deadline {
            while let Some(event) = player.poll_event() {
                let done = last(&event);
                events.push(event);
                if done {
                    return events;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        events
    }

    const KEY: &str = "MediaBrowser Token=\"sec,ret\\key\"";

    fn auth() -> Vec<(String, String)> {
        vec![("Authorization".into(), KEY.into())]
    }

    #[test]
    fn plays_a_clip_then_the_queued_program_at_its_offset() {
        if !tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path(), "clip.mp4", 1);
        let program = make_clip(dir.path(), "program.mp4", 6);
        let url = program.to_str().unwrap().to_owned();
        let resolver = move |_: &MediaId, _: u64| {
            Ok(Stream {
                headers: auth(),
                ..Stream::new(url.clone())
            })
        };
        let mut player = MpvPlayer::start(headless(&dir.path().join("run")), resolver).unwrap();

        player.show_static().unwrap();
        player.play_clip(&clip).unwrap();
        player.queue_program(&MediaId::new("p"), 2_000).unwrap();
        let events = wait_for(&mut player, 15, |e| {
            matches!(e, PlayerEvent::ProgramStarted { .. })
        });
        let [
            PlayerEvent::ClipEnded,
            PlayerEvent::ProgramStarted { position_ms },
        ] = events[..]
        else {
            panic!("unexpected events {events:?}");
        };
        assert!((1_900..=2_400).contains(&position_ms), "{position_ms}");

        // The key went through the socket, not the process arguments.
        let cmdline = fs::read(format!("/proc/{}/cmdline", player.pid().unwrap())).unwrap();
        let cmdline = String::from_utf8_lossy(&cmdline);
        assert!(!cmdline.contains("sec,ret"), "{cmdline}");
        assert!(!cmdline.contains("Authorization"), "{cmdline}");

        player.seek(4_000).unwrap();
        let overlay = Overlay {
            x: 8,
            y: 8,
            width: 4,
            height: 2,
            bgra: vec![0x80; 32],
        };
        player.show_overlay(5, &overlay).unwrap();
        player.show_overlay(5, &overlay).unwrap();
        player.clear_overlay(5).unwrap();
        player.show_static().unwrap();
        // Static never ends and the program was replaced: neither reports
        // anything.
        assert_eq!(wait_for(&mut player, 2, |_| false), []);
        player.stop().unwrap();
    }

    #[test]
    fn the_standby_card_stays_up_and_a_clip_replaces_it() {
        if !tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path(), "clip.mp4", 1);
        let resolver = |_: &MediaId, _: u64| Err(PlayerError("no programs here".into()));
        let mut player = MpvPlayer::start(headless(&dir.path().join("run")), resolver).unwrap();
        player.show_standby().unwrap();

        // The generator never ends: after a few seconds the bars still play
        // and nothing was reported.
        assert_eq!(wait_for(&mut player, 4, |_| false), []);
        let mut property =
            |name: &str| player.session.as_mut().unwrap().get_property(name).unwrap();
        assert_eq!(property("path"), json!(STANDBY_SOURCE));
        assert_eq!(property("idle-active"), json!(false));
        assert_eq!(property("eof-reached"), json!(false));
        let seconds = property("time-pos").as_f64().unwrap();
        assert!(seconds >= 2.0, "{seconds}");

        player.play_clip(&clip).unwrap();
        let events = wait_for(&mut player, 10, |e| *e == PlayerEvent::ClipEnded);
        assert_eq!(events, [PlayerEvent::ClipEnded]);
    }

    #[test]
    fn sends_the_key_as_an_http_header_and_not_in_the_url() {
        if !tools() {
            return;
        }
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = server.local_addr().unwrap().port();
        let (tx, requests) = mpsc::channel();
        std::thread::spawn(move || {
            for stream in server.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut request = Vec::new();
                let mut byte = [0];
                while !request.ends_with(b"\r\n\r\n") && stream.read(&mut byte).unwrap_or(0) == 1 {
                    request.push(byte[0]);
                }
                let _ = tx.send(String::from_utf8_lossy(&request).into_owned());
                let _ = stream.write_all(
                    b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            }
        });

        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path(), "clip.mp4", 1);
        let resolver = move |media: &MediaId, _: u64| {
            Ok(Stream {
                headers: auth(),
                ..Stream::new(format!(
                    "http://127.0.0.1:{port}/Videos/{}/stream?static=true",
                    media.as_str()
                ))
            })
        };
        let mut player = MpvPlayer::start(headless(&dir.path().join("run")), resolver).unwrap();
        player.play_clip(&clip).unwrap();
        player.queue_program(&MediaId::new("abc"), 0).unwrap();

        let request = requests.recv_timeout(Duration::from_secs(15)).unwrap();
        let first_line = request.lines().next().unwrap();
        assert_eq!(first_line, "GET /Videos/abc/stream?static=true HTTP/1.1");
        assert!(
            request
                .lines()
                .any(|l| l == format!("Authorization: {KEY}")),
            "{request}"
        );
    }

    #[test]
    fn a_killed_mpv_is_restarted_and_plays_again() {
        if !tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let clip = make_clip(dir.path(), "clip.mp4", 1);
        let long = make_clip(dir.path(), "long.mp4", 30);
        let resolver = |_: &MediaId, _: u64| Err(PlayerError("no programs here".into()));
        let mut player = MpvPlayer::start(headless(&dir.path().join("run")), resolver).unwrap();
        player.play_clip(&long).unwrap();
        let first = player.pid().unwrap();

        let killed = Command::new("kill")
            .args(["-KILL", &first.to_string()])
            .status()
            .unwrap();
        assert!(killed.success());
        let events = wait_for(&mut player, 10, |e| *e == PlayerEvent::Restarted);
        assert_eq!(events, [PlayerEvent::Restarted]);
        assert_ne!(player.pid(), Some(first));

        // The new MPV works, and nothing from the old one leaks through.
        player.play_clip(&clip).unwrap();
        let events = wait_for(&mut player, 10, |e| *e == PlayerEvent::ClipEnded);
        assert_eq!(events, [PlayerEvent::ClipEnded]);
    }

    #[test]
    fn a_hung_mpv_is_restarted_without_a_command() {
        if !tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let long = make_clip(dir.path(), "long.mp4", 30);
        let resolver = |_: &MediaId, _: u64| Err(PlayerError("no programs here".into()));
        let config = MpvConfig {
            command_timeout: Duration::from_millis(500),
            ..headless(&dir.path().join("run"))
        };
        let mut player = MpvPlayer::start(config, resolver).unwrap();
        player.play_clip(&long).unwrap();
        let first = player.pid().unwrap();

        // A stopped process is alive but answers nothing.
        let stopped = Command::new("kill")
            .args(["-STOP", &first.to_string()])
            .status()
            .unwrap();
        assert!(stopped.success());
        let events = wait_for(&mut player, 15, |e| *e == PlayerEvent::Restarted);
        assert_eq!(events, [PlayerEvent::Restarted]);
        assert_ne!(player.pid(), Some(first));
    }

    #[test]
    fn the_card_switches_mpv_to_its_shaders_and_its_glitch_is_accepted() {
        if !tools() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        let resolver = |_: &MediaId, _: u64| Err(PlayerError("no programs here".into()));
        let mut player = MpvPlayer::start(headless(&run), resolver).unwrap();
        let crt = run.join("crt.glsl").to_str().unwrap().to_owned();
        let property = |player: &mut MpvPlayer<_>, name: &str| -> Value {
            player.session.as_mut().unwrap().get_property(name).unwrap()
        };
        assert_eq!(property(&mut player, "glsl-shaders"), json!([]));
        player.show_standby().unwrap();
        assert_eq!(property(&mut player, "glsl-shaders"), json!([crt]));

        // A torn band and the settled one are both drawn without an error.
        let past = Instant::now() - Duration::from_millis(1);
        player.glitch.as_mut().unwrap().state = glitch::State::Waiting(past);
        player.step_glitch();
        player.glitch.as_mut().unwrap().state = glitch::State::Tearing(past);
        player.step_glitch();
        assert!(player.session.as_ref().unwrap().standby_up());

        player.show_static().unwrap();
        assert_eq!(property(&mut player, "glsl-shaders"), json!([]));
    }

    /// Compiles the CRT shader on a real GPU, as OpenGL ES like the Pi's.
    /// It needs a display, so it runs only with `SURFCULT_GPU_TEST=1`; a
    /// headless sway gives one (see docs/reference/player.md).
    #[test]
    fn the_crt_shader_compiles_on_a_gpu() {
        if std::env::var_os("SURFCULT_GPU_TEST").is_none() {
            eprintln!("skipping the GPU shader test: SURFCULT_GPU_TEST is not set");
            return;
        }
        assert!(tools(), "the GPU shader test needs mpv and ffmpeg");
        let dir = tempfile::tempdir().unwrap();
        let shader = dir.path().join("crt.glsl");
        fs::write(&shader, CRT_SHADER).unwrap();
        let output = Command::new("mpv")
            .args([
                "--no-config",
                "--vo=gpu",
                "--gpu-api=opengl",
                "--opengl-es=yes",
                "--frames=15",
                "--ao=null",
                "--msg-level=all=warn",
            ])
            .arg(format!("--glsl-shaders={}", shader.display()))
            .arg(STANDBY_SOURCE)
            .output()
            .unwrap();
        let log = String::from_utf8_lossy(&output.stderr).into_owned()
            + &String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "{log}");
        assert!(!log.to_lowercase().contains("error"), "{log}");
        assert!(!log.to_lowercase().contains("fail"), "{log}");
    }
}
