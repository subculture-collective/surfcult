//! The program guide in the Cable Grid design
//! (`docs/design/cable-grid.md#tv-screens`), drawn from a [`GuideView`]
//! the run loop builds.
//!
//! The top-left quarter is the info panel for the focused program; the
//! top-right quarter is left clear, where the player draws the video
//! ([`VideoArea::TopRight`](crate::player::VideoArea::TopRight)). The bottom
//! half is the grid: a header of three half-hour slots, a date row, and
//! [`ROWS`] channel rows. The clock cell at the head of the grid is a
//! separate overlay ([`draw_clock`]), so the clock can tick without
//! redrawing the whole guide.

use tiny_skia::PathBuilder;

use super::palette::*;
use super::{Canvas, Font, Rgba, Shadow};
use crate::player::Overlay;

/// The number of channel rows on screen.
pub const ROWS: usize = 4;

const SCREEN: (f32, f32) = (1920.0, 1080.0);
/// The grid's left edge and width, inside the 5% margin a TV may cut off.
const GRID_X: f32 = 96.0;
const GRID_W: f32 = 1728.0;
const CHANNEL_W: f32 = 280.0;
const GAP: f32 = 6.0;
const HEADER_Y: f32 = 556.0;
const HEADER_H: f32 = 52.0;
const DATE_Y: f32 = 614.0;
const DATE_H: f32 = 40.0;
const ROWS_Y: f32 = 660.0;
const ROW_H: f32 = 86.0;
const EDGE: f32 = 3.0;
const MARGIN: u32 = 16;

/// What one program cell shows.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideCell {
    pub title: String,
    /// Where the cell starts and ends across the three slots, 0 to 1.
    pub left: f32,
    pub right: f32,
    /// The program started before the grid's first slot.
    pub continues_before: bool,
    /// The program ends after the grid's last slot.
    pub continues_after: bool,
    pub movie: bool,
    pub focused: bool,
}

/// One channel row.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideRow {
    pub number: u32,
    pub name: String,
    pub cells: Vec<GuideCell>,
}

/// Everything the guide shows, already worked out.
#[derive(Debug, Clone, PartialEq)]
pub struct GuideView {
    /// The info panel's lines, top to bottom: the channel, the program, the
    /// day and the channel number. The second is set in yellow.
    pub info: [String; 4],
    /// The three slots' start times.
    pub slots: [String; 3],
    /// The date row's text.
    pub date: String,
    /// The rows on screen, at most [`ROWS`].
    pub rows: Vec<GuideRow>,
}

fn shadow() -> Option<Shadow> {
    Some(Shadow {
        dx: 3.0,
        dy: 3.0,
        color: SHADOW,
    })
}

fn bevel(c: &mut Canvas, x: f32, y: f32, w: f32, h: f32, fill: Rgba) {
    c.bevel(x, y, w, h, fill, BEVEL_LIGHT, BEVEL_DARK, EDGE);
}

#[allow(clippy::too_many_arguments)]
fn centred(
    c: &mut Canvas,
    font: &Font,
    text: &str,
    px: f32,
    x: f32,
    w: f32,
    baseline: f32,
    color: Rgba,
) {
    let text = font.fit(text, px, 0.0, w - 16.0);
    let tw = font.width(&text, px, 0.0);
    c.text(
        font,
        &text,
        px,
        0.0,
        x + (w - tw) / 2.0,
        baseline,
        color,
        shadow(),
    );
}

