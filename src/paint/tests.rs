use super::crt::{self, Crt};
use super::palette::*;
use super::*;
use crate::player::Overlay;

fn opaque(width: u32, height: u32, bgra: [u8; 4]) -> Overlay {
    Overlay {
        x: 0,
        y: 0,
        width,
        height,
        bgra: bgra.repeat((width * height) as usize),
    }
}

fn at(o: &Overlay, x: u32, y: u32) -> [u8; 4] {
    let i = ((y * o.width + x) * 4) as usize;
    o.bgra[i..i + 4].try_into().unwrap()
}

fn premultiplied(o: &Overlay) -> bool {
    o.bgra
        .chunks_exact(4)
        .all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3])
}

#[test]
fn both_fonts_load_and_measure() {
    for font in [Font::archivo(), Font::vcr()] {
        let one = font.width("A", 40.0, 0.0);
        let two = font.width("AA", 40.0, 0.0);
        assert!(one > 5.0, "{font:?}");
        assert!((two - 2.0 * one).abs() < 0.01, "{font:?}");
        assert!(
            font.width("AA", 40.0, 3.0) - two > 2.9,
            "tracking between letters"
        );
        assert!(font.cap_height(40.0) > 20.0 && font.cap_height(40.0) < 40.0);
    }
}

#[test]
fn the_vcr_font_is_monospaced() {
    let vcr = Font::vcr();
    assert!((vcr.width("iiii", 30.0, 0.0) - vcr.width("WWWW", 30.0, 0.0)).abs() < 0.01);
}

#[test]
fn a_space_has_width_but_no_outline() {
    let font = Font::archivo();
    assert!(font.width(" ", 40.0, 0.0) > 0.0);
    assert!(font.outline(" ", 40.0, 0.0, 0.0, 40.0).is_none());
}

#[test]
fn text_lands_between_the_baseline_and_the_cap_height() {
    let font = Font::archivo();
    let mut canvas = Canvas::new(200, 80);
    let width = canvas.text(&font, "H", 60.0, 0.0, 10.0, 70.0, WHITE, None);
    let cap = font.cap_height(60.0);
    let inked = |x: u32, y: u32| canvas.pixel(x, y).unwrap()[3] > 0;
    // H's left stem, half way up.
    let mid = (70.0 - cap / 2.0) as u32;
    assert!((10..(10 + width as u32)).any(|x| inked(x, mid)));
    assert!(
        !(0..200).any(|x| inked(x, 72)),
        "nothing below the baseline"
    );
    assert!(
        !(0..200).any(|x| inked(x, (70.0 - cap) as u32 - 3)),
        "nothing above the cap height"
    );
}

#[test]
fn a_shadow_is_drawn_under_the_text_and_offset() {
    let font = Font::archivo();
    let mut canvas = Canvas::new(120, 80);
    let shadow = Shadow {
        dx: 6.0,
        dy: 6.0,
        color: SHADOW,
    };
    canvas.text(&font, "I", 60.0, 0.0, 10.0, 60.0, WHITE, Some(shadow));
    // Below the I's stem, where only the shadow reaches.
    let dark = (0..120)
        .filter_map(|x| canvas.pixel(x, 63))
        .any(|p| p[3] == 255 && p[0] < 20);
    assert!(dark);
}

#[test]
fn a_bevel_has_a_light_top_left_and_a_dark_bottom_right() {
    let mut canvas = Canvas::new(40, 40);
    canvas.bevel(0.0, 0.0, 40.0, 40.0, CELL, BEVEL_LIGHT, BEVEL_DARK, 4.0);
    let rgb = |x, y| {
        let p = canvas.pixel(x, y).unwrap();
        Rgba::hex(u32::from(p[0]) << 16 | u32::from(p[1]) << 8 | u32::from(p[2]))
    };
    assert_eq!(rgb(1, 20), BEVEL_LIGHT);
    assert_eq!(rgb(20, 1), BEVEL_LIGHT);
    assert_eq!(rgb(38, 20), BEVEL_DARK);
    assert_eq!(rgb(20, 38), BEVEL_DARK);
    assert_eq!(rgb(20, 20), CELL);
}

