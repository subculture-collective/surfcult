//! When the SCTV watermark shows, in which form, and how opaque.
//!
//! - Landing on a channel shows the beside form for [`HOLD_MS`], then fades
//!   it out over [`FADE_MS`].
//! - A break or filler on screen shows the under form for as long as it
//!   lasts. When it ends, the under form fades out over [`FADE_MS`].
//! - A break replaces a landing's hold and fade.
//!
//! Opacity moves in eighths, so a fade is at most eight redraws of the
//! watermark's small bitmap.

use crate::paint::watermark::Form;

/// How long the watermark holds after landing on a channel.
pub const HOLD_MS: i64 = 3_000;
/// How long the watermark takes to fade out.
pub const FADE_MS: i64 = 1_000;
/// Opacity steps in a fade.
const STEPS: f32 = 8.0;

#[derive(Debug, Default)]
pub(super) struct Watermark {
    landed_ms: Option<i64>,
    break_on: bool,
    break_ended_ms: Option<i64>,
}

impl Watermark {
    /// A channel change settled at `now_ms`.
    pub(super) fn landed(&mut self, now_ms: i64) {
        self.landed_ms = Some(now_ms);
        self.break_ended_ms = None;
    }

    /// The channel is being changed, or the screen was lost: show nothing
    /// until the next landing.
    pub(super) fn leave(&mut self) {
        *self = Self::default();
    }

    /// The form and opacity (0 to 1, in eighths) at `now_ms`, or `None` when
    /// the watermark is hidden. `break_on` says whether a break or filler is
    /// on screen now.
    pub(super) fn at(&mut self, now_ms: i64, break_on: bool) -> Option<(Form, f32)> {
        if self.break_on && !break_on {
            self.break_ended_ms = Some(now_ms);
        }
        self.break_on = break_on;
        if break_on {
            // The break replaces the landing's hold and fade.
            self.landed_ms = None;
            return Some((Form::Under, 1.0));
        }
        if let Some(opacity) = self.break_ended_ms.and_then(|t| fading(now_ms - t)) {
            return Some((Form::Under, opacity));
        }
        let since = now_ms - self.landed_ms?;
        if (0..HOLD_MS).contains(&since) {
            return Some((Form::Beside, 1.0));
        }
        fading(since - HOLD_MS).map(|opacity| (Form::Beside, opacity))
    }
}

/// The opacity `elapsed_ms` into a fade, rounded up to an eighth, or `None`
/// once the fade is over.
fn fading(elapsed_ms: i64) -> Option<f32> {
    if !(0..FADE_MS).contains(&elapsed_ms) {
        return None;
    }
    let left = 1.0 - elapsed_ms as f32 / FADE_MS as f32;
    Some((left * STEPS).ceil() / STEPS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn landing_holds_the_beside_form_then_fades_it_in_eighths() {
        let mut w = Watermark::default();
        assert_eq!(w.at(0, false), None);
        w.landed(1_000);
        assert_eq!(w.at(1_000, false), Some((Form::Beside, 1.0)));
        assert_eq!(w.at(3_999, false), Some((Form::Beside, 1.0)));
        assert_eq!(w.at(4_000, false), Some((Form::Beside, 1.0)));
        assert_eq!(w.at(4_125, false), Some((Form::Beside, 0.875)));
        assert_eq!(w.at(4_999, false), Some((Form::Beside, 0.125)));
        assert_eq!(w.at(5_000, false), None);
    }

    #[test]
    fn a_break_shows_the_under_form_until_it_ends_then_fades() {
        let mut w = Watermark::default();
        w.landed(0);
        assert_eq!(w.at(60_000, true), Some((Form::Under, 1.0)));
        assert_eq!(w.at(150_000, true), Some((Form::Under, 1.0)));
        assert_eq!(w.at(180_000, false), Some((Form::Under, 1.0)));
        assert_eq!(w.at(180_500, false), Some((Form::Under, 0.5)));
        assert_eq!(w.at(181_000, false), None);
    }

    #[test]
    fn a_break_during_the_hold_takes_over() {
        let mut w = Watermark::default();
        w.landed(0);
        assert_eq!(w.at(1_000, true), Some((Form::Under, 1.0)));
        assert_eq!(w.at(2_000, false), Some((Form::Under, 1.0)));
        assert_eq!(w.at(3_000, false), None);
    }

    #[test]
    fn leaving_hides_it_until_the_next_landing() {
        let mut w = Watermark::default();
        w.landed(0);
        w.leave();
        assert_eq!(w.at(500, false), None);
        w.landed(10_000);
        assert_eq!(w.at(10_000, false), Some((Form::Beside, 1.0)));
    }
}
