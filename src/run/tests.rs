//! The loop and the playback driver, driven by hand with a virtual clock: a
//! `FakePlayer` (wrapped so single programs, or all of them, can fail as a
//! Jellyfin error or outage would), a `FakeInput`, an in-memory `Store` whose library cache is the
//! media source, as in production, and clip pools scanned from a temporary
//! directory with a fake prober. Nothing sleeps and nothing needs MPV,
//! Jellyfin or ffprobe.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::clips::{ProbeError, Prober};
use crate::config::Tz;
use crate::input::FakeInput;
use crate::library::{AudioStream, Collection, MediaId, MediaKind, VideoStream};
use crate::overlay::{Banner, clock};
use crate::player::{
    Command, FakePlayer, Overlay, Picture, PlayerError, Slate, VideoArea, VirtualClock,
};
use crate::store::{LibraryEntry, Substitution};
use crate::timeline::{Event, EventKind};

const T: i64 = 1_800_000_000_000;
const MIN: i64 = 60_000;
const TUNE_IN_MS: i64 = 1_200;

/// A `FakePlayer` whose programs in `broken` fail to load, as when Jellyfin
/// cannot play them, and whose programs all fail as unreachable while
/// `down` is set. Nothing is recorded for a failed command.
struct TestPlayer {
    fake: FakePlayer,
    broken: HashSet<MediaId>,
    down: bool,
}

impl TestPlayer {
    fn check(&self, media: &MediaId) -> Result<(), PlayerError> {
        if self.down {
            Err(PlayerError::unreachable("Jellyfin is unreachable"))
        } else if self.broken.contains(media) {
            Err(PlayerError::new(format!("Jellyfin cannot play {media}")))
        } else {
            Ok(())
        }
    }
}

impl Player for TestPlayer {
    fn show_static(&mut self) -> Result<(), PlayerError> {
        self.fake.show_static()
    }
    fn show_standby(&mut self) -> Result<(), PlayerError> {
        self.fake.show_standby()
    }
    fn show_slate(&mut self, slate: Slate) -> Result<(), PlayerError> {
        self.fake.show_slate(slate)
    }
    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.fake.play_clip(path)
    }
    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        self.check(media)?;
        self.fake.queue_program(media, start_ms)
    }
    fn play_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        self.check(media)?;
        self.fake.play_program(media, start_ms)
    }
    fn queue_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.fake.queue_clip(path)
    }
    fn loop_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.fake.loop_clip(path)
    }
    fn queue_looping_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.fake.queue_looping_clip(path)
    }
    fn seek(&mut self, position_ms: u64) -> Result<(), PlayerError> {
        self.fake.seek(position_ms)
    }
    fn stop(&mut self) -> Result<(), PlayerError> {
        self.fake.stop()
    }
    fn show_overlay(&mut self, id: u8, overlay: &Overlay) -> Result<(), PlayerError> {
        self.fake.show_overlay(id, overlay)
    }
    fn clear_overlay(&mut self, id: u8) -> Result<(), PlayerError> {
        self.fake.clear_overlay(id)
    }
    fn show_banner(&mut self, banner: &Banner) -> Result<(), PlayerError> {
        self.fake.show_banner(banner)
    }
    fn clear_banner(&mut self) -> Result<(), PlayerError> {
        self.fake.clear_banner()
    }
    fn set_video_area(&mut self, area: VideoArea) -> Result<(), PlayerError> {
        self.fake.set_video_area(area)
    }
    fn set_crt(&mut self, crt: Crt) -> Result<(), PlayerError> {
        self.fake.set_crt(crt)
    }
    fn set_picture(&mut self, picture: Option<Picture>) -> Result<(), PlayerError> {
        self.fake.set_picture(picture)
    }
    fn show_overlay_rows(
        &mut self,
        id: u8,
        overlay: &Overlay,
        first_row: u32,
        rows: u32,
    ) -> Result<(), PlayerError> {
        self.fake.show_overlay_rows(id, overlay, first_row, rows)
    }
    fn scroll_overlay(&mut self, id: u8, first_row: u32, rows: u32) -> Result<(), PlayerError> {
        self.fake.scroll_overlay(id, first_row, rows)
    }
    fn poll_event(&mut self) -> Option<PlayerEvent> {
        self.fake.poll_event()
    }
}

/// Counts `stop_all` calls.
struct CountedStreams(Rc<Cell<usize>>);

