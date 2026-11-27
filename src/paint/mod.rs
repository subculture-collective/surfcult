//! Drawing the TV's interface: the Cable Grid design in
//! `docs/design/cable-grid.md`.
//!
//! [`Canvas`] is a bitmap the interface is drawn into: flat and gradient
//! rectangles, bevelled cells, filled paths and text in the two fonts the
//! design uses ([`Font::archivo`] and [`Font::vcr`]), with the hard text
//! shadow. [`Canvas::to_overlay`] hands the result to the player as an
//! [`Overlay`](crate::player::Overlay).
//!
//! [`crt`] is the CRT pass, which the caller runs on that overlay at the
//! viewer's chosen [`Crt`] level before showing it. It only ever touches the
//! interface, never the video.
//!
//! [`banner`] draws the channel banner and [`guide`] the program guide. [`mark`] is the surfcult mark as a path, and [`watermark`] draws the
//! SCTV station bug from it. [`fade`] scales a finished overlay to an
//! opacity.
//!
//! The colours are the design's tokens, in [`palette`].

pub mod banner;
mod canvas;
pub mod crt;
mod font;
pub mod guide;
pub mod mark;
pub mod palette;
pub mod watermark;

pub use canvas::{Canvas, Rgba, Shadow};
pub use crt::Crt;
pub use font::Font;

use crate::player::Overlay;

/// Scales every pixel of `overlay` by `opacity`, from 0 (transparent) to 1
/// (as drawn). The pixels are premultiplied, so colour and alpha scale
/// together.
pub fn fade(overlay: &mut Overlay, opacity: f32) {
    let factor = (opacity.clamp(0.0, 1.0) * 256.0).round() as u32;
    for byte in &mut overlay.bgra {
        *byte = ((u32::from(*byte) * factor + 128) >> 8).min(255) as u8;
    }
}

#[cfg(test)]
mod tests;
