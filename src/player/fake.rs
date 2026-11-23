use std::cell::Cell;
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use super::{Player, PlayerError, PlayerEvent};
use crate::library::MediaId;

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
    PlayClip(PathBuf),
    QueueProgram { media_id: MediaId, start_ms: u64 },
    Seek(u64),
    Stop,
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
        self.events
            .push_back(PlayerEvent::ProgramStarted { position_ms });
    }

    fn record(&mut self, command: Command) -> Result<(), PlayerError> {
        if let Some(reason) = &self.failure {
            return Err(PlayerError(reason.clone()));
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

    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError> {
        self.record(Command::PlayClip(path.to_path_buf()))
    }

    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError> {
        self.record(Command::QueueProgram {
            media_id: media.clone(),
            start_ms,
        })
    }

    fn seek(&mut self, position_ms: u64) -> Result<(), PlayerError> {
        self.record(Command::Seek(position_ms))
    }

    fn stop(&mut self) -> Result<(), PlayerError> {
        self.record(Command::Stop)
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
        clock.set(1_500);
        player.play_clip(Path::new("/clips/a.mp4")).unwrap();
        player.queue_program(&MediaId::new("m"), 4_000).unwrap();
        player.seek(9_000).unwrap();
        player.stop().unwrap();
        let expected = [
            (1_000, Command::ShowStatic),
            (1_500, Command::PlayClip(PathBuf::from("/clips/a.mp4"))),
            (
                1_500,
                Command::QueueProgram {
                    media_id: MediaId::new("m"),
                    start_ms: 4_000,
                },
            ),
            (1_500, Command::Seek(9_000)),
            (1_500, Command::Stop),
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
        assert_eq!(player.poll_event(), Some(PlayerEvent::ClipEnded));
        assert_eq!(
            player.poll_event(),
            Some(PlayerEvent::ProgramStarted {
                position_ms: 12_000
            })
        );
        assert_eq!(player.poll_event(), None);
    }

    #[test]
    fn a_failing_player_returns_the_error_and_records_nothing() {
        let mut player = FakePlayer::new(VirtualClock::new(0));
        player.fail_commands("socket closed");
        assert_eq!(
            player.show_static(),
            Err(PlayerError("socket closed".into()))
        );
        assert!(player.issued().is_empty());
    }
}