impl Streams for CountedStreams {
    fn stop_all(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}

/// Clip lengths by file name.
struct Lengths(HashMap<&'static str, u64>);

impl Prober for Lengths {
    fn duration(&self, path: &Path) -> Result<Duration, ProbeError> {
        let name = path.file_name().unwrap().to_str().unwrap();
        self.0
            .get(name)
            .map(|ms| Duration::from_millis(*ms))
            .ok_or_else(|| ProbeError::Unparseable(format!("{name} is not a test clip")))
    }
}

const CLIPS: [(&str, &str, u64); 5] = [
    ("tune", "tune.mp4", TUNE_IN_MS as u64),
    ("ads", "ad1.mp4", 30_000),
    ("ads", "ad2.mp4", 30_000),
    ("ads", "pad.mp4", 10_000),
    ("filler", "filler.mp4", 60_000),
];

fn item(id: &str, title: &str, codec: &str) -> MediaItem {
    let mut item = MediaItem::new(id, title, MediaKind::Movie, 60 * MIN as u64);
    item.video.push(VideoStream {
        codec: codec.into(),
        width: 1920,
        height: 1080,
        bit_depth: Some(8),
    });
    item.audio.push(AudioStream {
        codec: "aac".into(),
        channels: 2,
        language: None,
    });
    item
}

struct Rig {
    tv: Tv<TestPlayer, FakeInput>,
    store: Store,
    clock: VirtualClock,
    stops: Rc<Cell<usize>>,
    trusted: Rc<Cell<bool>>,
    _dir: TempDir,
}

impl Rig {
    /// Channels 7, 3 and 5 (in that config order), all playing the
    /// collection `Cartoons`, except that `five_source` replaces channel 5's
    /// source. The library has `show` and `movie` (direct play) and `tc`
    /// (a transcode); `Cartoons` holds `show`. The clock is trusted, so
    /// startup extends the schedules written before it.
    fn new() -> Self {
        Self::with_five("jellyfin_collection = \"Cartoons\"")
    }

    fn with_five(five_source: &str) -> Self {
        Self::build(five_source, false, "")
    }

    /// As [`Rig::new`], with channel 3 a music channel with `picture`. The
    /// library also has `song`, on album `alb`, and `single`, on none.
    fn music(picture: &str) -> Self {
        Self::build(
            "jellyfin_collection = \"Cartoons\"",
            false,
            &format!("picture = \"{picture}\""),
        )
    }

    /// As [`Rig::new`], with the SCTV watermark on.
    fn watermarked() -> Self {
        Self::build("jellyfin_collection = \"Cartoons\"", true, "")
    }

    fn build(five_source: &str, watermark: bool, three_extra: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (pool, file, _) in CLIPS {
            let pool = dir.path().join(pool);
            std::fs::create_dir_all(&pool).unwrap();
            std::fs::write(pool.join(file), b"clip").unwrap();
        }
        let root = dir.path().display();
        let text = format!(
            r#"
            timezone = "UTC"
            tune_in_pool = "tune"
            [[channel]]
            number = 7
            name = "SEVEN"
            source = {{ jellyfin_collection = "Cartoons" }}
            mode = "sequential"
            filler = "filler"
            [[channel]]
            number = 3
            name = "THREE"
            source = {{ jellyfin_collection = "Cartoons" }}
            mode = "sequential"
            {three_extra}
            [[channel]]
            number = 5
            name = "FIVE"
            source = {{ {five_source} }}
            mode = "sequential"
            [[interstitial_pool]]
            name = "tune"
            path = "{root}/tune"
            [[interstitial_pool]]
            name = "ads"
            path = "{root}/ads"
            [[interstitial_pool]]
            name = "filler"
            path = "{root}/filler"
            "#
        );
        let config = Config::parse(&text).unwrap();
        let lengths = Lengths(CLIPS.iter().map(|(_, f, ms)| (*f, *ms)).collect());
        let clips = ClipLibrary::scan(&config.pools, lengths);
        assert_eq!(clips.issues().count(), 0);

        let mut store = Store::open_in_memory().unwrap();
        let entries: Vec<LibraryEntry> = [
            item("show", "The Show", "hevc"),
            item("movie", "The Movie", "h264"),
            item("tc", "Transcoded", "av1"),
            MediaItem {
                artists: vec!["The Band".into()],
                album: Some("The Record".into()),
                album_id: Some(MediaId::new("alb")),
                ..MediaItem::new("song", "The Song", MediaKind::Song, 4 * MIN as u64)
            },
            MediaItem::new("single", "The Single", MediaKind::Song, 3 * MIN as u64),
        ]
        .into_iter()
        .map(|item| LibraryEntry { item, path: None })
        .collect();
        let cartoons = Collection {
            name: "Cartoons".into(),
            members: vec![MediaId::new("show")],
        };
        store.replace_library(&entries, &[cartoons], 0).unwrap();

        let clock = VirtualClock::new(T);
        let player = TestPlayer {
            fake: FakePlayer::new(clock.clone()),
            broken: HashSet::new(),
            down: false,
        };
        let stops = Rc::new(Cell::new(0));
        let trusted = Rc::new(Cell::new(true));
        let trust = trusted.clone();
        let tv = Tv::new(
            config,
            player,
            FakeInput::new(),
            clips,
            Box::new(CountedStreams(stops.clone())),
            Box::new(move || trust.get()),
            Settings {
                watermark,
                // The pass has its own tests; in a debug build it is slow.
                crt: Crt::Off,
                ..Settings::default()
            },
        );
        Self {
            tv,
            store,
            clock,
            stops,
            trusted,
            _dir: dir,
        }
    }

    /// The canonical path of a test clip, which is its media id.
    fn clip(&self, file: &str) -> String {
        let pool = CLIPS.iter().find(|(_, f, _)| *f == file).unwrap().0;
        let path = self._dir.path().join(pool).join(file);
        path.canonicalize().unwrap().to_str().unwrap().to_string()
    }

    fn write(&mut self, channel: u32, events: &[Event]) {
        self.store.write_events(channel, events).unwrap();
    }

    fn start(&mut self) {
        self.clock.set(T);
        self.tv.start(T, &mut self.store);
    }

    fn step(&mut self, at_ms: i64) {
        self.clock.set(at_ms);
        self.tv.step(at_ms, &mut self.store);
    }

    fn press(&mut self, key: Key, at_ms: i64) {
        self.tv.input_mut().push(key);
        self.step(at_ms);
    }

    fn fake(&mut self) -> &mut FakePlayer {
        &mut self.tv.player_mut().fake
    }

    fn commands(&self) -> Vec<Command> {
        self.tv.player().fake.commands()
    }

    /// The commands since the last call.
    fn take(&mut self) -> Vec<Command> {
        let commands = self.commands();
        self.fake().clear();
        commands
    }

    /// Runs the tune-in sequence that began with a key press (or the start)
    /// at `pressed_ms`: static for 0.5 s, the 1.2 s clip, then the program
    /// reports its start at the position it was queued at. Returns the
    /// commands from the clip on, through the banner.
    fn go_live(&mut self, pressed_ms: i64) -> Vec<Command> {
        let clip_at = pressed_ms + 500;
        self.step(clip_at);
        let queued = self.commands().into_iter().find_map(|c| match c {
            Command::QueueProgram { start_ms, .. } => Some(start_ms),
            _ => None,
        });
        let ends = clip_at + TUNE_IN_MS;
        self.fake().clip_ended();
        if let Some(start_ms) = queued {
            self.fake().program_started(start_ms);
        }
        self.step(ends);
        self.take()
    }

    fn history(&self, channel: u32) -> Vec<(String, i64, String)> {
        let mut rows: Vec<_> = self
            .store
            .playback_history(channel, 100)
            .unwrap()
            .into_iter()
            .map(|r| (r.media_id.to_string(), r.started_at_ms, r.outcome))
            .collect();
        rows.reverse();
        rows
    }
}

fn event(start_ms: i64, end_ms: i64, media: &str, offset_ms: u64, kind: EventKind) -> Event {
    Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media),
        media_offset_ms: offset_ms,
        media_duration_ms: 60 * MIN as u64,
        kind,
    }
}

fn program(start_ms: i64, end_ms: i64, media: &str, offset_ms: u64) -> Event {
    event(start_ms, end_ms, media, offset_ms, EventKind::Program)
}

fn queue(media: &str, start_ms: i64) -> Command {
    Command::QueueProgram {
        media_id: MediaId::new(media),
        start_ms: u64::try_from(start_ms).unwrap(),
    }
}

fn play(media: &str, start_ms: i64) -> Command {
    Command::PlayProgram {
        media_id: MediaId::new(media),
        start_ms: u64::try_from(start_ms).unwrap(),
    }
}

/// A program on every channel from 10 minutes before `T` for an hour.
fn hour_everywhere(rig: &mut Rig) {
    for n in [3, 5, 7] {
        rig.write(n, &[program(T - 10 * MIN, T + 50 * MIN, "show", 0)]);
    }
}

