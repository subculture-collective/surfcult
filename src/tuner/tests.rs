//! Every test drives the tuner by hand with a virtual clock: a `FakePlayer`,
//! an in-memory `Store`, a `FakeMediaSource` and a fake prober. Nothing sleeps
//! and nothing needs MPV, Jellyfin or ffprobe.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use tempfile::TempDir;

use super::*;
use crate::clips::{ClipLibrary, ProbeError, Prober};
use crate::config::InterstitialPool;
use crate::library::{AudioStream, FakeMediaSource, MediaItem, MediaKind, VideoStream};
use crate::player::{Command, FakePlayer, VirtualClock};

/// The moment the key is pressed in most tests.
const T: i64 = 10_000_000;
const CHANNEL: u32 = 5;

fn clip(name: &str, ms: u64) -> Clip {
    Clip {
        path: PathBuf::from(format!("/clips/{name}")),
        duration: Duration::from_millis(ms),
    }
}

/// 0.4 s, 1.2 s, 3 s and 8 s. With the default timing a direct-play program
/// gets the 1.2 s clip and a transcode gets the 8 s clip.
fn tune_in_clips() -> Vec<Clip> {
    vec![
        clip("a-tiny.mp4", 400),
        clip("b-short.mp4", 1_200),
        clip("c-mid.mp4", 3_000),
        clip("d-long.mp4", 8_000),
    ]
}

fn event(start_ms: i64, end_ms: i64, media: &str, offset_ms: u64, kind: EventKind) -> Event {
    Event {
        start_ms,
        end_ms,
        media_id: MediaId::new(media),
        media_offset_ms: offset_ms,
        media_duration_ms: 3_600_000,
        kind,
    }
}

/// `show` has been on since 100 s before the key press and runs for an hour.
fn long_program() -> Vec<Event> {
    vec![event(
        T - 100_000,
        T + 3_600_000,
        "show",
        0,
        EventKind::Program,
    )]
}

fn item(id: &str, codec: &str, height: u32, bit_depth: u8) -> MediaItem {
    let mut item = MediaItem::new(id, id, MediaKind::Movie, 3_600_000);
    item.video.push(VideoStream {
        codec: codec.into(),
        width: height * 16 / 9,
        height,
        bit_depth: Some(bit_depth),
    });
    item.audio.push(AudioStream {
        codec: "aac".into(),
        channels: 2,
        language: None,
    });
    item
}

fn direct(id: &str) -> MediaItem {
    item(id, "hevc", 1080, 10)
}

fn transcoded(id: &str) -> MediaItem {
    item(id, "av1", 1080, 8)
}

struct Rig {
    tuner: Tuner<FakePlayer>,
    store: Store,
    media: FakeMediaSource,
    clock: VirtualClock,
}

impl Rig {
    fn new(events: Vec<Event>, items: Vec<MediaItem>, clips: Vec<Clip>) -> Self {
        Self::on_channel(CHANNEL, events, items, clips)
    }

    fn on_channel(
        channel: u32,
        events: Vec<Event>,
        items: Vec<MediaItem>,
        clips: Vec<Clip>,
    ) -> Self {
        let mut store = Store::open_in_memory().unwrap();
        if !events.is_empty() {
            store.write_events(channel, &events).unwrap();
        }
        let media = items
            .into_iter()
            .fold(FakeMediaSource::new(), FakeMediaSource::with_item);
        let clock = VirtualClock::new(T);
        let tuner = Tuner::new(FakePlayer::new(clock.clone()), clips, Timing::default());
        Self {
            tuner,
            store,
            media,
            clock,
        }
    }

    fn tune(&mut self, channel: u32, now: i64) {
        self.clock.set(now);
        self.tuner.tune(channel, now);
    }

    /// Moves the clock to `now` and lets the tuner handle what is due.
    fn at(&mut self, now: i64) {
        self.clock.set(now);
        self.tuner.pump(now, &self.store, &self.media);
    }

    fn issued(&self) -> Vec<(i64, Command)> {
        self.tuner
            .player()
            .issued()
            .iter()
            .map(|i| (i.at_ms, i.command.clone()))
            .collect()
    }

    fn commands(&self) -> Vec<Command> {
        self.tuner.player().commands()
    }

    fn seeks(&self) -> Vec<u64> {
        self.commands()
            .into_iter()
            .filter_map(|c| match c {
                Command::Seek(to) => Some(to),
                _ => None,
            })
            .collect()
    }

