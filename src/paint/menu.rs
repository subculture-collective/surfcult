//! A TV settings menu in the Cable Grid design
//! (`docs/design/cable-grid.md#tv-screens`): a full-screen violet field,
//! the mark and an ochre heading band, a list panel of settings with the
//! focused row in the focus colour and yellow, a line describing the focused
//! setting, and the remote's keys along the bottom.

use tiny_skia::{PathBuilder, Transform};

use super::palette::*;
use super::{Canvas, Font, Shadow, mark};
use crate::player::Overlay;

/// What the menu shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuView {
    /// The heading, such as `PICTURE`.
    pub title: String,
    /// Each setting's name and value.
    pub rows: Vec<(String, String)>,
    /// The focused row.
    pub focus: usize,
    /// What the focused setting does, under the list.
    pub help: String,
    /// The keys, along the bottom.
    pub keys: String,
}

const LEFT: f32 = 96.0;
const RIGHT: f32 = 1824.0;
const HEAD_Y: f32 = 80.0;
const HEAD_H: f32 = 76.0;
const LIST_Y: f32 = 200.0;
const ROW_H: f32 = 76.0;
const PAD: f32 = 16.0;

fn shadow() -> Option<Shadow> {
    Some(Shadow {
        dx: 3.0,
        dy: 3.0,
        color: SHADOW,
    })
}

/// The menu as a full-screen overlay, before the CRT pass.
pub fn draw(view: &MenuView) -> Overlay {
    let font = Font::archivo();
    let vcr = Font::vcr();
    let mut c = Canvas::new(1920, 1080);
    c.fill_gradient(0.0, 0.0, 1920.0, 1080.0, FIELD_TOP, FIELD_BOTTOM);

    // The mark, then the heading band with a slanted right end.
    let scale = HEAD_H / mark::HEIGHT;
    let place = |dx: f32, dy: f32| {
        Transform::from_translate(-mark::LEFT, -mark::TOP)
            .post_scale(scale, scale)
            .post_translate(LEFT + dx, HEAD_Y + dy)
    };
    let shapes = mark::shapes();
    c.fill_path(&shapes, SHADOW, place(3.0, 3.0));
    c.fill_path(&shapes, YELLOW, place(0.0, 0.0));
    let band_x = LEFT + mark::WIDTH * scale + 24.0;
    let mut band = PathBuilder::new();
    band.move_to(band_x, HEAD_Y);
    band.line_to(RIGHT, HEAD_Y);
    band.line_to(RIGHT - 30.0, HEAD_Y + HEAD_H);
    band.line_to(band_x, HEAD_Y + HEAD_H);
    band.close();
    if let Some(band) = band.finish() {
        c.fill_path_gradient(&band, HEAD_Y, HEAD_H, OCHRE_TOP, OCHRE_BOTTOM);
    }
    c.text(
        &font,
        &view.title,
        48.0,
        1.0,
        band_x + 28.0,
        HEAD_Y + 55.0,
        OCHRE_INK,
        None,
    );

    // The list.
    let list_h = view.rows.len() as f32 * ROW_H + 2.0 * PAD;
    c.bevel(
        LEFT,
        LIST_Y,
        RIGHT - LEFT,
        list_h,
        CELL,
        BEVEL_LIGHT,
        BEVEL_DARK,
        4.0,
    );
    for (i, (name, value)) in view.rows.iter().enumerate() {
        let y = LIST_Y + PAD + i as f32 * ROW_H;
        let focused = i == view.focus;
        if focused {
            c.bevel(
                LEFT + PAD,
                y,
                RIGHT - LEFT - 2.0 * PAD,
                ROW_H,
                FOCUS,
                BEVEL_LIGHT,
                BEVEL_DARK,
                3.0,
            );
        }
        let color = if focused { YELLOW } else { WHITE };
        c.text(
            &font,
            name,
            44.0,
            0.0,
            LEFT + 2.0 * PAD + 16.0,
            y + 54.0,
            color,
            shadow(),
        );
        let w = font.width(value, 44.0, 1.0);
        c.text(
            &font,
            value,
            44.0,
            1.0,
            RIGHT - 2.0 * PAD - 16.0 - w,
            y + 54.0,
            color,
            shadow(),
        );
    }

    // What the focused setting does.
    let help = font.fit(&view.help, 36.0, 0.0, RIGHT - LEFT);
    c.text(
        &font,
        &help,
        36.0,
        0.0,
        LEFT,
        LIST_Y + list_h + 70.0,
        WHITE,
        shadow(),
    );

    // The keys along the bottom, under a rule.
    c.fill_rect(LEFT, 960.0, RIGHT - LEFT, 4.0, BEVEL_LIGHT);
    let w = vcr.width(&view.keys, 32.0, 0.0);
    c.text(
        &vcr,
        &view.keys,
        32.0,
        0.0,
        RIGHT - w,
        1012.0,
        WHITE,
        shadow(),
    );
    c.to_overlay(0, 0)
}