fn banners(commands: &[Command]) -> Vec<Banner> {
    commands
        .iter()
        .filter_map(|c| match c {
            Command::ShowBanner(b) => Some(b.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn up_and_down_wrap_around_the_channel_numbers_in_order() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    // The lowest number first, whatever the config order.
    assert_eq!(rig.tv.channel(), Some(3));
    let mut seen = Vec::new();
    for (i, key) in [
        Key::Next,
        Key::Next,
        Key::Next,
        Key::Previous,
        Key::Previous,
    ]
    .into_iter()
    .enumerate()
    {
        rig.press(key, T + 10_000 * (i as i64 + 1));
        seen.push(rig.tv.channel().unwrap());
    }
    assert_eq!(seen, [5, 7, 3, 7, 5]);
    // Every change starts with static.
    let statics = rig
        .commands()
        .iter()
        .filter(|c| **c == Command::ShowStatic)
        .count();
    assert_eq!(statics, 6);
}

#[test]
fn a_tune_shows_static_a_tune_in_clip_the_program_at_its_live_position_and_the_banner() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    assert_eq!(rig.take(), [Command::ShowStandby, Command::ShowStatic]);
    let live = rig.go_live(T);
    let tune_in = Path::new(&rig.clip("tune.mp4")).to_path_buf();
    // Queued for the moment the clip ends: 10 minutes into the program, plus
    // the static and the clip.
    let at_clip_end = 10 * MIN + 500 + TUNE_IN_MS;
    assert_eq!(
        live,
        [
            Command::PlayClip(tune_in),
            queue("show", at_clip_end),
            Command::ShowBanner(Banner {
                number: 3,
                name: "THREE".into(),
                title: "The Show".into(),
                times: format!(
                    "{}–{}",
                    clock(T - 10 * MIN, Tz::UTC),
                    clock(T + 50 * MIN, Tz::UTC)
                ),
                clock: clock(T + 500 + TUNE_IN_MS, Tz::UTC),
                next: format!("NEXT {} The Show", clock(T + 50 * MIN, Tz::UTC)),
            }),
        ]
    );
    assert_eq!(
        rig.history(3),
        [("show".into(), T + 500 + TUNE_IN_MS, "played".into())]
    );
}

#[test]
fn the_banner_clears_after_its_timeout_and_ok_shows_it_again() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    let shown = T + 500 + TUNE_IN_MS;
    rig.step(shown + 4_999);
    assert_eq!(rig.take(), []);
    rig.step(shown + 5_000);
    assert_eq!(rig.take(), [Command::ClearBanner]);
    rig.press(Key::Banner, shown + 20_000);
    let again = rig.take();
    assert_eq!(banners(&again).len(), 1);
    rig.step(shown + 25_000);
    assert_eq!(rig.take(), [Command::ClearBanner]);
}

#[test]
fn switching_away_and_back_resumes_at_the_live_position() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Next, T + MIN);
    rig.go_live(T + MIN);
    rig.press(Key::Previous, T + 5 * MIN);
    let back = rig.go_live(T + 5 * MIN);
    // Fifteen minutes in (plus the sequence), not where the viewer left
    // and not from the start.
    assert!(
        back.contains(&queue("show", 15 * MIN + 500 + TUNE_IN_MS)),
        "{back:?}"
    );
    // Each channel change stopped the streams of the program before it.
    assert!(rig.stops.get() >= 3);
}

/// `show` 11 minutes from `T - 10 min`, two 30 s ads, then `show` again
/// from 11 minutes in.
fn cut_by_a_break(rig: &mut Rig) -> Vec<Event> {
    let (ad1, ad2) = (rig.clip("ad1.mp4"), rig.clip("ad2.mp4"));
    let events = vec![
        program(T - 10 * MIN, T + MIN, "show", 0),
        event(T + MIN, T + MIN + 30_000, &ad1, 0, EventKind::Break),
        event(T + MIN + 30_000, T + 2 * MIN, &ad2, 0, EventKind::Break),
        program(T + 2 * MIN, T + 10 * MIN, "show", 11 * MIN as u64),
    ];
    rig.write(3, &events);
    events
}

#[test]
fn a_program_cut_by_breaks_plays_its_segments_and_break_clips_in_order() {
    let mut rig = Rig::new();
    cut_by_a_break(&mut rig);
    rig.start();
    rig.go_live(T);
    let (ad1, ad2) = (rig.clip("ad1.mp4"), rig.clip("ad2.mp4"));
    rig.step(T + MIN - 1);
    assert_eq!(rig.take(), [Command::ClearBanner]);
    // The program is cut at the break; the second ad is queued behind the
    // first, which ends exactly at its event's end.
    rig.step(T + MIN);
    assert_eq!(
        rig.take(),
        [
            Command::PlayClip(ad1.clone().into()),
            Command::QueueClip(ad2.clone().into())
        ]
    );
    // MPV moves on by itself: nothing at the boundary except queueing the
    // rest of the program behind the second ad, at the next segment's offset.
    rig.step(T + MIN + 30_000);
    assert_eq!(rig.take(), [queue("show", 11 * MIN)]);
    rig.step(T + 2 * MIN);
    assert_eq!(rig.take(), []);
    // It started 2 s late, so it seeks once to the live position.
    rig.fake().program_started(11 * MIN as u64);
    rig.step(T + 2 * MIN + 2_000);
    assert_eq!(rig.take(), [Command::Seek(11 * MIN as u64 + 2_000)]);
    let played: Vec<_> = rig
        .history(3)
        .into_iter()
        .map(|(m, at, _)| (m, at))
        .collect();
    assert_eq!(
        played,
        [
            ("show".into(), T + 500 + TUNE_IN_MS),
            (ad1, T + MIN),
            (ad2, T + MIN + 30_000),
            ("show".into(), T + 2 * MIN),
        ]
    );
}

#[test]
fn a_pad_longer_than_its_clip_loops_the_clip_until_the_next_event() {
    let mut rig = Rig::new();
    let (ad1, ad2) = (rig.clip("ad1.mp4"), rig.clip("ad2.mp4"));
    // Both clips are 30 s. The break's pad repeats ad2 for 90 s, and a
    // second pad, on its own, repeats ad1 for 2 minutes.
    rig.write(
        3,
        &[
            program(T - 10 * MIN, T + MIN, "show", 0),
            event(T + MIN, T + MIN + 30_000, &ad1, 0, EventKind::Break),
            event(T + MIN + 30_000, T + 3 * MIN, &ad2, 0, EventKind::Ident),
            program(T + 3 * MIN, T + 10 * MIN, "show", 11 * MIN as u64),
            event(T + 10 * MIN, T + 12 * MIN, &ad1, 0, EventKind::Ident),
            program(T + 12 * MIN, T + 20 * MIN, "show", 18 * MIN as u64),
        ],
    );
    rig.start();
    rig.go_live(T);
    rig.step(T + MIN - 1);
    rig.take();
    // The pad is queued to loop, so MPV never runs out of it.
    rig.step(T + MIN);
    assert_eq!(
        rig.take(),
        [
            Command::PlayClip(ad1.clone().into()),
            Command::QueueLoopingClip(ad2.clone().into())
        ]
    );
    // Nothing is queued behind a looping clip: the program replaces it.
    rig.step(T + MIN + 30_000);
    assert_eq!(rig.take(), []);
    rig.step(T + 3 * MIN);
    assert_eq!(rig.take(), [play("show", 11 * MIN)]);
    rig.step(T + 10 * MIN);
    assert_eq!(rig.take(), [Command::LoopClip(ad1.into())]);
    rig.step(T + 12 * MIN);
    assert_eq!(rig.take(), [play("show", 18 * MIN)]);
}