    fn failed_with(&self) -> Option<&Reason> {
        match self.tuner.state() {
            State::Failed { reason, .. } => Some(reason),
            _ => None,
        }
    }
}

fn play(name: &str) -> Command {
    Command::PlayClip(PathBuf::from(format!("/clips/{name}")))
}

fn queue(media: &str, start_ms: u64) -> Command {
    Command::QueueProgram {
        media_id: MediaId::new(media),
        start_ms,
    }
}

// The sequence

#[test]
fn a_tune_shows_static_then_the_clip_then_the_program_at_the_look_ahead_offset() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());

    rig.tune(CHANNEL, T);
    assert_eq!(rig.issued(), [(T, Command::ShowStatic)]);
    assert_eq!(
        rig.tuner.state(),
        &State::Static {
            channel: CHANNEL,
            until_ms: T + 500
        }
    );
    assert_eq!(rig.tuner.next_wakeup(), Some(T + 500));

    // Static stays up for 0.5 s.
    rig.at(T + 499);
    assert_eq!(rig.issued().len(), 1);

    // Then the 1.2 s clip, with the program for T + 0.5 s + 1.2 s queued behind
    // it. `show` started 100 s before T, so that is 101.7 s in.
    rig.at(T + 500);
    assert_eq!(
        rig.issued(),
        [
            (T, Command::ShowStatic),
            (T + 500, play("b-short.mp4")),
            (T + 500, queue("show", 101_700)),
        ]
    );
    assert_eq!(rig.tuner.state(), &State::ClipPlaying { channel: CHANNEL });
    assert_eq!(rig.tuner.next_wakeup(), None);

    // The clip ends and the program starts where it was queued: on time.
    rig.tuner.player_mut().clip_ended();
    rig.at(T + 1_700);
    assert_eq!(
        rig.tuner.state(),
        &State::AwaitingProgram { channel: CHANNEL }
    );
    rig.tuner.player_mut().program_started(101_700);
    rig.at(T + 1_700);
    assert_eq!(rig.tuner.state(), &State::Live { channel: CHANNEL });
    assert!(rig.seeks().is_empty());
    assert_eq!(rig.issued().len(), 3);
}

#[test]
fn the_offset_follows_the_events_media_offset() {
    // `show` resumes at 20 minutes into the file after an earlier break.
    let events = vec![event(
        T - 5_000,
        T + 600_000,
        "show",
        1_200_000,
        EventKind::Program,
    )];
    let mut rig = Rig::new(events, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.commands()[2], queue("show", 1_200_000 + 5_000 + 1_700));
}