#[test]
fn a_gradient_runs_from_top_to_bottom() {
    let mut canvas = Canvas::new(4, 100);
    canvas.fill_gradient(0.0, 0.0, 4.0, 100.0, FIELD_TOP, FIELD_BOTTOM);
    let top = canvas.pixel(1, 0).unwrap();
    let bottom = canvas.pixel(1, 99).unwrap();
    assert!(top[2] > bottom[2], "{top:?} {bottom:?}");
    assert_eq!(top[3], 255);
}

#[test]
fn the_overlay_is_premultiplied_bgra_at_its_position() {
    let mut canvas = Canvas::new(3, 2);
    canvas.fill_rect(0.0, 0.0, 3.0, 2.0, Rgba::hex(0xff0000).with_alpha(128));
    let o = canvas.to_overlay(10, 20);
    assert_eq!((o.x, o.y, o.width, o.height), (10, 20, 3, 2));
    assert_eq!(o.bgra.len(), 3 * 2 * 4);
    assert_eq!(at(&o, 2, 1), [0, 0, 128, 128]);
}

#[test]
fn crt_parses_its_levels() {
    assert_eq!(Crt::parse(" Heavy "), Some(Crt::Heavy));
    assert_eq!(Crt::parse("light"), Some(Crt::Light));
    assert_eq!(Crt::parse("OFF"), Some(Crt::Off));
    assert_eq!(Crt::parse("medium"), None);
    assert_eq!(Crt::default(), Crt::Heavy);
}

#[test]
fn crt_off_leaves_the_overlay_as_drawn() {
    let mut o = opaque(8, 8, [10, 20, 30, 255]);
    let before = o.clone();
    crt::apply(&mut o, Crt::Off);
    assert_eq!(o, before);
}

#[test]
fn crt_keeps_transparent_pixels_transparent() {
    let mut o = opaque(64, 64, [0, 0, 0, 0]);
    crt::apply(&mut o, Crt::Heavy);
    assert!(o.bgra.iter().all(|&b| b == 0));
}

#[test]
fn crt_darkens_two_rows_in_every_four_of_the_screen() {
    // Rows away from the edges, where the glow is even.
    let mut o = opaque(32, 32, [200, 200, 200, 255]);
    crt::apply(&mut o, Crt::Heavy);
    let row = |y| i32::from(at(&o, 16, y)[1]);
    let close = |a: i32, b: i32| (a - b).abs() <= 2;
    assert!(close(row(12), row(13)));
    assert!(row(14) < row(13) - 20);
    assert!(row(15) < row(14) - 20);
    assert!(close(row(16), row(12)));
    assert_eq!(at(&o, 16, 15)[3], 255, "the alpha is not darkened");

    // An overlay one row lower on the screen has its dark rows one row
    // earlier, so it lines up with the screen.
    let mut lower = opaque(32, 32, [200, 200, 200, 255]);
    lower.y = 1;
    crt::apply(&mut lower, Crt::Heavy);
    assert!(close(i32::from(at(&lower, 16, 14)[1]), row(15)));
}

#[test]
fn heavy_is_stronger_than_light() {
    let mut heavy = opaque(32, 16, [200, 200, 200, 255]);
    let mut light = heavy.clone();
    crt::apply(&mut heavy, Crt::Heavy);
    crt::apply(&mut light, Crt::Light);
    assert!(at(&heavy, 16, 3)[1] < at(&light, 16, 3)[1]);
}

#[test]
fn crt_glow_spills_past_an_edge_but_not_far() {
    let mut canvas = Canvas::new(64, 8);
    canvas.fill_rect(0.0, 0.0, 16.0, 8.0, WHITE);
    let mut o = canvas.to_overlay(0, 0);
    crt::apply(&mut o, Crt::Heavy);
    assert!(at(&o, 22, 1)[3] > 0, "glow just past the edge");
    assert_eq!(at(&o, 60, 1)[3], 0, "nothing far away");
    assert!(premultiplied(&o));
}