#[test]
fn a_late_loop_replaces_a_stale_queue_with_what_is_on_now() {
    let mut rig = Rig::new();
    cut_by_a_break(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.step(T + MIN);
    rig.take();
    // The loop stalled (a watchdog restart blocks for seconds) past the
    // second ad and the queue behind it: the program is played where it is now.
    rig.step(T + 2 * MIN + 3_000);
    let commands = rig.take();
    assert_eq!(commands, [play("show", 11 * MIN + 3_000)]);
}

#[test]
fn a_gap_shows_the_card_and_the_next_program_starts_on_time() {
    let mut rig = Rig::new();
    rig.write(
        3,
        &[
            program(T - 10 * MIN, T + MIN, "show", 0),
            program(T + 5 * MIN, T + 50 * MIN, "movie", 0),
            program(T + 50 * MIN, T + 55 * MIN, "show", 0),
            program(T + 60 * MIN, T + 90 * MIN, "tc", 0),
        ],
    );
    rig.start();
    rig.go_live(T);
    rig.step(T + 30_000);
    rig.take();
    rig.step(T + MIN);
    assert_eq!(rig.take(), [Command::ShowStandby]);
    // A direct-play program is started its start-up time (1 s) early, at
    // its first frame, so it is on screen when its event starts.
    rig.step(T + 5 * MIN - 1_001);
    assert_eq!(rig.take(), []);
    rig.step(T + 5 * MIN - 1_000);
    assert_eq!(rig.take(), [play("movie", 0)]);
    rig.step(T + 5 * MIN);
    assert_eq!(rig.take(), []);
    // Program to program replaces at the boundary.
    rig.step(T + 50 * MIN);
    assert_eq!(rig.take(), [play("show", 0)]);
    rig.step(T + 55 * MIN);
    assert_eq!(rig.take(), [Command::ShowStandby]);
    // A transcode gets 7 s.
    rig.step(T + 60 * MIN - 7_000);
    assert_eq!(rig.take(), [play("tc", 0)]);
}

#[test]
fn a_tune_into_a_gap_shows_the_card_with_off_the_air_and_follows_on() {
    let mut rig = Rig::new();
    rig.write(3, &[program(T + 5 * MIN, T + 50 * MIN, "movie", 0)]);
    rig.start();
    rig.take();
    rig.step(T + 500);
    let commands = rig.take();
    assert_eq!(commands[0], Command::ShowStandby);
    assert_eq!(
        banners(&commands),
        [Banner {
            number: 3,
            name: "THREE".into(),
            title: "OFF THE AIR".into(),
            times: format!("BACK AT {}", clock(T + 5 * MIN, Tz::UTC)),
            clock: clock(T + 500, Tz::UTC),
            next: String::new(),
        }]
    );
    rig.step(T + 10_000);
    assert_eq!(rig.take(), [Command::ClearBanner]);
    rig.step(T + 5 * MIN - 1_000);
    assert_eq!(rig.take(), [play("movie", 0)]);
}

#[test]
fn an_unplayable_program_without_filler_shows_the_card_and_the_loop_goes_on() {
    let mut rig = Rig::new();
    rig.write(
        3,
        &[
            program(T - 10 * MIN, T + MIN, "show", 0),
            program(T + MIN, T + 2 * MIN, "tc", 0),
            program(T + 2 * MIN, T + 3 * MIN, "movie", 0),
        ],
    );
    hour_on(&mut rig, 5);
    rig.tv.player_mut().broken.insert(MediaId::new("tc"));
    rig.start();
    rig.go_live(T);
    rig.step(T + 30_000);
    rig.take();
    rig.step(T + MIN);
    assert_eq!(rig.take(), [Command::ShowStandby]);
    assert_eq!(rig.history(3)[1], ("tc".into(), T + MIN, "failed".into()));
    let substitution = Substitution {
        channel: 3,
        media_id: MediaId::new("tc"),
        scheduled_ms: T + MIN,
        replacement: None,
    };
    assert_eq!(rig.store.substitutions(3).unwrap(), [substitution]);
    // The next event plays as usual, and keys still work.
    rig.step(T + 2 * MIN);
    assert_eq!(rig.take(), [play("movie", 0)]);
    rig.press(Key::Next, T + 2 * MIN + 10_000);
    assert_eq!(rig.take(), [Command::ShowStatic]);
}

fn hour_on(rig: &mut Rig, channel: u32) {
    rig.write(channel, &[program(T - 10 * MIN, T + 50 * MIN, "show", 0)]);
}

#[test]
fn a_tune_the_player_refuses_shows_the_card_and_tries_the_program_without_a_clip() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.tv.player_mut().broken.insert(MediaId::new("show"));
    rig.start();
    rig.take();
    rig.step(T + 500);
    let commands = rig.take();
    // The tuner played the clip, then could not queue the program: the loop
    // shows the card, the driver tries the program once more, fails again
    // and leaves the card up until the event ends.
    assert_eq!(
        commands[..3],
        [
            Command::PlayClip(rig.clip("tune.mp4").into()),
            Command::ShowStandby,
            Command::ShowStandby,
        ]
    );
    assert_eq!(banners(&commands).len(), 1);
    rig.press(Key::Next, T + 10_000);
    assert_eq!(rig.tv.channel(), Some(5));
}

#[test]
fn a_restarted_player_tunes_again_and_shows_the_banner_again() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.fake().restarted();
    rig.step(T + MIN);
    assert_eq!(rig.take(), [Command::ShowStatic]);
    let live = rig.go_live(T + MIN);
    assert_eq!(banners(&live).len(), 1);
}

#[test]
fn startup_skips_a_channel_whose_source_cannot_be_resolved() {
    let mut rig = Rig::with_five("jellyfin_series = \"Not In The Library\"");
    rig.trusted.set(true);
    rig.start();
    assert!(rig.store.schedule_end_ms(3).unwrap().is_some());
    assert!(rig.store.schedule_end_ms(7).unwrap().is_some());
    assert_eq!(rig.store.schedule_end_ms(5).unwrap(), None);
    // Channel 3 plays; channel 5 shows the card; channel 7 plays.
    let live = rig.go_live(T);
    assert!(
        live.iter()
            .any(|c| matches!(c, Command::QueueProgram { .. }))
    );
    rig.press(Key::Next, T + MIN);
    rig.step(T + MIN + 500);
    let five = rig.take();
    assert_eq!(five[1], Command::ShowStandby, "{five:?}");
    assert_eq!(banners(&five)[0].title, "NO SCHEDULE");
    rig.press(Key::Next, T + 2 * MIN);
    let seven = rig.go_live(T + 2 * MIN);
    assert!(
        seven
            .iter()
            .any(|c| matches!(c, Command::QueueProgram { .. }))
    );
}

#[test]
fn an_untrusted_clock_shows_the_slate_and_writes_nothing_until_it_is_trusted() {
    let mut rig = Rig::new();
    // Events stored before a reboot are not played at an untrusted time.
    hour_everywhere(&mut rig);
    rig.trusted.set(false);
    rig.start();
    assert_eq!(rig.take(), [Command::ShowSlate(Slate::ClockNotSet)]);
    let stored = rig.store.schedule_end_ms(3).unwrap();
    // Channel keys do nothing; the minute's check finds it still untrusted.
    rig.press(Key::Next, T + 10_000);
    rig.step(T + MIN);
    assert_eq!(rig.take(), []);
    assert_eq!(rig.tv.channel(), None);
    assert_eq!(rig.store.schedule_end_ms(3).unwrap(), stored);
    // A restarted player gets the slate back.
    rig.fake().restarted();
    rig.step(T + MIN + 10_000);
    assert_eq!(rig.take(), [Command::ShowSlate(Slate::ClockNotSet)]);
    // The clock synchronizes: the next check writes the schedules and tunes
    // the lowest channel.
    rig.trusted.set(true);
    rig.step(T + 2 * MIN);
    assert!(rig.store.schedule_end_ms(3).unwrap() > stored);
    assert_eq!(rig.take(), [Command::ShowStatic]);
    assert_eq!(rig.tv.channel(), Some(3));
    let live = rig.go_live(T + 2 * MIN);
    assert!(live.contains(&queue("show", 12 * MIN + 500 + TUNE_IN_MS)));
}

/// Tunes channel 7, which has the filler pool, with `events` on it, and
/// goes live on its first program.
fn on_seven(rig: &mut Rig, events: &[Event]) {
    rig.write(3, &[program(T - 10 * MIN, T + 50 * MIN, "show", 0)]);
    rig.write(7, events);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Previous, T + 10_000);
    rig.go_live(T + 10_000);
    assert_eq!(rig.tv.channel(), Some(7));
    rig.step(T + 30_000);
    rig.take();
}