#[test]
fn a_program_that_needs_a_transcode_gets_a_longer_clip() {
    let mut rig = Rig::new(long_program(), vec![transcoded("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    // 8 s is the shortest clip that covers the 7 s transcode start-up, and the
    // program is queued for T + 0.5 s + 8 s.
    assert_eq!(
        rig.issued(),
        [
            (T, Command::ShowStatic),
            (T + 500, play("d-long.mp4")),
            (T + 500, queue("show", 108_500)),
        ]
    );
}

#[test]
fn the_longest_clip_is_used_when_none_covers_the_start_up_cost() {
    let clips = vec![clip("one.mp4", 2_000), clip("two.mp4", 4_000)];
    let mut rig = Rig::new(long_program(), vec![transcoded("show")], clips);
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.commands()[1], play("two.mp4"));
    assert_eq!(rig.commands()[2], queue("show", 104_500));
}

#[test]
fn the_cost_comes_from_the_program_that_will_be_on_when_the_clip_ends() {
    // `old` is on now but ends 1 s in, so `new` is what follows. `old` needs a
    // transcode, `new` does not: the plan uses `new`.
    let events = vec![
        event(T - 100_000, T + 1_000, "old", 0, EventKind::Program),
        event(T + 1_000, T + 600_000, "new", 0, EventKind::Program),
    ];
    let items = vec![transcoded("old"), direct("new")];
    let mut rig = Rig::new(events, items, tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.commands()[1], play("b-short.mp4"));
    assert_eq!(rig.commands()[2], queue("new", 700));
}

#[test]
fn a_transcode_found_only_with_the_short_clip_switches_to_the_long_clip() {
    // At T + 1.7 s the program is `new`, which needs a transcode, so the
    // tuner plans again with a clip of 7 s or more. Ending at T + 8.5 s the
    // program is still `new`.
    let events = vec![
        event(T - 100_000, T + 1_000, "old", 0, EventKind::Program),
        event(T + 1_000, T + 600_000, "new", 0, EventKind::Program),
    ];
    let items = vec![direct("old"), transcoded("new")];
    let mut rig = Rig::new(events, items, tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.commands()[1], play("d-long.mp4"));
    assert_eq!(rig.commands()[2], queue("new", 7_500));
}

#[test]
fn clips_come_from_a_scanned_pool() {
    struct Durations(HashMap<&'static str, u64>);
    impl Prober for Durations {
        fn duration(&self, path: &Path) -> Result<Duration, ProbeError> {
            let name = path.file_name().unwrap().to_string_lossy();
            match self.0.get(name.as_ref()) {
                Some(ms) => Ok(Duration::from_millis(*ms)),
                None => Err(ProbeError::Failed(Some(1))),
            }
        }
    }

    let dir = TempDir::new().unwrap();
    for name in ["short.mp4", "long.mp4", "broken.mp4"] {
        fs::write(dir.path().join(name), "x").unwrap();
    }
    let prober = Durations(HashMap::from([("short.mp4", 1_500), ("long.mp4", 9_000)]));
    let library = ClipLibrary::scan(
        &[InterstitialPool {
            name: "tune-in".into(),
            path: dir.path().to_path_buf(),
        }],
        prober,
    );
    let pool = library.pool("tune-in").unwrap();
    assert_eq!(pool.issues().len(), 1, "the broken file is skipped");

    let mut rig = Rig::new(
        long_program(),
        vec![transcoded("show")],
        pool.clips().to_vec(),
    );
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    let Command::PlayClip(path) = &rig.commands()[1] else {
        panic!("expected a clip, got {:?}", rig.commands());
    };
    assert_eq!(path.file_name().unwrap(), "long.mp4");
}

#[test]
fn clip_choice_is_the_shortest_that_is_long_enough_else_the_longest() {
    let clips = vec![
        clip("c.mp4", 3_000),
        clip("a.mp4", 1_000),
        clip("b.mp4", 1_000),
        clip("d.mp4", 6_000),
    ];
    let name = |cost| {
        choose_clip(&clips, cost)
            .unwrap()
            .path
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned()
    };
    // A clip exactly as long as the cost is long enough; ties go to the first.
    assert_eq!(name(1_000), "a.mp4");
    assert_eq!(name(1_001), "c.mp4");
    assert_eq!(name(3_000), "c.mp4");
    assert_eq!(name(6_000), "d.mp4");
    assert_eq!(name(6_001), "d.mp4");
    assert_eq!(name(0), "a.mp4");
    assert!(choose_clip(&[], 1_000).is_none());
    // Of equal longest clips the first wins.
    let equal = vec![clip("x.mp4", 2_000), clip("y.mp4", 2_000)];
    assert_eq!(
        choose_clip(&equal, 9_000).unwrap().path,
        PathBuf::from("/clips/x.mp4")
    );
}

// Breaks

#[test]
fn a_clip_that_ends_in_a_break_stands_in_for_it() {
    // The clip ends at T + 1.7 s. The break runs from T + 1 s to T + 35 s
    // (an ad, then an ident), then `show` resumes 10 minutes in.
    let events = vec![
        event(T - 100_000, T + 1_000, "show", 0, EventKind::Program),
        event(T + 1_000, T + 30_000, "ad", 0, EventKind::Break),
        event(T + 30_000, T + 35_000, "ident", 0, EventKind::Ident),
        event(T + 35_000, T + 900_000, "show", 600_000, EventKind::Program),
    ];
    let mut rig = Rig::new(events, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);

    // One clip, then the program that follows the break at its start offset.
    // No second clip is queued for the break.
    assert_eq!(
        rig.commands(),
        [
            Command::ShowStatic,
            play("b-short.mp4"),
            queue("show", 600_000)
        ]
    );

    // The program starts early, while the break is still on. That is not a
    // late start, so there is no seek.
    rig.tuner.player_mut().clip_ended();
    rig.tuner.player_mut().program_started(600_000);
    rig.at(T + 1_700);
    assert_eq!(rig.tuner.state(), &State::Live { channel: CHANNEL });
    assert!(rig.seeks().is_empty());
}

#[test]
fn filler_counts_as_a_break() {
    let events = vec![
        event(T - 100_000, T + 1_000, "show", 0, EventKind::Program),
        event(T + 1_000, T + 20_000, "fill", 0, EventKind::Filler),
        event(T + 20_000, T + 900_000, "next", 0, EventKind::Program),
    ];
    let mut rig = Rig::new(events, vec![direct("next")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.commands()[2], queue("next", 0));
}

#[test]
fn a_break_that_ends_before_the_clip_does_not_stand_in() {
    // The break is over at T + 1.2 s, before the clip ends at T + 1.7 s.
    let events = vec![
        event(T - 100_000, T + 400, "show", 0, EventKind::Program),
        event(T + 400, T + 1_200, "ad", 0, EventKind::Break),
        event(T + 1_200, T + 900_000, "show", 50_000, EventKind::Program),
    ];
    let mut rig = Rig::new(events, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    // 0.5 s into the second `show` event.
    assert_eq!(rig.commands()[2], queue("show", 50_500));
}

#[test]
fn a_break_followed_by_a_gap_or_nothing_ends_on_static() {
    let gap_after = vec![
        event(T - 100_000, T + 1_000, "show", 0, EventKind::Program),
        event(T + 1_000, T + 30_000, "ad", 0, EventKind::Break),
        event(T + 60_000, T + 900_000, "show", 600_000, EventKind::Program),
    ];
    let mut rig = Rig::new(gap_after, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(
        rig.failed_with(),
        Some(&Reason::Gap {
            next_start_ms: Some(T + 60_000)
        })
    );

    let last_event = vec![
        event(T - 100_000, T + 1_000, "show", 0, EventKind::Program),
        event(T + 1_000, T + 30_000, "ad", 0, EventKind::Break),
    ];
    let mut rig = Rig::new(last_event, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(
        rig.failed_with(),
        Some(&Reason::Gap {
            next_start_ms: None
        })
    );
    assert_eq!(rig.commands(), [Command::ShowStatic]);
}

// Late start

#[test]
fn a_late_start_gets_exactly_one_correction_seek() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().clip_ended();
    // The program was queued for 101.7 s and starts at T + 4.5 s, 2.8 s late.
    // Live is then 100 s + 4.5 s.
    rig.tuner.player_mut().program_started(101_700);
    rig.at(T + 4_500);
    assert_eq!(rig.seeks(), [104_500]);
    assert_eq!(rig.issued().last().unwrap().0, T + 4_500);
    assert_eq!(rig.tuner.state(), &State::Live { channel: CHANNEL });

    // Another report for the same program, or any later tick, seeks no more.
    rig.tuner.player_mut().program_started(104_600);
    rig.tuner.player_mut().program_started(101_700);
    rig.at(T + 9_000);
    rig.at(T + 60_000);
    assert_eq!(rig.seeks(), [104_500]);
}

#[test]
fn a_start_within_the_tolerance_is_left_alone() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().program_started(101_700);
    // Live is 102.2 s: exactly 500 ms ahead of where the program started.
    rig.at(T + 2_200);
    assert!(rig.seeks().is_empty());

    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().program_started(101_700);
    // One millisecond more is a late start.
    rig.at(T + 2_201);
    assert_eq!(rig.seeks(), [102_201]);
}

#[test]
fn a_program_that_starts_ahead_is_not_sought_back() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().program_started(105_000);
    rig.at(T + 1_700);
    assert!(rig.seeks().is_empty());
}

#[test]
fn no_seek_when_the_live_event_is_no_longer_the_program() {
    // The program ends at T + 2 s and an ad follows, so a start at T + 4 s has
    // no live position to seek to.
    let events = vec![
        event(T - 100_000, T + 2_000, "show", 0, EventKind::Program),
        event(T + 2_000, T + 60_000, "ad", 0, EventKind::Break),
        event(T + 60_000, T + 900_000, "show", 500_000, EventKind::Program),
    ];
    let mut rig = Rig::new(events, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().program_started(101_700);
    rig.at(T + 4_000);
    assert!(rig.seeks().is_empty());
    assert_eq!(rig.tuner.state(), &State::Live { channel: CHANNEL });
}

// Cancelling

#[test]
fn a_second_tune_during_static_cancels_the_first() {
    let mut store = Store::open_in_memory().unwrap();
    store.write_events(5, &long_program()).unwrap();
    let other = vec![event(
        T - 10_000,
        T + 900_000,
        "other",
        0,
        EventKind::Program,
    )];
    store.write_events(7, &other).unwrap();
    let clock = VirtualClock::new(T);
    let media = FakeMediaSource::new()
        .with_item(direct("show"))
        .with_item(direct("other"));
    let mut tuner = Tuner::new(
        FakePlayer::new(clock.clone()),
        tune_in_clips(),
        Timing::default(),
    );

    tuner.tune(5, T);
    clock.set(T + 200);
    tuner.tune(7, T + 200);
    assert_eq!(
        tuner.state(),
        &State::Static {
            channel: 7,
            until_ms: T + 700
        }
    );

    // The first sequence's deadline passes and nothing happens.
    clock.set(T + 500);
    tuner.pump(T + 500, &store, &media);
    assert_eq!(tuner.player().commands().len(), 2);

    clock.set(T + 700);
    tuner.pump(T + 700, &store, &media);
    assert_eq!(
        tuner.player().commands(),
        [
            Command::ShowStatic,
            Command::ShowStatic,
            play("b-short.mp4"),
            queue("other", 11_900),
        ]
    );
    // Nothing was ever queued for channel 5's program.
    assert!(!tuner.player().commands().contains(&queue("show", 0)));
}

#[test]
fn a_second_tune_during_the_clip_cancels_it() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.tuner.state(), &State::ClipPlaying { channel: CHANNEL });

    // Static replaces the clip and the queued program in the player.
    rig.tune(CHANNEL, T + 900);
    assert_eq!(
        rig.tuner.state(),
        &State::Static {
            channel: CHANNEL,
            until_ms: T + 1_400
        }
    );

    // A program-started report from the cancelled sequence is ignored: it
    // must not trigger a seek.
    rig.tuner.player_mut().program_started(101_700);
    rig.at(T + 1_000);
    assert!(rig.seeks().is_empty());
    assert!(matches!(rig.tuner.state(), State::Static { .. }));

    // The new sequence plans from its own times: clip at T + 1.4 s, program
    // for T + 2.6 s.
    rig.at(T + 1_400);
    assert_eq!(rig.commands()[4], play("b-short.mp4"));
    assert_eq!(rig.commands()[5], queue("show", 102_600));
}

#[test]
fn stop_cancels_the_sequence_and_stops_the_player() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.tuner.stop().unwrap();
    assert_eq!(rig.tuner.state(), &State::Idle);
    rig.at(T + 500);
    assert_eq!(rig.commands(), [Command::ShowStatic, Command::Stop]);
}

// A restarted player

#[test]
fn a_restarted_player_is_tuned_to_the_same_channel_again() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().clip_ended();
    rig.tuner.player_mut().program_started(101_700);
    rig.at(T + 1_700);
    assert_eq!(rig.tuner.state(), &State::Live { channel: CHANNEL });
    rig.tuner.player_mut().clear();

    // MPV died a minute later. The tuner runs the whole sequence again and
    // queues the program where the schedule is then.
    rig.tuner.player_mut().restarted();
    rig.at(T + 60_000);
    assert_eq!(
        rig.tuner.state(),
        &State::Static {
            channel: CHANNEL,
            until_ms: T + 60_500
        }
    );
    rig.at(T + 60_500);
    assert_eq!(
        rig.commands(),
        [
            Command::ShowStatic,
            play("b-short.mp4"),
            queue("show", 161_700)
        ]
    );
}

#[test]
fn a_restart_after_a_player_failure_tunes_again() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tuner.player_mut().fail_commands("socket closed");
    rig.tune(CHANNEL, T);
    assert!(matches!(rig.failed_with(), Some(Reason::Player(_))));

    let mut fresh = FakePlayer::new(rig.clock.clone());
    fresh.restarted();
    *rig.tuner.player_mut() = fresh;
    rig.at(T + 1_000);
    assert_eq!(
        rig.tuner.state(),
        &State::Static {
            channel: CHANNEL,
            until_ms: T + 1_500
        }
    );
}

