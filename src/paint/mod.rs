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
//! The colours are the design's tokens, in [`palette`].

mod canvas;
pub mod crt;
mod font;
pub mod palette;

pub use canvas::{Canvas, Rgba, Shadow};
pub use crt::Crt;
pub use font::Font;

#[cfg(test)]
mod tests;