#[test]
fn an_unplayable_program_is_replaced_by_filler_and_the_next_item_plays() {
    let mut rig = Rig::new();
    on_seven(
        &mut rig,
        &[
            program(T - 10 * MIN, T + MIN, "show", 0),
            program(T + MIN, T + 3 * MIN + 30_000, "tc", 0),
            program(T + 3 * MIN + 30_000, T + 5 * MIN, "movie", 0),
        ],
    );
    rig.tv.player_mut().broken.insert(MediaId::new("tc"));
    let filler = rig.clip("filler.mp4");
    // Jellyfin refuses `tc`: its time is filled with the 60 s filler clip,
    // whole, then cut at the event's end.
    rig.step(T + MIN);
    assert_eq!(
        rig.take(),
        [
            Command::PlayClip(filler.clone().into()),
            Command::QueueClip(filler.clone().into())
        ]
    );
    rig.step(T + 2 * MIN);
    assert_eq!(rig.take(), [Command::QueueClip(filler.clone().into())]);
    rig.step(T + 3 * MIN);
    assert_eq!(rig.take(), []);
    rig.step(T + 3 * MIN + 30_000);
    assert_eq!(rig.take(), [play("movie", 0)]);
    let history = rig.history(7);
    assert_eq!(history[1], ("tc".into(), T + MIN, "failed".into()));
    assert_eq!(history[2], (filler.clone(), T + MIN, "played".into()));
    assert_eq!(
        rig.store.substitutions(7).unwrap(),
        [Substitution {
            channel: 7,
            media_id: MediaId::new("tc"),
            scheduled_ms: T + MIN,
            replacement: Some(MediaId::new(filler)),
        }]
    );
}

#[test]
fn a_program_that_fails_twice_on_screen_is_replaced_by_filler() {
    let mut rig = Rig::new();
    on_seven(
        &mut rig,
        &[
            program(T - 10 * MIN, T + MIN, "show", 0),
            program(T + MIN, T + 3 * MIN, "movie", 0),
        ],
    );
    rig.step(T + MIN);
    assert_eq!(rig.take(), [play("movie", 0)]);
    // The stream breaks off: it is tried once more at the live position.
    rig.fake().failed();
    rig.step(T + MIN + 5_000);
    assert_eq!(rig.take(), [play("movie", 5_000)]);
    // It fails again, so filler takes the rest of its time.
    rig.fake().failed();
    rig.step(T + MIN + 6_000);
    let filler = rig.clip("filler.mp4");
    assert_eq!(rig.take(), [Command::PlayClip(filler.into())]);
    assert_eq!(rig.store.substitutions(7).unwrap().len(), 1);
    // Reading the schedule again does not try it again.
    rig.tv.synced(
        T + MIN + 7_000,
        &mut rig.store,
        Ok(SyncReport {
            items: 3,
            collections: 1,
            skipped: 0,
            changes: LibraryChanges::default(),
            written: true,
        }),
    );
    rig.step(T + MIN + 7_020);
    assert_eq!(rig.take(), []);
}

#[test]
fn with_jellyfin_unreachable_the_slate_shows_clips_play_and_programs_come_back() {
    let mut rig = Rig::new();
    let ad1 = rig.clip("ad1.mp4");
    on_seven(
        &mut rig,
        &[
            program(T - 10 * MIN, T + MIN, "show", 0),
            program(T + MIN, T + 4 * MIN, "movie", 0),
            event(T + 4 * MIN, T + 4 * MIN + 30_000, &ad1, 0, EventKind::Break),
            program(T + 4 * MIN + 30_000, T + 10 * MIN, "movie", 3 * MIN as u64),
        ],
    );
    rig.tv.player_mut().down = true;
    rig.step(T + MIN);
    assert_eq!(
        rig.take(),
        [Command::ShowSlate(Slate::TechnicalDifficulties)]
    );
    // A minute later it is tried again, quietly; still down.
    rig.step(T + 2 * MIN);
    assert_eq!(rig.take(), []);
    // The break's clip is local and plays.
    rig.step(T + 4 * MIN);
    assert_eq!(rig.take(), [Command::PlayClip(ad1.clone().into())]);
    rig.step(T + 4 * MIN + 30_000);
    assert_eq!(
        rig.take(),
        [Command::ShowSlate(Slate::TechnicalDifficulties)]
    );
    // Jellyfin is back: the next retry plays the program at its live position.
    rig.tv.player_mut().down = false;
    rig.step(T + 5 * MIN + 30_000);
    assert_eq!(rig.take(), [play("movie", 4 * MIN)]);
    let outcomes: Vec<_> = rig.history(7).into_iter().skip(1).collect();
    assert_eq!(
        outcomes,
        [
            ("movie".into(), T + MIN, "failed".into()),
            (ad1, T + 4 * MIN, "played".into()),
            ("movie".into(), T + 4 * MIN + 30_000, "failed".into()),
            ("movie".into(), T + 5 * MIN + 30_000, "played".into()),
        ]
    );
    assert_eq!(rig.store.substitutions(7).unwrap(), []);
}

#[test]
fn a_sync_that_changes_a_scheduled_runtime_regenerates_after_the_freeze() {
    let mut rig = Rig::new();
    rig.trusted.set(true);
    rig.start();
    let before = rig.store.events_between(3, T, i64::MAX).unwrap();
    assert_eq!((before[0].start_ms, before[0].end_ms), (T, T + 60 * MIN));

    // Tdarr replaced `show` with a 30-minute file under the same id.
    let mut shorter = item("show", "The Show", "hevc");
    shorter.runtime_ms = 30 * MIN as u64;
    let entries: Vec<LibraryEntry> = [
        shorter,
        item("movie", "The Movie", "h264"),
        item("tc", "Transcoded", "av1"),
    ]
    .into_iter()
    .map(|item| LibraryEntry { item, path: None })
    .collect();
    let cartoons = Collection {
        name: "Cartoons".into(),
        members: vec![MediaId::new("show")],
    };
    let changes = rig.store.replace_library(&entries, &[cartoons], 1).unwrap();
    assert_eq!(changes.resized, [MediaId::new("show")]);
    let report = SyncReport {
        items: 3,
        collections: 1,
        skipped: 0,
        changes,
        written: true,
    };
    rig.tv.synced(T + MIN, &mut rig.store, Ok(report));

    let after = rig.store.events_between(3, T, i64::MAX).unwrap();
    // The program on screen is frozen; the next one has the new runtime.
    assert_eq!(after[0], before[0]);
    assert_eq!(
        (after[1].start_ms, after[1].end_ms),
        (T + 60 * MIN, T + 90 * MIN)
    );
    assert_eq!(after[1].media_duration_ms, 30 * MIN as u64);
}

#[test]
fn one_digit_tunes_when_no_second_digit_comes() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    assert_eq!(rig.tv.channel(), Some(3));
    rig.press(Key::Digit(5), T + 10_000);
    rig.step(T + 11_999);
    assert_eq!(rig.tv.channel(), Some(3));
    rig.step(T + 12_000);
    assert_eq!(rig.tv.channel(), Some(5));
}

