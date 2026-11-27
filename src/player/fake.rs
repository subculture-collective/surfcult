use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{Overlay, Player, PlayerError, PlayerEvent, Slate, VideoArea};
use crate::library::MediaId;
use crate::overlay::Banner;
use crate::paint::Crt;

/// A clock that only moves when a test moves it. Clones share the time, so
/// the test keeps one and gives another to the [`FakePlayer`].
#[derive(Debug, Clone, Default)]
pub struct VirtualClock(Rc<Cell<i64>>);

impl VirtualClock {
    /// A clock at `now_ms`, in epoch milliseconds.
    pub fn new(now_ms: i64) -> Self {
        Self(Rc::new(Cell::new(now_ms)))
    }

    pub fn now(&self) -> i64 {
        self.0.get()
    }

    pub fn set(&self, now_ms: i64) {
        self.0.set(now_ms);
    }
}

/// One command the tuner gave the player.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    ShowStatic,
    ShowStandby,
    ShowSlate(Slate),
    PlayClip(PathBuf),
    QueueProgram {
        media_id: MediaId,
        start_ms: u64,
    },
    PlayProgram {
        media_id: MediaId,
        start_ms: u64,
    },
    QueueClip(PathBuf),
    LoopClip(PathBuf),
    QueueLoopingClip(PathBuf),
    Seek(u64),
    Stop,
    /// The overlay's place and size; the pixels are not recorded.
    ShowOverlay {
        id: u8,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
    },
    ClearOverlay(u8),
    /// Part of an overlay: its place and full size, and the rows shown.
    ShowOverlayRows {
        id: u8,
        x: i32,
        y: i32,
        width: u32,
        height: u32,
        first_row: u32,
        rows: u32,
    },
    ScrollOverlay {
        id: u8,
        first_row: u32,
        rows: u32,
    },
    ShowBanner(Banner),
    ClearBanner,
    SetVideoArea(VideoArea),
    SetCrt(Crt),
}

/// A command and the virtual time it was issued.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issued {
    pub at_ms: i64,
    pub command: Command,
}

/// A [`Player`] that plays nothing. It records every command with the time on
/// its [`VirtualClock`], and a test can make it report that a clip ended or a
/// program started.
#[derive(Debug)]
pub struct FakePlayer {
    clock: VirtualClock,
    issued: Vec<Issued>,
    events: VecDeque<PlayerEvent>,
    failure: Option<String>,
}

impl FakePlayer {
    pub fn new(clock: VirtualClock) -> Self {
        Self {
            clock,
            issued: Vec::new(),
            events: VecDeque::new(),
            failure: None,
        }
    }

    /// Every command so far, oldest first.
    pub fn issued(&self) -> &[Issued] {
        &self.issued
    }

    /// The commands without their times.
    pub fn commands(&self) -> Vec<Command> {
        self.issued.iter().map(|i| i.command.clone()).collect()
    }

    /// Forgets the commands recorded so far.
    pub fn clear(&mut self) {
        self.issued.clear();
    }

    /// Makes every later command fail with `reason`. The failed command is not
    /// recorded.
    pub fn fail_commands(&mut self, reason: impl Into<String>) {
        self.failure = Some(reason.into());
    }

    /// Tells the player the clip on screen reached its end.
    pub fn clip_ended(&mut self) {
        self.events.push_back(PlayerEvent::ClipEnded);
    }

    /// Tells the player the queued program started at `position_ms`.
    pub fn program_started(&mut self, position_ms: u64) {
        self.events.push_back(PlayerEvent::ProgramStarted {
            position_ms,
            duration_ms: 0,
        });
    }

    /// Tells the player a program started at `position_ms` and has a real
    /// duration of `duration_ms`.
    pub fn program_started_with_duration(&mut self, position_ms: u64, duration_ms: u64) {
        self.events.push_back(PlayerEvent::ProgramStarted {
            position_ms,
            duration_ms,
        });
    }

    /// Tells the player the item on screen could not be played.
    pub fn failed(&mut self) {
        self.events.push_back(PlayerEvent::Failed);
    }

    /// Tells the player its video player was restarted.
    pub fn restarted(&mut self) {
        self.events.push_back(PlayerEvent::Restarted);
    }

    fn record(&mut self, command: Command) -> Result<(), PlayerError> {
        if let Some(reason) = &self.failure {
            return Err(PlayerError::new(reason.clone()));
        }
        self.issued.push(Issued {
            at_ms: self.clock.now(),
            command,
        });
        Ok(())
    }
}

impl Player for FakePlayer {
    fn show_static(&mut self) -> Result<(), PlayerError> {
        self.record(Command::ShowStatic)
    }

    fn show_standby(&mut self) -> Result<(), PlayerError> {
        self.record(Command::ShowStandby)
    }