#[test]
fn a_restart_with_nothing_tuned_does_nothing() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tuner.player_mut().restarted();
    rig.at(T);
    assert_eq!(rig.tuner.state(), &State::Idle);
    assert!(rig.commands().is_empty());
}

// Nothing to show

#[test]
fn an_empty_tune_in_pool_leaves_static_up() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], Vec::new());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.failed_with(), Some(&Reason::NoTuneInClip));
    assert_eq!(rig.commands(), [Command::ShowStatic]);
    assert_eq!(rig.tuner.next_wakeup(), None);
}

#[test]
fn a_channel_with_no_schedule_leaves_static_up() {
    let mut rig = Rig::new(Vec::new(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(rig.failed_with(), Some(&Reason::NoEvents));
    assert_eq!(rig.commands(), [Command::ShowStatic]);

    // A channel number nobody scheduled is the same.
    rig.tune(99, T + 10_000);
    rig.at(T + 10_500);
    assert_eq!(rig.failed_with(), Some(&Reason::NoEvents));
}

#[test]
fn a_gap_when_the_clip_ends_leaves_static_up() {
    let events = vec![
        event(T - 100_000, T + 1_000, "show", 0, EventKind::Program),
        event(T + 20_000, T + 900_000, "show", 50_000, EventKind::Program),
    ];
    let mut rig = Rig::new(events, vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert_eq!(
        rig.failed_with(),
        Some(&Reason::Gap {
            next_start_ms: Some(T + 20_000)
        })
    );
    assert_eq!(rig.commands(), [Command::ShowStatic]);
}

#[test]
fn a_program_the_media_source_does_not_know_leaves_static_up() {
    let mut rig = Rig::new(long_program(), Vec::new(), tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    assert!(matches!(rig.failed_with(), Some(Reason::Media(_))));
    assert_eq!(rig.commands(), [Command::ShowStatic]);
}

#[test]
fn a_failing_player_is_reported_not_panicked_on() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tuner.player_mut().fail_commands("socket closed");
    rig.tune(CHANNEL, T);
    assert_eq!(
        rig.failed_with(),
        Some(&Reason::Player("socket closed".into()))
    );
    rig.at(T + 500);
    assert!(rig.commands().is_empty());
}

#[test]
fn a_failed_sequence_ignores_late_player_events_until_the_next_tune() {
    let mut rig = Rig::new(Vec::new(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    rig.at(T + 500);
    rig.tuner.player_mut().clip_ended();
    rig.tuner.player_mut().program_started(0);
    rig.at(T + 2_000);
    assert_eq!(rig.failed_with(), Some(&Reason::NoEvents));
    assert!(rig.seeks().is_empty());
}

#[test]
fn ticking_with_nothing_due_does_nothing() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.at(T);
    assert_eq!(rig.tuner.state(), &State::Idle);
    assert!(rig.commands().is_empty());
}

#[test]
fn a_late_tick_plans_from_the_time_the_clip_actually_starts() {
    let mut rig = Rig::new(long_program(), vec![direct("show")], tune_in_clips());
    rig.tune(CHANNEL, T);
    // The caller was busy and ticks 300 ms late: the program is queued for
    // when this clip will end, T + 0.8 s + 1.2 s.
    rig.at(T + 800);
    assert_eq!(rig.issued()[1], (T + 800, play("b-short.mp4")));
    assert_eq!(rig.commands()[2], queue("show", 102_000));
}

#[test]
fn a_store_that_cannot_be_read_leaves_static_up_with_the_store_error() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("schedule.db");
    let mut rig = Rig::new(Vec::new(), vec![direct("show")], tune_in_clips());
    rig.store = Store::open(&path).unwrap();
    rig.store.write_events(CHANNEL, &long_program()).unwrap();
    // Another connection breaks the schema under the open store.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute_batch("DROP TABLE events")
        .unwrap();

    rig.tune(CHANNEL, T);
    rig.at(T + 500);

    assert!(
        matches!(
            rig.failed_with(),
            Some(Reason::Schedule(StoreError::Database { .. }))
        ),
        "{:?}",
        rig.failed_with()
    );
    assert_eq!(rig.commands(), [Command::ShowStatic]);
    assert_eq!(rig.tuner.next_wakeup(), None);
}

#[test]
fn a_schedule_failure_keeps_the_store_error() {
    let reason = Reason::Schedule(StoreError::Corrupt("bad row".to_owned()));
    assert_eq!(
        reason.to_string(),
        "cannot read the schedule: stored data is unreadable: bad row"
    );
}
