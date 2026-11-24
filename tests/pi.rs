//! Acceptance checks that need the Raspberry Pi 4B, its TV and the owner's
//! Jellyfin. They are `#[ignore]`d: CI has none of these. Cross-build the test
//! binary on another machine and run it on the Pi (see
//! `docs/runbooks/raspberry-pi.md`, "Run the hardware checks"):
//!
//! ```text
//! SURFCULT_JELLYFIN_URL=http://10.0.0.200:8096 \
//! SURFCULT_JELLYFIN_API_KEY_FILE=~/spike/.jf-key \
//! ./pi-<hash> --ignored --nocapture --test-threads=1
//! ```
//!
//! Each test prints a table of measurements and asserts only what must hold.
//! `SURFCULT_PI_SAMPLES` (default `~/spike/samples`) is the directory of local
//! test files. The Jellyfin items are the ones of the Phase 0 census; their
//! ids can be replaced with `SURFCULT_PI_DIRECT_IDS`, `SURFCULT_PI_TRANSCODE_IDS`
//! (comma separated).

use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use surfcult::jellyfin::{ApiKey, JellyfinClient, JellyfinConfig, Playback, TranscodeSession};
use surfcult::library::MediaId;
use surfcult::player::mpv::{MpvConfig, MpvPlayer, Stream};
use surfcult::player::{Filter, Player, PlayerError, PlayerEvent};

const POLL: Duration = Duration::from_millis(20);

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").expect("HOME"))
}

fn samples() -> PathBuf {
    std::env::var("SURFCULT_PI_SAMPLES")
        .map(PathBuf::from)
        .unwrap_or_else(|_| home().join("spike/samples"))
}

fn sample(name: &str) -> PathBuf {
    let path = samples().join(name);
    assert!(path.exists(), "{} is missing", path.display());
    path
}

fn run_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("surfcult-pi-{tag}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    dir
}

/// The Pi setup of the player, with no sound (the TV's audio is not under
/// test) and the given global filters.
fn config(dir: &Path, filters: Vec<Filter>) -> MpvConfig {
    let mut config = MpvConfig::new(dir);
    config.output_args.push("--ao=null".into());
    config.filters = filters;
    config
}

fn nothing(_: &MediaId, _: u64) -> Result<Stream, PlayerError> {
    Err(PlayerError("no programs in this test".into()))
}

/// A second connection to the player's MPV, to read properties.
struct Probe {
    socket: BufReader<UnixStream>,
    next: u64,
}

impl Probe {
    fn open(dir: &Path) -> Self {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            match UnixStream::connect(dir.join("mpv.sock")) {
                Ok(stream) => {
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    return Self {
                        socket: BufReader::new(stream),
                        next: 1,
                    };
                }
                Err(_) if Instant::now() < deadline => sleep(Duration::from_millis(100)),
                Err(error) => panic!("cannot reach MPV's socket: {error}"),
            }
        }
    }

    fn command(&mut self, args: Value) -> Value {
        let id = self.next;
        self.next += 1;
        let line = json!({ "command": args, "request_id": id }).to_string();
        writeln!(self.socket.get_mut(), "{line}").unwrap();
        loop {
            let mut reply = String::new();
            if self.socket.read_line(&mut reply).unwrap_or(0) == 0 {
                return Value::Null;
            }
            let value: Value = serde_json::from_str(&reply).unwrap_or(Value::Null);
            if value["request_id"] == id {
                return value;
            }
        }
    }

    fn get(&mut self, property: &str) -> Value {
        self.command(json!(["get_property", property]))["data"].clone()
    }

    fn count(&mut self, property: &str) -> i64 {
        self.get(property).as_i64().unwrap_or(0)
    }
}

