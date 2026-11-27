//! The screen as the tuner sees it.
//!
//! [`Player`] is a list of intentions: show static or the stand-by card, play
//! or queue a local clip, play or queue a program, seek, stop, show or clear
//! an overlay, and show or clear the channel banner. It says nothing about
//! MPV. [`MpvPlayer`] turns these into commands for a real MPV process (see
//! the [`mpv`] module); [`FakePlayer`] records them so tests can check the
//! tuner without hardware.
//!
//! # Playlist rules
//!
//! The player keeps one item on screen and at most one queued behind it.
//!
//! - [`Player::show_static`], [`Player::show_standby`],
//!   [`Player::show_slate`], [`Player::play_clip`] and
//!   [`Player::play_program`] replace what is on
//!   screen and drop anything queued. Calling `show_static` is how a sequence
//!   is cancelled.
//! - [`Player::queue_program`] and [`Player::queue_clip`] append an item
//!   behind the current one. The player may open it early, so it starts as
//!   soon as the current item ends. One MPV process cannot hold a second file
//!   paused, so a program is queued rather than preloaded (ADR 0001,
//!   decision 5).
//! - [`Player::seek`] moves the item on screen to a position in milliseconds.
//! - [`Player::show_overlay`] draws a bitmap over whatever is on screen, in one
//!   of 64 numbered layers, and [`Player::clear_overlay`] removes it. Overlays
//!   stay up when the item on screen changes.
//! - [`Player::show_banner`] shows the channel banner, text drawn over the
//!   picture, and [`Player::clear_banner`] removes it. Unlike an overlay, the
//!   banner goes when a command replaces what is on screen or stops it;
//!   queueing an item leaves it up.
//!
//! # Events
//!
//! The player reports what happened through [`Player::poll_event`]. An adapter
//! must report [`PlayerEvent::ClipEnded`] only when a clip played to its end.
//! A clip that was replaced or stopped by a command must not produce one,
//! because the tuner cannot tell such an event from the end of a newer clip.
//! (For MPV, forward `end-file` with reason `eof` and drop `stop` and
//! `redirect`.)
//!
//! An adapter reports [`PlayerEvent::Failed`] when the clip or program on
//! screen could not be played: it would not open, or it broke off with an
//! error. It reports nothing for static or the stand-by card.
//!
//! An adapter that restarts its video player reports
//! [`PlayerEvent::Restarted`]. Everything was lost: the item on screen, the
//! queue and the overlays. The caller issues them again.

mod fake;
mod filter;
pub mod mpv;

use std::fmt;
use std::path::Path;

use crate::library::MediaId;
use crate::overlay::Banner;

pub use fake::{Command, FakePlayer, Issued, VirtualClock};
pub use filter::{DEFAULT_GLITCH_EVERY, DEFAULT_STANDBY_FILTERS, Filter, standby_filters};
pub use mpv::MpvPlayer;

/// Something the player did that the tuner needs to know about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEvent {
    /// The clip that was playing reached its end.
    ClipEnded,
    /// The queued program is on screen, at this position in the media.
    ProgramStarted { position_ms: u64, duration_ms: u64 },
    /// The clip or program on screen could not be played: it would not
    /// open, or it broke off with an error. Nothing plays until the next
    /// command.
    Failed,
    /// The video player stopped working and was started again. The screen
    /// is blank, nothing is queued and no overlay is shown.
    Restarted,
}

/// A message the stand-by card carries in place of its own
/// ([`Player::show_slate`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slate {
    /// The program source cannot be reached, so programs cannot play.
    TechnicalDifficulties,
    /// The system clock is not synchronized yet, so nothing is scheduled.
    ClockNotSet,
}

impl Slate {
    /// The text across the middle of the card.
    pub fn text(self) -> &'static str {
        match self {
            Self::TechnicalDifficulties => "TECHNICAL DIFFICULTIES",
            Self::ClockNotSet => "SETTING THE CLOCK",
        }
    }
}

