//! The SCTV watermark: the station bug in the top-right corner of the
//! screen (`docs/design/cable-grid.md#the-sctv-watermark`).
//!
//! [`draw`] returns the bug at full opacity, white with its shadow, on a
//! transparent margin wide enough for the CRT pass's glow. The caller runs
//! the CRT pass on it once and then scales it to the opacity it needs with
//! [`fade`](super::fade).

use tiny_skia::Transform;

use super::{Canvas, Font, Rgba, mark};
use crate::player::Overlay;

/// The two forms of the bug.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Form {
    /// The mark with SCTV beside it: shown on landing on a channel.
    Beside,
    /// The mark with SCTV under it, smaller and widely spaced: shown
    /// through commercials.
    Under,
}

/// Transparent space around the bug for the CRT pass's glow, in pixels.
pub const MARGIN: u32 = 16;
/// The screen size the position is worked out for.
pub const SCREEN: (u32, u32) = (1920, 1080);
/// The gap between the bug's right edge and the screen's.
pub const RIGHT: u32 = 96;
/// The gap between the bug's top edge and the screen's.
pub const TOP: u32 = 86;
/// The opacity the bug is shown at.
pub const OPACITY: f32 = 0.6;

const SHADOW_OFFSET: f32 = 3.0;
const WHITE: Rgba = Rgba::hex(0xffffff);
const SHADOW: Rgba = Rgba::hex(0x000000).with_alpha(128);

/// Sizes in pixels: the mark's height, the gap to the text, and the text's
/// capital height and tracking.
struct Layout {
    mark_height: f32,
    gap: f32,
    cap: f32,
    tracking_em: f32,
}

fn layout(form: Form) -> Layout {
    match form {
        // 72 px tall in all: the mark, and SCTV beside it at 24 of its 42
        // grid units.
        Form::Beside => Layout {
            mark_height: 72.0,
            gap: 72.0 * 9.0 / mark::HEIGHT,
            cap: 72.0 * 24.0 / mark::HEIGHT,
            tracking_em: 0.04,
        },
        // 100 px tall in all: the mark, 6 units of gap and SCTV 11 units
        // tall, on 60 units.
        Form::Under => Layout {
            mark_height: 100.0 * mark::HEIGHT / 60.0,
            gap: 100.0 * 6.0 / 60.0,
            cap: 100.0 * 11.0 / 60.0,
            tracking_em: 0.14,
        },
    }
}

/// The bug at full opacity, as an overlay at its place on the screen.
pub fn draw(form: Form) -> Overlay {
    let font = Font::archivo();
    let l = layout(form);
    let scale = l.mark_height / mark::HEIGHT;
    let mark_width = mark::WIDTH * scale;
    let px = l.cap / font.cap_height(1.0);
    let tracking = l.tracking_em * px;
    let text_width = font.width("SCTV", px, tracking);
    let (width, height) = match form {
        Form::Beside => (mark_width + l.gap + text_width, l.mark_height),
        Form::Under => (mark_width.max(text_width), l.mark_height + l.gap + l.cap),
    };
    let m = MARGIN as f32;
    let mut canvas = Canvas::new(
        (width + SHADOW_OFFSET).ceil() as u32 + 2 * MARGIN,
        (height + SHADOW_OFFSET).ceil() as u32 + 2 * MARGIN,
    );

    let mark_x = match form {
        Form::Beside => m,
        Form::Under => m + (width - mark_width) / 2.0,
    };
    let shapes = mark::shapes();
    let place = |dx: f32, dy: f32| {
        Transform::from_translate(-mark::LEFT, -mark::TOP)
            .post_scale(scale, scale)
            .post_translate(mark_x + dx, m + dy)
    };
    canvas.fill_path(&shapes, SHADOW, place(SHADOW_OFFSET, SHADOW_OFFSET));
    canvas.fill_path(&shapes, WHITE, place(0.0, 0.0));

    let (text_x, baseline) = match form {
        Form::Beside => (m + mark_width + l.gap, m + (l.mark_height + l.cap) / 2.0),
        Form::Under => (
            m + (width - text_width) / 2.0,
            m + l.mark_height + l.gap + l.cap,
        ),
    };
    let shadow = super::Shadow {
        dx: SHADOW_OFFSET,
        dy: SHADOW_OFFSET,
        color: SHADOW,
    };
    canvas.text(
        &font,
        "SCTV",
        px,
        tracking,
        text_x,
        baseline,
        WHITE,
        Some(shadow),
    );

    // The bug's own right edge and top, not the margin's, sit at RIGHT and
    // TOP from the screen's edges.
    let x = SCREEN.0 as i32 - RIGHT as i32 - (width + SHADOW_OFFSET).ceil() as i32 - MARGIN as i32;
    let y = TOP as i32 - MARGIN as i32;
    canvas.to_overlay(x, y)
}