#[test]
fn two_digits_tune_at_once() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.press(Key::Digit(0), T + 10_000);
    assert_eq!(rig.tv.channel(), Some(3));
    rig.press(Key::Digit(7), T + 11_000);
    assert_eq!(rig.tv.channel(), Some(7));
    // The entry is over: a later wait tunes nothing else.
    rig.step(T + 20_000);
    assert_eq!(rig.tv.channel(), Some(7));
}

#[test]
fn each_digit_restarts_the_wait() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    // 1 then 5 is channel 15, which does not exist: nothing changes.
    rig.press(Key::Digit(1), T + 10_000);
    rig.press(Key::Digit(5), T + 11_500);
    rig.step(T + 20_000);
    assert_eq!(rig.tv.channel(), Some(3));
    // The wait starts again at each digit.
    rig.press(Key::Digit(0), T + 30_000);
    rig.step(T + 31_999);
    assert_eq!(rig.tv.channel(), Some(3));
    rig.press(Key::Digit(5), T + 32_500);
    assert_eq!(rig.tv.channel(), Some(5));
}

#[test]
fn an_unknown_channel_number_is_ignored() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.step(T + 9_000);
    rig.take();
    rig.press(Key::Digit(9), T + 10_000);
    rig.step(T + 13_000);
    assert_eq!(rig.tv.channel(), Some(3));
    assert_eq!(rig.take(), []);
}

#[test]
fn a_channel_key_ends_number_entry() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.press(Key::Digit(7), T + 10_000);
    rig.press(Key::Next, T + 10_500);
    assert_eq!(rig.tv.channel(), Some(5));
    rig.step(T + 13_000);
    assert_eq!(rig.tv.channel(), Some(5));
}

#[test]
fn keys_without_a_use_yet_do_nothing() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.step(T + 9_000);
    rig.take();
    for key in [Key::Back, Key::Mute, Key::VolumeUp, Key::VolumeDown] {
        rig.press(key, T + 10_000);
    }
    assert_eq!(rig.tv.channel(), Some(3));
    assert_eq!(rig.take(), []);
}

/// The watermark layer's commands, with the width of each bitmap shown (the
/// beside form is wider than the under form) or `None` for a clear.
fn watermark(commands: &[Command]) -> Vec<Option<u32>> {
    commands
        .iter()
        .filter_map(|c| match c {
            Command::ShowOverlay { id, width, .. } if *id == WATERMARK_LAYER => Some(Some(*width)),
            Command::ClearOverlay(id) if *id == WATERMARK_LAYER => Some(None),
            _ => None,
        })
        .collect()
}

#[test]
fn the_watermark_shows_on_landing_holds_three_seconds_and_fades_out() {
    let mut rig = Rig::watermarked();
    hour_everywhere(&mut rig);
    rig.start();
    let live = rig.go_live(T);
    let shown = T + 500 + TUNE_IN_MS;
    let first = watermark(&live);
    assert_eq!(first.len(), 1, "{live:?}");
    let beside = first[0].unwrap();
    rig.step(shown + 2_999);
    assert_eq!(watermark(&rig.take()), []);
    // The fade redraws in eighths: the first step down is an eighth of the
    // way into the fade.
    rig.step(shown + 3_125);
    assert_eq!(watermark(&rig.take()), [Some(beside)]);
    rig.step(shown + 3_999);
    assert_eq!(watermark(&rig.take()), [Some(beside)]);
    rig.step(shown + 4_000);
    assert_eq!(watermark(&rig.take()), [None]);
    rig.step(shown + 10_000);
    assert_eq!(watermark(&rig.take()), []);
}

#[test]
fn the_watermark_stays_through_a_break_in_its_under_form() {
    let mut rig = Rig::watermarked();
    cut_by_a_break(&mut rig);
    rig.start();
    let live = rig.go_live(T);
    let beside = watermark(&live)[0].unwrap();
    // The break starts a minute in.
    rig.step(T + MIN);
    let under = watermark(&rig.take());
    assert_eq!(under.len(), 1);
    let under = under[0].unwrap();
    assert!(under < beside, "the under form is narrower");
    rig.fake().clip_ended();
    rig.step(T + MIN + 30_000);
    rig.step(T + MIN + 50_000);
    assert_eq!(
        watermark(&rig.take()),
        [],
        "no change while the break lasts"
    );
    // The program comes back at two minutes: the under form fades out.
    rig.step(T + 2 * MIN);
    rig.step(T + 2 * MIN + 500);
    assert_eq!(watermark(&rig.take()), [Some(under)]);
    rig.step(T + 2 * MIN + 1_000);
    assert_eq!(watermark(&rig.take()), [None]);
}

#[test]
fn a_channel_change_takes_the_watermark_away() {
    let mut rig = Rig::watermarked();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.press(Key::Next, T + 1_000);
    assert_eq!(watermark(&rig.take()), [None]);
}

#[test]
fn the_watermark_is_off_when_the_settings_say_so() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    let live = rig.go_live(T);
    assert_eq!(watermark(&live), []);
}

/// The guide's commands: its layers shown or cleared, and the video area.
fn guide_commands(commands: &[Command]) -> Vec<Command> {
    commands
        .iter()
        .filter(|c| match c {
            Command::ShowOverlay { id, .. }
            | Command::ShowOverlayRows { id, .. }
            | Command::ScrollOverlay { id, .. }
            | Command::ClearOverlay(id) => {
                *id == GUIDE_LAYER || *id == GUIDE_CLOCK_LAYER || *id == GUIDE_ROWS_LAYER
            }
            Command::SetVideoArea(_) => true,
            _ => false,
        })
        .cloned()
        .collect()
}

fn shown(layer: u8, commands: &[Command]) -> usize {
    commands
        .iter()
        .filter(|c| matches!(c, Command::ShowOverlay { id, .. } if *id == layer))
        .count()
}

#[test]
fn the_guide_key_opens_the_guide_with_the_video_in_the_corner() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.step(T + 9_000);
    rig.take();
    rig.press(Key::Guide, T + 10_000);
    let commands = rig.take();
    let guide = guide_commands(&commands);
    assert_eq!(guide[0], Command::SetVideoArea(VideoArea::TopRight));
    assert_eq!(
        guide[1],
        Command::ShowOverlay {
            id: GUIDE_LAYER,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080
        }
    );
    assert_eq!(shown(GUIDE_CLOCK_LAYER, &commands), 1);
    // The clock ticks on its own layer; the body is not drawn again.
    rig.step(T + 10_500);
    assert_eq!(guide_commands(&rig.take()), []);
    rig.step(T + 11_000);
    let tick = rig.take();
    assert_eq!(shown(GUIDE_CLOCK_LAYER, &tick), 1);
    assert_eq!(shown(GUIDE_LAYER, &tick), 0);
}

#[test]
fn up_and_down_move_the_focus_and_ok_tunes_the_focused_channel() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Guide, T + 10_000);
    rig.take();
    // The focus starts on channel 3, the top row: Up does nothing.
    rig.press(Key::Next, T + 10_200);
    assert_eq!(shown(GUIDE_LAYER, &rig.take()), 0);
    rig.press(Key::Previous, T + 10_400);
    assert_eq!(shown(GUIDE_LAYER, &rig.take()), 1);
    rig.press(Key::Previous, T + 10_600);
    rig.take();
    assert_eq!(rig.tv.channel(), Some(3), "moving the focus does not tune");
    rig.press(Key::Banner, T + 11_000);
    let commands = rig.take();
    assert_eq!(
        guide_commands(&commands),
        [
            Command::ClearOverlay(GUIDE_LAYER),
            Command::ClearOverlay(GUIDE_CLOCK_LAYER),
            Command::ClearOverlay(GUIDE_ROWS_LAYER),
            Command::SetVideoArea(VideoArea::Full),
        ]
    );
    assert!(commands.contains(&Command::ShowStatic), "{commands:?}");
    assert_eq!(rig.tv.channel(), Some(7));
}