/// A bitmap drawn over the video, `width` by `height` pixels with its top
/// left corner at `x`, `y` on the screen.
///
/// `bgra` holds the pixels row by row, four bytes each in the order blue,
/// green, red, alpha, with the colour already multiplied by alpha (MPV's
/// `bgra` format). Its length must be `width * height * 4`.
#[derive(Clone, PartialEq, Eq)]
pub struct Overlay {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl fmt::Debug for Overlay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Overlay")
            .field("x", &self.x)
            .field("y", &self.y)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("bgra_len", &self.bgra.len())
            .finish()
    }
}

/// A command the player could not carry out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayerError {
    message: String,
    unreachable: bool,
}

impl PlayerError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            unreachable: false,
        }
    }

    /// The program source did not answer, so every program would fail the
    /// same way until it does: a connection that failed or timed out, not
    /// one item it refused.
    pub fn unreachable(message: impl Into<String>) -> Self {
        Self {
            unreachable: true,
            ..Self::new(message)
        }
    }

    pub fn is_unreachable(&self) -> bool {
        self.unreachable
    }
}

impl fmt::Display for PlayerError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for PlayerError {}

/// What the tuner needs from a video player.
pub trait Player {
    /// Replaces everything on screen and queued with TV static.
    fn show_static(&mut self) -> Result<(), PlayerError>;

    /// Replaces everything on screen and queued with the stand-by card,
    /// colour bars with a message, for a channel that has nothing to play.
    /// The player removes the card's own drawing and display filters when
    /// anything else is shown, queued or stopped; overlays from
    /// [`Player::show_overlay`] are not touched. A player that cannot show
    /// the whole card shows static instead and returns the error.
    fn show_standby(&mut self) -> Result<(), PlayerError>;

    /// As [`Player::show_standby`], with `slate`'s text across the card in
    /// place of the card's own.
    fn show_slate(&mut self, slate: Slate) -> Result<(), PlayerError>;

    /// Replaces what is on screen with the local clip at `path`, and drops
    /// anything queued.
    fn play_clip(&mut self, path: &Path) -> Result<(), PlayerError>;

    /// As [`Player::play_clip`], but the clip starts again each time it ends,
    /// until something replaces it: for a clip shorter than its event.
    fn loop_clip(&mut self, path: &Path) -> Result<(), PlayerError>;

    /// Queues a program behind the current item. It will play `media`
    /// starting `start_ms` into it.
    fn queue_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError>;

    /// Replaces what is on screen with `media` starting `start_ms` into it,
    /// and drops anything queued.
    fn play_program(&mut self, media: &MediaId, start_ms: u64) -> Result<(), PlayerError>;

    /// Queues the local clip at `path` behind the current item.
    fn queue_clip(&mut self, path: &Path) -> Result<(), PlayerError>;

    /// As [`Player::queue_clip`], but the clip loops as in
    /// [`Player::loop_clip`] once it starts.
    fn queue_looping_clip(&mut self, path: &Path) -> Result<(), PlayerError>;

    /// Moves the item on screen to `position_ms` into the media.
    fn seek(&mut self, position_ms: u64) -> Result<(), PlayerError>;

    /// Stops playing and clears the screen.
    fn stop(&mut self) -> Result<(), PlayerError>;

    /// Draws `overlay` in layer `id`, 0 to 63, replacing what that layer
    /// showed. Higher layers are drawn on top. `MpvPlayer` keeps layer 63
    /// for the banner and refuses it here.
    fn show_overlay(&mut self, id: u8, overlay: &Overlay) -> Result<(), PlayerError>;

    /// Removes the overlay in layer `id`. Clearing an empty layer is not an
    /// error.
    fn clear_overlay(&mut self, id: u8) -> Result<(), PlayerError>;

    /// Shows `banner` over the picture, replacing a banner already shown.
    /// Any command that replaces what is on screen or stops it removes the
    /// banner; queueing does not.
    fn show_banner(&mut self, banner: &Banner) -> Result<(), PlayerError>;

    /// Removes the banner. Clearing when none shows is not an error.
    fn clear_banner(&mut self) -> Result<(), PlayerError>;

    /// The next thing the player reports, oldest first, or `None`.
    fn poll_event(&mut self) -> Option<PlayerEvent>;
}
