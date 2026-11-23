//! The screen as the tuner sees it.
//!
//! [`Player`] is a list of intentions: show static, play a local clip, queue a
//! program, seek, stop. It says nothing about MPV. The real implementation
//! (issue #22) turns these into MPV commands; [`FakePlayer`] records them so
//! tests can check the tuner without hardware.
//!
//! # Playlist rules
//!
//! The player keeps one item on screen and at most one queued behind it.
//!
//! - [`Player::show_static`] and [`Player::play_clip`] replace what is on
//!   screen and drop anything queued. Calling `show_static` is how a sequence
//!   is cancelled.
//! - [`Player::queue_program`] appends a program behind the current item. The
//!   player may open it early, so it starts as soon as the current item ends.
//!   One MPV process cannot hold a second file paused, so a program is queued
//!   rather than preloaded (ADR 0001, decision 5).
//! - [`Player::seek`] moves the item on screen to a position in milliseconds.
//!
//! # Events
//!
//! The player reports what happened through [`Player::poll_event`]. An adapter
//! must report [`PlayerEvent::ClipEnded`] only when a clip played to its end.
//! A clip that was replaced or stopped by a command must not produce one,
//! because the tuner cannot tell such an event from the end of a newer clip.
//! (For MPV, forward `end-file` with reason `eof` and drop `stop` and
//! `redirect`.)

mod fake;

use std::fmt;
use std::path::Path;

use crate::library::MediaId;

pub use fake::{Command, FakePlayer, Issued, VirtualClock};

/// Something the player did that the tuner needs to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    /// The clip that was playing reached its end.
    ClipEnded,
    /// The queued program is on screen, at this position in the media.
    ProgramStarted { position_ms: u64 },
}

/// A command the player could not carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerError(pub String);

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PlayerError {}

/// What the tuner needs from a video player.
pub trait Player {
    /// Replaces everything on screen and queued with TV static.
    fn show_static(&mut self) -> Result<(), PlayerError>;

    /// Replaces what is on screen with the local clip at `path`, and drops
    /// anything queued.
    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError>;

    /// Queues a program behind the current item. It will play `media`
    /// starting `start_ms` into it.
    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError>;

    /// Moves the item on screen to `position_ms` into the media.
    fn seek(&mut self, position_ms: u64) -> Result<(), PlayerError>;

    /// Stops playing and clears the screen.
    fn stop(&mut self) -> Result<(), PlayerError>;

    /// The next thing the player reports, oldest first, or `None`.
    fn poll_event(&mut self) -> Option<PlayerEvent>;
}
