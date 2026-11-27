//! Where the channel banner goes in MPV.
//!
//! The banner is a bitmap drawn by [`paint::banner`](crate::paint::banner)
//! in the Cable Grid design and given the CRT pass at the player's level. It
//! is shown with `overlay-add` in layer [`BANNER_LAYER`], which the player
//! keeps for itself: [`Player::show_overlay`](crate::player::Player::show_overlay)
//! refuses that layer. MPV draws overlays after its shaders, so the display
//! filters never touch the banner.

/// The `overlay-add` layer the banner is drawn in; the topmost.
pub(super) const BANNER_LAYER: u8 = 63;