#[test]
fn crt_moves_red_left_and_blue_right() {
    // A white bar on transparency: after the pass, its left fringe is red
    // and its right fringe is blue.
    let mut canvas = Canvas::new(64, 8);
    canvas.fill_rect(24.0, 0.0, 16.0, 8.0, WHITE);
    let mut o = canvas.to_overlay(0, 0);
    crt::apply(&mut o, Crt::Heavy);
    let left = at(&o, 22, 0);
    let right = at(&o, 41, 0);
    assert!(left[2] > left[0], "red leads on the left: {left:?}");
    assert!(right[0] > right[2], "blue trails on the right: {right:?}");
    assert!(premultiplied(&o));
}

#[test]
fn crt_leaves_a_mismatched_buffer_alone() {
    let mut o = opaque(4, 4, [1, 2, 3, 255]);
    o.bgra.pop();
    let before = o.clone();
    crt::apply(&mut o, Crt::Heavy);
    assert_eq!(o, before);
}

/// Writes sample drawings as PNG files to `SURFCULT_PAINT_PREVIEW`, to look
/// at by eye. Does nothing when that is not set.
#[test]
fn preview() {
    let Some(dir) = std::env::var_os("SURFCULT_PAINT_PREVIEW") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir).unwrap();
    let archivo = Font::archivo();
    let vcr = Font::vcr();
    let shadow = Some(Shadow {
        dx: 3.0,
        dy: 3.0,
        color: SHADOW,
    });
    let mut c = Canvas::new(960, 300);
    c.fill_gradient(0.0, 0.0, 960.0, 300.0, FIELD_TOP, FIELD_BOTTOM);
    c.bevel(20.0, 20.0, 180.0, 80.0, CELL, BEVEL_LIGHT, BEVEL_DARK, 4.0);
    c.text(&vcr, "9:16:20", 34.0, 0.0, 40.0, 72.0, WHITE, shadow);
    c.bevel(
        210.0,
        20.0,
        360.0,
        80.0,
        SERIES,
        BEVEL_LIGHT,
        BEVEL_DARK,
        4.0,
    );
    c.text(
        &archivo,
        "The Inkwell Hour",
        40.0,
        0.0,
        226.0,
        72.0,
        WHITE,
        shadow,
    );
    c.bevel(
        580.0,
        20.0,
        360.0,
        80.0,
        FOCUS,
        BEVEL_LIGHT,
        BEVEL_DARK,
        4.0,
    );
    c.outline(580.0, 20.0, 360.0, 80.0, YELLOW, 7.0);
    c.text(
        &archivo,
        "The Long Coast (1987)",
        40.0,
        0.0,
        596.0,
        72.0,
        WHITE,
        shadow,
    );
    c.fill_gradient(20.0, 130.0, 920.0, 56.0, OCHRE_TOP, OCHRE_BOTTOM);
    c.text(
        &archivo,
        "CHANNEL SETUP",
        36.0,
        0.0,
        40.0,
        172.0,
        OCHRE_INK,
        None,
    );
    c.text(
        &archivo,
        "04 MATINEE",
        64.0,
        1.0,
        40.0,
        270.0,
        YELLOW,
        shadow,
    );
    std::fs::write(dir.join("paint-flat.png"), c.png()).unwrap();
    for (name, level) in [("heavy", Crt::Heavy), ("light", Crt::Light)] {
        let mut o = c.to_overlay(0, 0);
        crt::apply(&mut o, level);
        let mut rgba = o.bgra.clone();
        rgba.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
        let size = tiny_skia::IntSize::from_wh(o.width, o.height).unwrap();
        let pixmap = tiny_skia::Pixmap::from_vec(rgba, size).unwrap();
        std::fs::write(
            dir.join(format!("paint-{name}.png")),
            pixmap.encode_png().unwrap(),
        )
        .unwrap();
    }
}