/// CPU time of a process in seconds, from `/proc`.
fn cpu_seconds(pid: u32) -> f64 {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let after_name = stat.rsplit(')').next().unwrap_or("");
    let fields: Vec<&str> = after_name.split_whitespace().collect();
    // utime and stime are fields 14 and 15 of the line; after the name they
    // are at 11 and 12.
    let ticks: f64 = [11, 12]
        .iter()
        .map(|i| {
            fields
                .get(*i)
                .and_then(|f| f.parse::<f64>().ok())
                .unwrap_or(0.0)
        })
        .sum();
    ticks / 100.0
}

struct Sample {
    drops: i64,
    delayed: i64,
    decoder_drops: i64,
    mpv_cores: f64,
    driver_cores: f64,
    fps: f64,
}

/// Polls `player` every 20 ms for `seconds`, as the caller's loop must for the
/// glitch, and measures MPV and this process.
fn measure<R: surfcult::player::mpv::StreamResolver>(
    player: &mut MpvPlayer<R>,
    probe: &mut Probe,
    seconds: u64,
) -> Sample {
    let pid = player.pid().expect("MPV is running");
    let (d0, v0, dd0) = (
        probe.count("frame-drop-count"),
        probe.count("vo-delayed-frame-count"),
        probe.count("decoder-frame-drop-count"),
    );
    let (c0, own0) = (cpu_seconds(pid), cpu_seconds(std::process::id()));
    let started = Instant::now();
    while started.elapsed() < Duration::from_secs(seconds) {
        let _ = player.poll_event();
        sleep(POLL);
    }
    let wall = started.elapsed().as_secs_f64();
    Sample {
        drops: probe.count("frame-drop-count") - d0,
        delayed: probe.count("vo-delayed-frame-count") - v0,
        decoder_drops: probe.count("decoder-frame-drop-count") - dd0,
        mpv_cores: (cpu_seconds(pid) - c0) / wall,
        driver_cores: (cpu_seconds(std::process::id()) - own0) / wall,
        fps: probe.get("estimated-vf-fps").as_f64().unwrap_or(0.0),
    }
}

fn print_sample(label: &str, s: &Sample, extra: &str) {
    println!(
        "{label:<34} mpv {:.2} cores | player loop {:.3} | vo drops {} | delayed {} | dec drops {} | vf fps {:.1} {extra}",
        s.mpv_cores, s.driver_cores, s.drops, s.delayed, s.decoder_drops, s.fps
    );
}

fn wait_for(
    player: &mut impl Player,
    limit: Duration,
    mut want: impl FnMut(&PlayerEvent) -> bool,
) -> Option<(PlayerEvent, Duration)> {
    let started = Instant::now();
    while started.elapsed() < limit {
        if let Some(event) = player.poll_event()
            && want(&event)
        {
            return Some((event, started.elapsed()));
        }
        sleep(POLL);
    }
    None
}

fn signal(name: &str, pid: u32) {
    let status = Command::new("kill")
        .arg(format!("-{name}"))
        .arg(pid.to_string())
        .status()
        .unwrap();
    assert!(status.success(), "kill -{name} {pid}");
}

fn alive(pid: u32) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|s| !s.contains(") Z") && !s.is_empty())
        .unwrap_or(false)
}

/// A short 1080p H.264 clip, the stand-in for a tune-in clip.
fn tune_in_clip(dir: &Path) -> PathBuf {
    fs::create_dir_all(dir).unwrap();
    let path = dir.join("tune-in.mp4");
    let status = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi"])
        .arg("-i")
        .arg("testsrc=duration=4:size=1280x720:rate=24")
        .args(["-pix_fmt", "yuv420p", "-preset", "ultrafast"])
        .arg(&path)
        .status()
        .unwrap();
    assert!(status.success(), "ffmpeg could not make the clip");
    path
}

// ---------------------------------------------------------------- #22