#[test]
fn back_closes_the_guide_without_tuning() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Guide, T + 10_000);
    rig.press(Key::Previous, T + 10_200);
    rig.take();
    rig.press(Key::Back, T + 11_000);
    let commands = rig.take();
    assert_eq!(
        guide_commands(&commands),
        [
            Command::ClearOverlay(GUIDE_LAYER),
            Command::ClearOverlay(GUIDE_CLOCK_LAYER),
            Command::ClearOverlay(GUIDE_ROWS_LAYER),
            Command::SetVideoArea(VideoArea::Full),
        ]
    );
    assert!(!commands.contains(&Command::ShowStatic));
    assert_eq!(rig.tv.channel(), Some(3));
    // Up and Down change channel again once the guide is closed.
    rig.press(Key::Next, T + 12_000);
    assert_eq!(rig.tv.channel(), Some(5));
}

#[test]
fn opening_the_guide_takes_the_banner_away() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.press(Key::Guide, T + 3_000);
    let commands = rig.take();
    assert!(commands.contains(&Command::ClearBanner), "{commands:?}");
    // The banner's timeout has nothing left to clear.
    rig.step(T + 20_000);
    assert!(!rig.take().contains(&Command::ClearBanner));
}

#[test]
fn the_watermark_hides_while_the_guide_is_open() {
    let mut rig = Rig::watermarked();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.press(Key::Guide, T + 2_000);
    assert_eq!(watermark(&rig.take()), [None]);
}

#[test]
fn the_guide_shows_its_rows_from_a_tall_bitmap_and_three_channels_never_scroll() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.press(Key::Guide, T + 10_000);
    let rows: Vec<Command> = guide_commands(&rig.take())
        .into_iter()
        .filter(|c| matches!(c, Command::ShowOverlayRows { .. }))
        .collect();
    // Three channels: three rows of 92, all on screen, no repeat for a loop.
    assert_eq!(
        rows,
        [Command::ShowOverlayRows {
            id: GUIDE_ROWS_LAYER,
            x: 96,
            y: 660,
            width: 1728,
            height: 3 * 92,
            first_row: 0,
            rows: 3 * 92,
        }]
    );
    // Well past the idle time, nothing scrolls.
    rig.step(T + 30_000);
    let later = rig.take();
    assert!(
        !later
            .iter()
            .any(|c| matches!(c, Command::ScrollOverlay { .. }))
    );
}

fn menu_shown(commands: &[Command]) -> usize {
    shown(MENU_LAYER, commands)
}

#[test]
fn the_menu_key_opens_the_picture_menu_and_ok_steps_the_crt_filter() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.press(Key::Menu, T + 10_000);
    let commands = rig.take();
    assert_eq!(menu_shown(&commands), 1);
    assert!(commands.contains(&Command::ClearBanner), "{commands:?}");
    // The rig starts at Off (its settings): OK goes on to Heavy.
    rig.press(Key::Banner, T + 11_000);
    let commands = rig.take();
    assert!(
        commands.contains(&Command::SetCrt(Crt::Heavy)),
        "{commands:?}"
    );
    assert_eq!(menu_shown(&commands), 1, "drawn again with the new level");
    assert_eq!(
        rig.store.viewer_setting("crt").unwrap().as_deref(),
        Some("heavy")
    );
    rig.press(Key::Banner, T + 12_000);
    assert!(rig.take().contains(&Command::SetCrt(Crt::Light)));
    assert_eq!(
        rig.store.viewer_setting("crt").unwrap().as_deref(),
        Some("light")
    );
    // Up and Down have nothing to move between, and change no channel.
    rig.press(Key::Next, T + 13_000);
    assert_eq!(rig.take(), []);
    assert_eq!(rig.tv.channel(), Some(3));
    rig.press(Key::Back, T + 14_000);
    assert_eq!(rig.take(), [Command::ClearOverlay(MENU_LAYER)]);
    // Closed: Up changes channel again.
    rig.press(Key::Next, T + 15_000);
    assert_eq!(rig.tv.channel(), Some(5));
}

#[test]
fn a_stored_crt_filter_wins_over_the_default_at_start() {
    let mut rig = Rig::new();
    rig.store.set_viewer_setting("crt", "light").unwrap();
    rig.start();
    assert!(rig.take().contains(&Command::SetCrt(Crt::Light)));
}

#[test]
fn the_guide_key_in_the_menu_closes_it_and_opens_the_guide() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Menu, T + 10_000);
    rig.take();
    rig.press(Key::Guide, T + 11_000);
    let commands = rig.take();
    assert_eq!(commands[0], Command::ClearOverlay(MENU_LAYER));
    assert!(commands.contains(&Command::SetVideoArea(VideoArea::TopRight)));
}

#[test]
fn the_watermark_hides_while_the_menu_is_open() {
    let mut rig = Rig::watermarked();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.press(Key::Menu, T + 1_000);
    assert_eq!(watermark(&rig.take()), [None]);
}

#[test]
fn right_and_left_move_the_guide_through_time() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Guide, T + 10_000);
    rig.take();
    rig.press(Key::Right, T + 10_200);
    assert_eq!(
        shown(GUIDE_LAYER, &rig.take()),
        1,
        "drawn for the next half hour"
    );
    rig.press(Key::Left, T + 10_400);
    assert_eq!(shown(GUIDE_LAYER, &rig.take()), 1, "and back");
    rig.press(Key::Left, T + 10_600);
    assert_eq!(shown(GUIDE_LAYER, &rig.take()), 0, "not before now");
    assert_eq!(rig.tv.channel(), Some(3));
}

#[test]
fn left_and_right_do_nothing_outside_the_guide() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.step(T + 9_000);
    rig.take();
    rig.press(Key::Left, T + 10_000);
    rig.press(Key::Right, T + 10_500);
    assert_eq!(rig.take(), []);
    assert_eq!(rig.tv.channel(), Some(3));
}

#[test]
fn the_web_admin_tunes_known_channels_and_sets_the_crt_filter() {
    let mut rig = Rig::new();
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.take();
    rig.tv
        .admin(AdminCommand::Tune(7), T + 10_000, &mut rig.store);
    assert_eq!(rig.tv.channel(), Some(7));
    assert!(rig.take().contains(&Command::ShowStatic));
    rig.tv
        .admin(AdminCommand::Tune(42), T + 11_000, &mut rig.store);
    assert_eq!(rig.tv.channel(), Some(7), "an unknown channel is ignored");
    rig.tv
        .admin(AdminCommand::SetCrt(Crt::Light), T + 12_000, &mut rig.store);
    assert_eq!(rig.tv.crt(), Crt::Light);
    assert!(rig.take().contains(&Command::SetCrt(Crt::Light)));
    assert_eq!(
        rig.store.viewer_setting("crt").unwrap().as_deref(),
        Some("light")
    );
}