/// The guide as a full-screen overlay, before the CRT pass, with the
/// top-right quarter clear for the video and the clock cell left to
/// [`draw_clock`].
pub fn draw(view: &GuideView) -> Overlay {
    let font = Font::archivo();
    let (sw, sh) = SCREEN;
    let mut c = Canvas::new(sw as u32, sh as u32);

    // The info panel in the top-left quarter.
    c.fill_gradient(0.0, 0.0, sw / 2.0, sh / 2.0, FIELD_TOP, FIELD_BOTTOM);
    for (i, line) in view.info.iter().enumerate() {
        let color = if i == 1 { YELLOW } else { WHITE };
        centred(
            &mut c,
            &font,
            line,
            52.0,
            GRID_X,
            sw / 2.0 - GRID_X,
            170.0 + 80.0 * i as f32,
            color,
        );
    }

    // The bottom half behind the grid.
    c.fill_gradient(0.0, sh / 2.0, sw, sh / 2.0, FIELD_BOTTOM, BEVEL_DARK);

    // The header: the clock's place, then the three slots.
    let slots_x = GRID_X + CHANNEL_W + GAP;
    let slots_w = GRID_W - CHANNEL_W - GAP;
    let slot_w = (slots_w - 2.0 * GAP) / 3.0;
    for (i, slot) in view.slots.iter().enumerate() {
        let x = slots_x + i as f32 * (slot_w + GAP);
        bevel(&mut c, x, HEADER_Y, slot_w, HEADER_H, CELL);
        centred(
            &mut c,
            &font,
            slot,
            34.0,
            x,
            slot_w,
            HEADER_Y + 39.0,
            YELLOW,
        );
    }
    bevel(&mut c, GRID_X, DATE_Y, CHANNEL_W, DATE_H, CELL);
    bevel(&mut c, slots_x, DATE_Y, slots_w, DATE_H, CELL);
    centred(
        &mut c,
        &font,
        &view.date,
        28.0,
        slots_x,
        slots_w,
        DATE_Y + 31.0,
        YELLOW,
    );

    for (r, row) in view.rows.iter().take(ROWS).enumerate() {
        let y = ROWS_Y + r as f32 * (ROW_H + GAP);
        bevel(&mut c, GRID_X, y, CHANNEL_W, ROW_H, CELL);
        centred(
            &mut c,
            &font,
            &format!("{:02}", row.number),
            32.0,
            GRID_X,
            CHANNEL_W,
            y + 38.0,
            YELLOW,
        );
        centred(
            &mut c,
            &font,
            &row.name,
            26.0,
            GRID_X,
            CHANNEL_W,
            y + 72.0,
            YELLOW,
        );
        for cell in &row.cells {
            let x = slots_x + cell.left.clamp(0.0, 1.0) * slots_w;
            let w = (cell.right.clamp(0.0, 1.0) - cell.left.clamp(0.0, 1.0)) * slots_w - GAP;
            if w < 4.0 {
                continue;
            }
            let fill = match (cell.focused, cell.movie) {
                (true, _) => FOCUS,
                (false, true) => MOVIE,
                (false, false) => SERIES,
            };
            bevel(&mut c, x, y, w, ROW_H, fill);
            let left = if cell.continues_before { 30.0 } else { 14.0 };
            let right = if cell.continues_after { 34.0 } else { 14.0 };
            if cell.continues_before {
                arrow(&mut c, x + 8.0, y + ROW_H / 2.0, -1.0);
            }
            if cell.continues_after {
                arrow(&mut c, x + w - 8.0, y + ROW_H / 2.0, 1.0);
            }
            let room = w - left - right;
            let title = font.fit(&cell.title, 34.0, 0.0, room.max(0.0));
            // A sliver of a cell gets no title rather than a lone ellipsis.
            if title.chars().count() > 3 {
                c.text(
                    &font,
                    &title,
                    34.0,
                    0.0,
                    x + left,
                    y + 55.0,
                    WHITE,
                    shadow(),
                );
            }
            if cell.focused {
                c.outline(x, y, w, ROW_H, YELLOW, 6.0);
            }
        }
    }

    // The video's quarter is cleared last, so nothing drawn above spills
    // into it.
    let mut o = c.to_overlay(0, 0);
    let (qw, qh) = ((sw / 2.0) as usize, (sh / 2.0) as usize);
    for y in 0..qh {
        let start = (y * sw as usize + qw) * 4;
        o.bgra[start..start + qw * 4].fill(0);
    }
    o
}

/// A continuation arrow at `x`, `y`, pointing left (`dir` -1) or right (1).
fn arrow(c: &mut Canvas, x: f32, y: f32, dir: f32) {
    let mut pb = PathBuilder::new();
    pb.move_to(x, y - 20.0);
    pb.line_to(x + 14.0 * dir, y);
    pb.line_to(x, y + 20.0);
    pb.line_to(x + 4.0 * dir, y);
    pb.close();
    if let Some(path) = pb.finish() {
        c.fill_path(&path, WHITE, tiny_skia::Transform::identity());
    }
}

/// The clock cell at the head of the grid, showing `clock`, as its own
/// small overlay.
pub fn draw_clock(clock: &str) -> Overlay {
    let vcr = Font::vcr();
    let m = MARGIN as f32;
    let mut c = Canvas::new(
        CHANNEL_W as u32 + 2 * MARGIN + 3,
        HEADER_H as u32 + 2 * MARGIN + 3,
    );
    bevel(&mut c, m, m, CHANNEL_W, HEADER_H, CELL);
    let px = 32.0;
    let w = vcr.width(clock, px, 0.0);
    let baseline = m + (HEADER_H + vcr.cap_height(px)) / 2.0;
    c.text(
        &vcr,
        clock,
        px,
        0.0,
        m + (CHANNEL_W - w) / 2.0,
        baseline,
        WHITE,
        shadow(),
    );
    c.to_overlay(
        GRID_X as i32 - MARGIN as i32,
        HEADER_Y as i32 - MARGIN as i32,
    )
}