/// #22: killing MPV during playback restarts it and the player reports
/// `Restarted`; a stopped (hung) MPV is found and replaced too.
#[test]
#[ignore = "needs the Pi and its TV"]
fn mpv_is_restarted_when_it_dies_or_hangs() {
    let dir = run_dir("watchdog");
    let clip = sample("Test_Jellyfin_1080p_HEVC_8bit_5M.mp4");
    let mut player = MpvPlayer::start(config(&dir, vec![]), nothing).unwrap();
    let mut probe = Probe::open(&dir);

    player.play_clip(&clip).unwrap();
    sleep(Duration::from_secs(3));
    let position = probe.get("time-pos").as_f64().unwrap_or(-1.0);
    println!(
        "playing: time-pos {position:.1}s, hwdec {}",
        probe.get("hwdec-current")
    );
    assert!(position > 1.0, "the clip is not playing");

    for (how, kill) in [("kill -9", "KILL"), ("SIGSTOP (hang)", "STOP")] {
        let old = player.pid().unwrap();
        signal(kill, old);
        let (event, took) = wait_for(&mut player, Duration::from_secs(40), |e| {
            *e == PlayerEvent::Restarted
        })
        .unwrap_or_else(|| panic!("{how}: MPV was not restarted within 40 s"));
        let new = player.pid().unwrap();
        println!(
            "{how:<16}: {event:?} after {:.1}s, pid {old} -> {new}, old process alive: {}",
            took.as_secs_f64(),
            alive(old)
        );
        assert_ne!(old, new);
        assert!(!alive(old), "{how}: the old MPV is still running");

        // The tuner would tune again here; the player must play at once.
        let mut probe = Probe::open(&dir);
        player.play_clip(&clip).unwrap();
        sleep(Duration::from_secs(3));
        let position = probe.get("time-pos").as_f64().unwrap_or(-1.0);
        println!("{how:<16}: after restart time-pos {position:.1}s");
        assert!(position > 1.0, "{how}: nothing plays after the restart");
    }
    let _ = player.stop();
    drop(player);
}

// ---------------------------------------------------------------- #21

struct Case {
    name: &'static str,
    ids: &'static str,
    env: &'static str,
    transcode: bool,
}

const CASES: [Case; 2] = [
    Case {
        name: "direct play (HEVC)",
        // hevc8_1080_24, hevc10_1080_24
        ids: "26423a8e457c12749df7520dbbef35ef,96d79cbd52c084a882ca7e4373954716",
        env: "SURFCULT_PI_DIRECT_IDS",
        transcode: false,
    },
    Case {
        name: "transcode (AV1, H.264 10-bit)",
        // av1_1080_24, h264_10bit_1080_24
        ids: "d06f99ee1ccf3f45e0639e48687a7aae,cd3781168dbe21b6d0e31fb328c3ab89",
        env: "SURFCULT_PI_TRANSCODE_IDS",
        transcode: true,
    },
];

/// The client, the server's address, the `Authorization` header value and the
/// key itself (only to check it never reaches a command line).
fn jellyfin() -> (Arc<JellyfinClient>, String, String, String) {
    let base = std::env::var("SURFCULT_JELLYFIN_URL").expect("SURFCULT_JELLYFIN_URL");
    let key_file = std::env::var("SURFCULT_JELLYFIN_API_KEY_FILE").expect("key file");
    let secret = fs::read_to_string(&key_file)
        .expect("key file readable")
        .trim()
        .to_string();
    let header = format!("MediaBrowser Token=\"{secret}\"");
    let client = JellyfinClient::new(JellyfinConfig {
        base_url: base.clone(),
        key: ApiKey::from_env().expect("the key from the environment"),
        user_id: JellyfinConfig::user_id_from_env(),
    });
    (Arc::new(client), base, header, secret)
}