#[test]
fn a_channel_changed_in_the_web_admin_is_regenerated_after_the_freeze() {
    let mut rig = Rig::new();
    rig.start();
    let before = rig.store.events_between(3, T, i64::MAX).unwrap();
    let seven = rig.store.events_between(7, T, i64::MAX).unwrap();

    let mut config = rig.tv.config.clone();
    let three = config.channels.iter_mut().find(|c| c.number == 3).unwrap();
    three.name = "TRES".into();
    three.grid = Some(Duration::from_secs(90 * 60));
    rig.tv.admin(
        AdminCommand::Reconfigure(Box::new(config)),
        T + MIN,
        &mut rig.store,
    );

    assert_eq!(rig.tv.channel_config(3).unwrap().name, "TRES");
    let after = rig.store.events_between(3, T, i64::MAX).unwrap();
    // The program on screen is frozen; from the freeze on the grid applies.
    assert_eq!(after[0], before[0]);
    assert_ne!(after, before);
    assert!(
        after[1..]
            .iter()
            .filter(|e| e.kind == EventKind::Program)
            .all(|e| e.start_ms % (90 * MIN) == 0),
        "{after:?}"
    );
    // A channel that did not change keeps its schedule, extended as usual.
    let seven_after = rig.store.events_between(7, T, i64::MAX).unwrap();
    assert_eq!(seven_after[..seven.len()], seven[..]);
    assert!(rig.tv.upkeep.reconfigured.is_empty());
}

#[test]
fn a_change_made_before_the_clock_is_trusted_waits_for_it() {
    let mut rig = Rig::new();
    rig.start();
    let before = rig.store.events_between(3, T, i64::MAX).unwrap();
    rig.trusted.set(false);
    let mut config = rig.tv.config.clone();
    config.channels[1].grid = Some(Duration::from_secs(90 * 60));
    rig.tv.admin(
        AdminCommand::Reconfigure(Box::new(config)),
        T + MIN,
        &mut rig.store,
    );
    assert_eq!(rig.store.events_between(3, T, i64::MAX).unwrap(), before);
    rig.trusted.set(true);
    rig.tv.step(T + 2 * MIN, &mut rig.store);
    assert_ne!(rig.store.events_between(3, T, i64::MAX).unwrap(), before);
}

// ---- music channels -----------------------------------------------------

type Answers = Vec<(MediaId, Option<Image>)>;

/// Covers whose requests a test reads and whose answers it sends.
#[derive(Clone, Default)]
struct TestCovers {
    asked: Rc<std::cell::RefCell<Vec<MediaId>>>,
    answers: Rc<std::cell::RefCell<Answers>>,
}

impl Covers for TestCovers {
    fn request(&mut self, id: &MediaId) {
        self.asked.borrow_mut().push(id.clone());
    }
    fn take(&mut self) -> Vec<(MediaId, Option<Image>)> {
        self.answers.borrow_mut().drain(..).collect()
    }
}

fn picture_layer(commands: &[Command]) -> Vec<Command> {
    commands
        .iter()
        .filter(|c| {
            matches!(
                c,
                Command::ShowOverlay {
                    id: PICTURE_LAYER,
                    ..
                } | Command::ClearOverlay(PICTURE_LAYER)
            )
        })
        .cloned()
        .collect()
}

const CARD: Command = Command::ShowOverlay {
    id: PICTURE_LAYER,
    x: 0,
    y: 0,
    width: 1920,
    height: 1080,
};

/// A PNG of one blue pixel, as Jellyfin would send a cover.
fn cover_png() -> Image {
    let mut canvas = paint::Canvas::new(1, 1);
    canvas.fill_rect(0.0, 0.0, 1.0, 1.0, paint::Rgba::hex(0x0000ff));
    Image::decode_png(&canvas.png()).unwrap()
}

#[test]
fn a_cover_channel_shows_the_card_for_each_song_and_hides_it_for_a_break() {
    let mut rig = Rig::music("cover");
    let covers = TestCovers::default();
    rig.tv.set_covers(Box::new(covers.clone()));
    let ad1 = rig.clip("ad1.mp4");
    rig.write(
        3,
        &[
            program(T - 10 * MIN, T + MIN, "song", 0),
            event(T + MIN, T + MIN + 30_000, &ad1, 0, EventKind::Break),
            program(T + MIN + 30_000, T + 50 * MIN, "single", 0),
        ],
    );
    rig.start();
    let started = rig.commands();
    assert!(started.contains(&Command::SetPicture(Some(Picture::Cover))));
    // Static and the tune-in clip have no card.
    rig.step(T + 500);
    assert_eq!(picture_layer(&rig.commands()), []);

    let live = rig.go_live(T);
    // The card shows as soon as the song is on, with the mark until the
    // cover arrives; the cover is asked for by the album's id.
    assert_eq!(picture_layer(&live), [CARD]);
    assert_eq!(*covers.asked.borrow(), [MediaId::new("alb")]);
    rig.step(T + 3_000);
    assert_eq!(picture_layer(&rig.take()), []);
    covers
        .answers
        .borrow_mut()
        .push((MediaId::new("alb"), Some(cover_png())));
    rig.step(T + 3_020);
    assert_eq!(picture_layer(&rig.take()), [CARD], "redrawn with the cover");
    rig.step(T + 3_040);
    assert_eq!(picture_layer(&rig.take()), []);

    // The break clip plays without the card.
    rig.step(T + MIN);
    let at_break = rig.take();
    assert!(
        at_break.contains(&Command::PlayClip(ad1.into())),
        "{at_break:?}"
    );
    assert_eq!(
        picture_layer(&at_break),
        [Command::ClearOverlay(PICTURE_LAYER)]
    );
    // The next song gets its own card; with no album it asks for its own image.
    rig.step(T + MIN + 30_000);
    rig.fake().program_started(0);
    rig.step(T + MIN + 30_020);
    assert_eq!(picture_layer(&rig.take()), [CARD]);
    assert_eq!(
        *covers.asked.borrow(),
        [MediaId::new("alb"), MediaId::new("single")]
    );
    // The guide hides it.
    rig.press(Key::Guide, T + 2 * MIN);
    assert_eq!(
        picture_layer(&rig.take()),
        [Command::ClearOverlay(PICTURE_LAYER)]
    );
}

#[test]
fn a_visualizer_channel_shows_the_now_playing_line() {
    let mut rig = Rig::music("visualizer");
    rig.write(3, &[program(T - 10 * MIN, T + 50 * MIN, "song", 0)]);
    rig.start();
    assert!(
        rig.commands()
            .contains(&Command::SetPicture(Some(Picture::Visualizer)))
    );

    let live = rig.go_live(T);

    let shown = picture_layer(&live);
    let [Command::ShowOverlay { x, y, width, .. }] = shown[..] else {
        panic!("{shown:?}");
    };
    assert_eq!((x, y), (80, 38));
    assert!(width < 1920, "a line, not a card");
}

#[test]
fn tuning_from_a_music_channel_to_a_video_channel_puts_the_picture_back() {
    let mut rig = Rig::music("cover");
    hour_everywhere(&mut rig);
    rig.start();
    rig.go_live(T);
    rig.press(Key::Next, T + MIN);
    let commands = rig.take();
    assert!(
        commands.contains(&Command::SetPicture(None)),
        "{commands:?}"
    );
    // Going on to another video channel sends nothing more.
    rig.go_live(T + MIN);
    rig.press(Key::Next, T + 2 * MIN);
    assert!(
        !rig.take()
            .iter()
            .any(|c| matches!(c, Command::SetPicture(_)))
    );
}
