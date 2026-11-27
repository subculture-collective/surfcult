//! The channel banner in the Cable Grid design
//! (`docs/design/cable-grid.md#tv-screens`): a band across the bottom of a
//! 1920x1080 screen, inside the 5% margin a TV may cut off.
//!
//! From left to right: a channel cell with the number large and the name
//! under it; a violet panel with an ochre strip carrying the times, the
//! title under it and the next program under that; and a clock cell. Text
//! that does not fit is cut with an ellipsis.

use super::palette::*;
use super::{Canvas, Font, Shadow};
use crate::overlay::Banner;
use crate::player::Overlay;
use tiny_skia::PathBuilder;

/// Transparent space around the band for the CRT pass's glow, in pixels.
pub const MARGIN: u32 = 16;
/// The band's left edge on the screen; its right edge is as far from the
/// right.
pub const LEFT: u32 = 96;
/// The gap between the band's bottom and the screen's.
pub const BOTTOM: u32 = 86;
/// The band's height.
pub const HEIGHT: u32 = 176;

const SCREEN: (u32, u32) = (1920, 1080);
const GAP: f32 = 8.0;
const EDGE: f32 = 4.0;
const CHANNEL_WIDTH: f32 = 276.0;
const STRIP_HEIGHT: f32 = 56.0;
const PAD: f32 = 22.0;
const SHADOW_OFFSET: f32 = 3.0;

fn shadow() -> Option<Shadow> {
    Some(Shadow {
        dx: SHADOW_OFFSET,
        dy: SHADOW_OFFSET,
        color: SHADOW,
    })
}

/// The banner as an overlay at its place on the screen, before the CRT
/// pass.
pub fn draw(banner: &Banner) -> Overlay {
    let archivo = Font::archivo();
    let vcr = Font::vcr();
    let width = (SCREEN.0 - 2 * LEFT) as f32;
    let height = HEIGHT as f32;
    let m = MARGIN as f32;
    let extra = SHADOW_OFFSET.ceil() as u32;
    let mut c = Canvas::new(
        SCREEN.0 - 2 * LEFT + 2 * MARGIN + extra,
        HEIGHT + 2 * MARGIN + extra,
    );
    let (x0, y0) = (m, m);

    // The channel cell.
    c.bevel(
        x0,
        y0,
        CHANNEL_WIDTH,
        height,
        CELL,
        BEVEL_LIGHT,
        BEVEL_DARK,
        EDGE,
    );
    let number = format!("{:02}", banner.number);
    let inner = CHANNEL_WIDTH - 2.0 * PAD;
    let number = archivo.fit(&number, 108.0, 0.0, inner);
    let w = archivo.width(&number, 108.0, 0.0);
    c.text(
        &archivo,
        &number,
        108.0,
        0.0,
        x0 + (CHANNEL_WIDTH - w) / 2.0,
        y0 + 112.0,
        YELLOW,
        shadow(),
    );
    let name = archivo.fit(&banner.name, 34.0, 0.5, inner);
    let w = archivo.width(&name, 34.0, 0.5);
    c.text(
        &archivo,
        &name,
        34.0,
        0.5,
        x0 + (CHANNEL_WIDTH - w) / 2.0,
        y0 + 156.0,
        YELLOW,
        shadow(),
    );

    // The clock cell, as wide as its text needs.
    let clock_px = 46.0;
    let clock_width = vcr
        .width(&banner.clock, clock_px, 0.0)
        .max(vcr.width("00:00", clock_px, 0.0))
        + 2.0 * 28.0;
    let clock_x = x0 + width - clock_width;
    c.bevel(
        clock_x,
        y0,
        clock_width,
        height,
        CELL,
        BEVEL_LIGHT,
        BEVEL_DARK,
        EDGE,
    );
    let w = vcr.width(&banner.clock, clock_px, 0.0);
    let baseline = y0 + (height + vcr.cap_height(clock_px)) / 2.0;
    c.text(
        &vcr,
        &banner.clock,
        clock_px,
        0.0,
        clock_x + (clock_width - w) / 2.0,
        baseline,
        WHITE,
        shadow(),
    );

    // The main panel: the field, the ochre strip with the times, the title.
    let mx = x0 + CHANNEL_WIDTH + GAP;
    let mw = clock_x - GAP - mx;
    c.bevel(mx, y0, mw, height, FIELD_TOP, BEVEL_LIGHT, BEVEL_DARK, EDGE);
    c.fill_gradient(
        mx + EDGE,
        y0 + EDGE,
        mw - 2.0 * EDGE,
        height - 2.0 * EDGE,
        FIELD_TOP,
        FIELD_BOTTOM,
    );
    let mut strip = PathBuilder::new();
    let (sx, sy, sw) = (mx + EDGE, y0 + EDGE, mw - 2.0 * EDGE);
    strip.move_to(sx, sy);
    strip.line_to(sx + sw, sy);
    strip.line_to(sx + sw - 24.0, sy + STRIP_HEIGHT);
    strip.line_to(sx, sy + STRIP_HEIGHT);
    strip.close();
    if let Some(strip) = strip.finish() {
        c.fill_path_gradient(&strip, sy, STRIP_HEIGHT, OCHRE_TOP, OCHRE_BOTTOM);
    }
    let times = archivo.fit(&banner.times, 32.0, 0.5, sw - 2.0 * PAD - 24.0);
    c.text(
        &archivo,
        &times,
        32.0,
        0.5,
        sx + PAD,
        sy + 40.0,
        OCHRE_INK,
        None,
    );
    // With a NEXT line the title is smaller and higher, to make room.
    let (title_px, title_base) = if banner.next.is_empty() {
        (64.0, y0 + STRIP_HEIGHT + 86.0)
    } else {
        (54.0, y0 + STRIP_HEIGHT + 60.0)
    };
    let title = archivo.fit(&banner.title, title_px, 0.0, mw - 2.0 * PAD);
    let base = title_base;
    c.text(
        &archivo,
        &title,
        title_px,
        0.0,
        mx + PAD,
        base,
        WHITE,
        shadow(),
    );
    if !banner.next.is_empty() {
        let next = archivo.fit(&banner.next, 30.0, 0.5, mw - 2.0 * PAD);
        c.text(
            &archivo,
            &next,
            30.0,
            0.5,
            mx + PAD,
            y0 + height - 16.0,
            YELLOW,
            shadow(),
        );
    }

    let x = LEFT as i32 - MARGIN as i32;
    let y = (SCREEN.1 - BOTTOM - HEIGHT) as i32 - MARGIN as i32;
    c.to_overlay(x, y)
}