/// #21: both URL kinds play in MPV on this network, the key travels only as
/// a header, a transcode starts at the requested position and stopping it
/// works.
#[test]
#[ignore = "needs the Pi, its TV and Jellyfin"]
fn jellyfin_urls_play_in_mpv() {
    let (client, base, header, secret) = jellyfin();
    const START_MS: u64 = 600_000;

    for case in CASES {
        let ids = std::env::var(case.env).unwrap_or_else(|_| case.ids.to_string());
        for id in ids.split(',') {
            let dir = run_dir("jellyfin");
            let clip = tune_in_clip(&std::env::temp_dir().join("surfcult-pi-clips"));
            let session: Arc<Mutex<Option<TranscodeSession>>> = Arc::new(Mutex::new(None));
            let kind: Arc<Mutex<&'static str>> = Arc::new(Mutex::new("?"));
            let resolver = {
                let (client, base, header) = (client.clone(), base.clone(), header.clone());
                let (session, kind) = (session.clone(), kind.clone());
                move |media: &MediaId, start_ms: u64| -> Result<Stream, PlayerError> {
                    let playback = client
                        .playback(media, start_ms)
                        .map_err(|e| PlayerError(e.to_string()))?;
                    let mut stream = Stream::new(playback.url(&base));
                    stream
                        .headers
                        .push(("Authorization".into(), header.clone()));
                    match &playback {
                        Playback::Direct { .. } => *kind.lock().unwrap() = "direct",
                        Playback::Transcode { session: s, .. } => {
                            *kind.lock().unwrap() = "transcode";
                            *session.lock().unwrap() = Some(s.clone());
                            stream.stream_offset_ms = start_ms;
                        }
                    }
                    Ok(stream)
                }
            };

            let mut player = MpvPlayer::start(config(&dir, vec![]), resolver).unwrap();
            let mut probe = Probe::open(&dir);
            player.play_clip(&clip).unwrap();
            let queued = Instant::now();
            player
                .queue_program(&MediaId::new(id), START_MS)
                .expect("the program could be queued");
            let kind = *kind.lock().unwrap();

            let ended = wait_for(&mut player, Duration::from_secs(30), |e| {
                *e == PlayerEvent::ClipEnded
            });
            let started = wait_for(&mut player, Duration::from_secs(90), |e| {
                matches!(e, PlayerEvent::ProgramStarted { .. })
            });
            let (event, _) =
                started.unwrap_or_else(|| panic!("{} {id}: the program never started", case.name));
            let PlayerEvent::ProgramStarted { position_ms } = event else {
                unreachable!()
            };
            let to_picture = queued.elapsed();
            let m = measure(&mut player, &mut probe, 10);
            let pid = player.pid().unwrap();
            let cmdline = fs::read(format!("/proc/{pid}/cmdline")).unwrap_or_default();
            let leaked = String::from_utf8_lossy(&cmdline).contains(&secret);
            let video = probe.get("video-format");
            let hwdec = probe.get("hwdec-current");
            let time_pos = probe.get("time-pos").as_f64().unwrap_or(-1.0);
            println!(
                "{} id {}: {kind}, ClipEnded after {:?}s, ProgramStarted at {} ms after {:.1}s from queue, time-pos {:.0}s, {video} via {hwdec}",
                case.name,
                &id[..8],
                ended.map(|(_, d)| d.as_secs_f64()),
                position_ms,
                to_picture.as_secs_f64(),
                time_pos,
            );
            print_sample("  10 s of playback", &m, "");

            assert_eq!(
                kind,
                if case.transcode {
                    "transcode"
                } else {
                    "direct"
                },
                "{id}: unexpected URL kind"
            );
            assert!(!leaked, "the key is in MPV's command line");
            // A transcode starts at the requested position, so MPV's clock
            // is 0-based and the media position is that plus the offset.
            let media_pos = if case.transcode {
                time_pos + START_MS as f64 / 1000.0
            } else {
                time_pos
            };
            assert!(
                (START_MS..START_MS + 5_000).contains(&position_ms),
                "{id}: ProgramStarted at {position_ms} ms, not near {START_MS}"
            );
            assert!(
                (media_pos - START_MS as f64 / 1000.0 - 10.0).abs() < 5.0,
                "{id}: media position {media_pos:.0}s ten seconds in, not near {} s",
                START_MS / 1000 + 10
            );

            player.stop().unwrap();
            if let Some(s) = session.lock().unwrap().take() {
                client
                    .stop_transcode(&s)
                    .expect("Jellyfin accepted the stop");
                println!("  stop_transcode: accepted for {}", s.play_session_id);
                // Leave the transcode a moment to show up in `ps` on Almaz.
                sleep(Duration::from_secs(3));
            }
            drop(player);
            let _ = fs::remove_dir_all(&dir);
        }
    }
}