    fn show_slate(&mut self, slate: Slate) -> Result<(), PlayerError> {
        self.record(Command::ShowSlate(slate))
    }

    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.record(Command::PlayClip(path.to_path_buf()))
    }

    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        self.record(Command::QueueProgram {
            media_id: media.clone(),
            start_ms,
        })
    }

    fn play_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        self.record(Command::PlayProgram {
            media_id: media.clone(),
            start_ms,
        })
    }

    fn queue_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.record(Command::QueueClip(path.to_path_buf()))
    }

    fn loop_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.record(Command::LoopClip(path.to_path_buf()))
    }

    fn queue_looping_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.record(Command::QueueLoopingClip(path.to_path_buf()))
    }

    fn seek(&mut self, position_ms: u64) -> Result<(), PlayerError> {
        self.record(Command::Seek(position_ms))
    }

    fn stop(&mut self) -> Result<(), PlayerError> {
        self.record(Command::Stop)
    }

    fn show_overlay(&mut self, id: u8, overlay: &Overlay) -> Result<(), PlayerError> {
        self.record(Command::ShowOverlay {
            id,
            x: overlay.x,
            y: overlay.y,
            width: overlay.width,
            height: overlay.height,
        })
    }

    fn clear_overlay(&mut self, id: u8) -> Result<(), PlayerError> {
        self.record(Command::ClearOverlay(id))
    }

    fn show_banner(&mut self, banner: &Banner) -> Result<(), PlayerError> {
        self.record(Command::ShowBanner(banner.clone()))
    }

    fn clear_banner(&mut self) -> Result<(), PlayerError> {
        self.record(Command::ClearBanner)
    }

    fn set_video_area(&mut self, area: VideoArea) -> Result<(), PlayerError> {
        self.record(Command::SetVideoArea(area))
    }

    fn set_crt(&mut self, crt: Crt) -> Result<(), PlayerError> {
        self.record(Command::SetCrt(crt))
    }

    fn show_overlay_rows(
        &mut self,
        id: u8,
        overlay: &Overlay,
        first_row: u32,
        rows: u32,
    ) -> Result<(), PlayerError> {
        self.record(Command::ShowOverlayRows {
            id,
            x: overlay.x,
            y: overlay.y,
            width: overlay.width,
            height: overlay.height,
            first_row,
            rows,
        })
    }

    fn scroll_overlay(&mut self, id: u8, first_row: u32, rows: u32) -> Result<(), PlayerError> {
        self.record(Command::ScrollOverlay {
            id,
            first_row,
            rows,
        })
    }

    fn poll_event(&mut self) -> Option<PlayerEvent> {
        self.events.pop_front()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_commands_with_the_virtual_time() {
        let clock = VirtualClock::new(1_000);
        let mut player = FakePlayer::new(clock.clone());
        player.show_static().unwrap();
        player.show_standby().unwrap();
        clock.set(1_500);
        player.play_clip(Path::new("/clips/a.mp4")).unwrap();
        player.queue_program(&MediaId::new("m"), 4_000).unwrap();
        player.play_program(&MediaId::new("n"), 5_000).unwrap();
        player.queue_clip(Path::new("/clips/b.mp4")).unwrap();
        player.seek(9_000).unwrap();
        player.stop().unwrap();
        let overlay = Overlay {
            x: 10,
            y: 20,
            width: 2,
            height: 1,
            bgra: vec![0; 8],
        };
        player.show_overlay(3, &overlay).unwrap();
        player.clear_overlay(3).unwrap();
        let banner = Banner {
            number: 3,
            name: "CARTOONS".into(),
            title: "Show".into(),
            times: "19:00–19:30".into(),
            clock: "19:05".into(),
            next: "NEXT 19:30 Other".into(),
        };
        player.show_banner(&banner).unwrap();
        player.clear_banner().unwrap();
        let expected = [
            (1_000, Command::ShowStatic),
            (1_000, Command::ShowStandby),
            (1_500, Command::PlayClip(PathBuf::from("/clips/a.mp4"))),
            (
                1_500,
                Command::QueueProgram {
                    media_id: MediaId::new("m"),
                    start_ms: 4_000,
                },
            ),
            (
                1_500,
                Command::PlayProgram {
                    media_id: MediaId::new("n"),
                    start_ms: 5_000,
                },
            ),
            (1_500, Command::QueueClip(PathBuf::from("/clips/b.mp4"))),
            (1_500, Command::Seek(9_000)),
            (1_500, Command::Stop),
            (
                1_500,
                Command::ShowOverlay {
                    id: 3,
                    x: 10,
                    y: 20,
                    width: 2,
                    height: 1,
                },
            ),
            (1_500, Command::ClearOverlay(3)),
            (1_500, Command::ShowBanner(banner)),
            (1_500, Command::ClearBanner),
        ];
        let issued: Vec<_> = player
            .issued()
            .iter()
            .map(|i| (i.at_ms, i.command.clone()))
            .collect();
        assert_eq!(issued, expected);
    }

    #[test]
    fn reports_events_in_order() {
        let mut player = FakePlayer::new(VirtualClock::new(0));
        assert_eq!(player.poll_event(), None);
        player.clip_ended();
        player.program_started(12_000);
        player.restarted();
        assert_eq!(player.poll_event(), Some(PlayerEvent::ClipEnded));
        assert_eq!(
            player.poll_event(),
            Some(PlayerEvent::ProgramStarted {
                position_ms: 12_000,
                duration_ms: 0,
            })
        );
        assert_eq!(player.poll_event(), Some(PlayerEvent::Restarted));
        assert_eq!(player.poll_event(), None);
    }

    #[test]
    fn a_failing_player_returns_the_error_and_records_nothing() {
        let mut player = FakePlayer::new(VirtualClock::new(0));
        player.fail_commands("socket closed");
        assert_eq!(player.show_static(), Err(PlayerError::new("socket closed")));
        assert!(player.issued().is_empty());
    }
}