// ---------------------------------------------------------------- #72

/// #72: the stand-by card and the CRT shader on the Pi: CPU, dropped frames,
/// whether a global `crt` keeps hardware decoding, and a screenshot of what
/// MPV drew. The look on the TV is the owner's check.
#[test]
#[ignore = "needs the Pi and its TV"]
fn standby_card_and_crt_cost_on_the_pi() {
    let out = PathBuf::from(
        std::env::var("SURFCULT_PI_OUT").unwrap_or_else(|_| "/tmp/surfcult-pi".into()),
    );
    fs::create_dir_all(&out).unwrap();
    let hold: u64 = std::env::var("SURFCULT_PI_CARD_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(20);

    // The card, with each filter set.
    for (label, filters, standby) in [
        ("card: no filters", vec![], vec![]),
        ("card: crt", vec![], vec![Filter::Crt]),
        (
            "card: crt + glitch (default)",
            vec![],
            vec![Filter::Crt, Filter::Glitch],
        ),
        (
            "card: global crt (no double)",
            vec![Filter::Crt],
            vec![Filter::Crt, Filter::Glitch],
        ),
    ] {
        let dir = run_dir("card");
        let mut cfg = config(&dir, filters);
        cfg.standby_filters = standby;
        cfg.glitch_every = Duration::from_secs(3);
        let mut player = MpvPlayer::start(cfg, nothing).unwrap();
        let mut probe = Probe::open(&dir);
        player.show_standby().unwrap();
        sleep(Duration::from_secs(4));
        let m = measure(&mut player, &mut probe, hold);
        let shot = out.join(format!(
            "{}.png",
            label.replace([' ', ':', '(', ')', '+'], "_")
        ));
        probe.command(json!([
            "screenshot-to-file",
            shot.to_str().unwrap(),
            "window"
        ]));
        print_sample(
            label,
            &m,
            &format!(
                "| shaders {} | display fps {}",
                probe.get("glsl-shaders"),
                probe.get("display-fps")
            ),
        );
        let _ = player.stop();
        drop(player);
        let _ = fs::remove_dir_all(&dir);
    }

    // A global CRT shader over hardware-decoded video.
    for file in [
        "Test_Jellyfin_1080p_HEVC_8bit_5M.mp4",
        "Test_Jellyfin_1080p_HEVC_10bit_5M.mp4",
        "Test_Jellyfin_1080p_AVC_5M.mp4",
    ] {
        let mut rows = Vec::new();
        for (label, filters) in [("without crt", vec![]), ("global crt", vec![Filter::Crt])] {
            let dir = run_dir("video");
            let mut player = MpvPlayer::start(config(&dir, filters), nothing).unwrap();
            let mut probe = Probe::open(&dir);
            player.play_clip(&sample(file)).unwrap();
            sleep(Duration::from_secs(4));
            let m = measure(&mut player, &mut probe, hold);
            let hw = probe.get("hwdec-current");
            let fmt = probe.get("video-format");
            print_sample(&format!("{file} {label}"), &m, &format!("| {fmt} via {hw}"));
            let shot = out.join(format!("{file}-{}.png", label.replace(' ', "_")));
            probe.command(json!([
                "screenshot-to-file",
                shot.to_str().unwrap(),
                "window"
            ]));
            rows.push((label, hw, m));
            let _ = player.stop();
            drop(player);
            let _ = fs::remove_dir_all(&dir);
        }
        println!(
            "  -> hardware decoding with global crt: {} (without: {})",
            rows[1].1, rows[0].1
        );
    }
    println!("screenshots are in {}", out.display());
}
