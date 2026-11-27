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
        .as_chunks::<4>()
        .0
        .iter()
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
        rgba.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|p| p.swap(0, 2));
        let size = tiny_skia::IntSize::from_wh(o.width, o.height).unwrap();
        let pixmap = tiny_skia::Pixmap::from_vec(rgba, size).unwrap();
        std::fs::write(
            dir.join(format!("paint-{name}.png")),
            pixmap.encode_png().unwrap(),
        )
        .unwrap();
    }
}

#[test]
fn the_mark_fills_its_grid_box() {
    let b = mark::shapes().bounds();
    assert_eq!((b.left(), b.top()), (mark::LEFT, mark::TOP));
    assert_eq!((b.width(), b.height()), (mark::WIDTH, mark::HEIGHT));
}

#[test]
fn the_watermark_sits_96_from_the_right_and_86_from_the_top() {
    use super::watermark::{Form, MARGIN, draw};
    for (form, height) in [(Form::Beside, 72), (Form::Under, 100)] {
        let o = draw(form);
        let m = MARGIN as i32;
        // The bitmap is the bug, its 3-pixel shadow and the glow margin.
        let bug = o.height as i32 - 3 - 2 * m;
        assert!((bug - height).abs() <= 2, "{form:?}: {bug}");
        assert_eq!(o.y + m, 86);
        assert_eq!(o.x + o.width as i32 - m, 1920 - 96);
        assert!(premultiplied(&o));
        // The margin is left clear for the glow.
        let top = (o.width * MARGIN * 4) as usize;
        assert!(o.bgra[..top].iter().all(|&b| b == 0));
    }
    assert!(draw(Form::Beside).width > draw(Form::Under).width);
}

#[test]
fn fade_scales_colour_and_alpha_together() {
    let mut o = opaque(2, 1, [200, 100, 50, 255]);
    fade(&mut o, 0.5);
    assert_eq!(at(&o, 0, 0), [100, 50, 25, 128]);
    fade(&mut o, 0.0);
    assert!(o.bgra.iter().all(|&b| b == 0));
}

/// Writes both watermark forms over a sample picture to
/// `SURFCULT_PAINT_PREVIEW`, at the opacity they are shown at.
#[test]
fn preview_watermark() {
    use super::watermark::{Form, OPACITY, draw};
    let Some(dir) = std::env::var_os("SURFCULT_PAINT_PREVIEW") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for (name, form) in [("beside", Form::Beside), ("under", Form::Under)] {
        for (level_name, level) in [("heavy", Crt::Heavy), ("off", Crt::Off)] {
            let mut bug = draw(form);
            crt::apply(&mut bug, level);
            fade(&mut bug, OPACITY);
            // A sunset-coloured picture behind the corner of the screen.
            let (w, h) = (bug.width + 80, bug.height + 40);
            let mut scene = Canvas::new(w, h);
            let (top, bottom) = (Rgba::hex(0xff9a5a), Rgba::hex(0x1f5f8f));
            scene.fill_gradient(0.0, 0.0, w as f32, h as f32, top, bottom);
            let mut rgba = scene.to_overlay(0, 0).bgra;
            for y in 0..bug.height {
                for x in 0..bug.width {
                    let s = ((y * bug.width + x) * 4) as usize;
                    let d = (((y + 20) * w + x + 40) * 4) as usize;
                    let a = u32::from(bug.bgra[s + 3]);
                    for c in 0..3 {
                        let under = u32::from(rgba[d + c]) * (255 - a) / 255;
                        rgba[d + c] = (u32::from(bug.bgra[s + c]) + under) as u8;
                    }
                }
            }
            rgba.as_chunks_mut::<4>()
                .0
                .iter_mut()
                .for_each(|p| p.swap(0, 2));
            let size = tiny_skia::IntSize::from_wh(w, h).unwrap();
            let png = tiny_skia::Pixmap::from_vec(rgba, size).unwrap();
            let file = dir.join(format!("watermark-{name}-{level_name}.png"));
            std::fs::write(file, png.encode_png().unwrap()).unwrap();
        }
    }
}

fn sample_banner(title: &str) -> crate::overlay::Banner {
    crate::overlay::Banner {
        number: 4,
        name: "MATINEE".into(),
        title: title.into(),
        times: "20:30–22:25".into(),
        clock: "21:16".into(),
    }
}

#[test]
fn the_banner_sits_across_the_bottom_inside_the_safe_area() {
    use super::banner::{BOTTOM, HEIGHT, LEFT, MARGIN, draw};
    let o = draw(&sample_banner("The Long Coast"));
    let m = MARGIN as i32;
    assert_eq!(o.x + m, LEFT as i32);
    assert_eq!(o.y + m + HEIGHT as i32, 1080 - BOTTOM as i32);
    assert_eq!(o.width as i32 - 2 * m - 3, 1920 - 2 * LEFT as i32);
    assert!(premultiplied(&o));
    // The band itself is opaque; the margin is clear.
    assert_eq!(at(&o, MARGIN + 40, MARGIN + 40)[3], 255);
    assert_eq!(at(&o, 2, 2)[3], 0);
}

#[test]
fn a_long_title_is_cut_with_an_ellipsis() {
    let font = Font::archivo();
    let long = "The Extremely Long Title of a Film That Goes On and On Past the Edge of the Panel";
    let cut = font.fit(long, 64.0, 0.0, 600.0);
    assert!(cut.ends_with('…'), "{cut}");
    assert!(font.width(&cut, 64.0, 0.0) <= 600.0);
    assert_eq!(font.fit("Short", 64.0, 0.0, 600.0), "Short");
    // It still draws.
    super::banner::draw(&sample_banner(long));
}

/// Writes the banner over a sample picture to `SURFCULT_PAINT_PREVIEW`,
/// with the pass at Heavy and Off.
#[test]
fn preview_banner() {
    let Some(dir) = std::env::var_os("SURFCULT_PAINT_PREVIEW") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    for (name, level) in [("heavy", Crt::Heavy), ("off", Crt::Off)] {
        let mut o = super::banner::draw(&sample_banner("The Long Coast (1987)"));
        crt::apply(&mut o, level);
        let (w, h) = (o.width, o.height + 60);
        let mut scene = Canvas::new(w, h);
        let (top, bottom) = (Rgba::hex(0xffcf7a), Rgba::hex(0x1f5f8f));
        scene.fill_gradient(0.0, 0.0, w as f32, h as f32, top, bottom);
        let mut rgba = scene.to_overlay(0, 0).bgra;
        for y in 0..o.height {
            for x in 0..o.width {
                let s = ((y * o.width + x) * 4) as usize;
                let d = (((y + 30) * w + x) * 4) as usize;
                let a = u32::from(o.bgra[s + 3]);
                for c in 0..3 {
                    let under = u32::from(rgba[d + c]) * (255 - a) / 255;
                    rgba[d + c] = (u32::from(o.bgra[s + c]) + under) as u8;
                }
            }
        }
        rgba.as_chunks_mut::<4>()
            .0
            .iter_mut()
            .for_each(|p| p.swap(0, 2));
        let size = tiny_skia::IntSize::from_wh(w, h).unwrap();
        let png = tiny_skia::Pixmap::from_vec(rgba, size).unwrap();
        let file = dir.join(format!("banner-{name}.png"));
        std::fs::write(file, png.encode_png().unwrap()).unwrap();
    }
}

fn sample_guide() -> super::guide::GuideView {
    use super::guide::{GuideCell, GuideRow, GuideView};
    let cell = |title: &str, left: f32, right: f32, movie: bool, focused: bool| GuideCell {
        title: title.into(),
        left,
        right,
        continues_before: left < 0.0,
        continues_after: right > 1.0,
        movie,
        focused,
    };
    GuideView {
        info: [
            "Matinee".into(),
            "“The Long Coast (1987)”".into(),
            "Saturday".into(),
            "Channel 4".into(),
        ],
        slots: ["21:00".into(), "21:30".into(), "22:00".into()],
        date: "Saturday November 28".into(),
        rows: vec![
            GuideRow {
                number: 2,
                name: "TOONS".into(),
                cells: vec![
                    cell("The Inkwell Hour", 0.0, 0.333, false, false),
                    cell("Static Pals", 0.333, 1.0, false, false),
                ],
            },
            GuideRow {
                number: 3,
                name: "RIFF".into(),
                cells: vec![cell("Video Jukebox", -0.5, 1.5, false, false)],
            },
            GuideRow {
                number: 4,
                name: "MATINEE".into(),
                cells: vec![
                    cell("The Long Coast (1987)", -0.3, 0.94, true, true),
                    cell("Night Matinee", 0.94, 2.0, true, false),
                ],
            },
            GuideRow {
                number: 5,
                name: "LAFF TRACK".into(),
                cells: vec![
                    cell("Roommates 2×04", 0.0, 0.333, false, false),
                    cell("Roommates 2×05", 0.333, 0.667, false, false),
                    cell("Corner Store", 0.667, 1.0, false, false),
                ],
            },
        ],
    }
}

#[test]
fn the_guide_leaves_the_top_right_quarter_clear_for_the_video() {
    let o = super::guide::draw(&sample_guide());
    assert_eq!((o.x, o.y, o.width, o.height), (0, 0, 1920, 1080));
    assert_eq!(at(&o, 1400, 270)[3], 0, "the video's quarter");
    assert_eq!(at(&o, 400, 270)[3], 255, "the info panel");
    assert_eq!(at(&o, 1400, 800)[3], 255, "the grid");
    assert!(premultiplied(&o));
    let clock = super::guide::draw_clock("21:16:20");
    assert_eq!((clock.x, clock.y), (96 - 16, 556 - 16));
}

/// Writes the guide over a sample picture to `SURFCULT_PAINT_PREVIEW`, as
/// the TV would show it, with the pass at Heavy.
#[test]
fn preview_guide() {
    let Some(dir) = std::env::var_os("SURFCULT_PAINT_PREVIEW") else {
        return;
    };
    let mut body = super::guide::draw(&sample_guide());
    crt::apply(&mut body, Crt::Heavy);
    let mut clock = super::guide::draw_clock("21:16:20");
    crt::apply(&mut clock, Crt::Heavy);
    let mut rows = super::guide::draw_rows(&sample_guide().rows);
    crt::apply(&mut rows, Crt::Heavy);
    // The picture: a sunset in the top-right quarter, black elsewhere.
    let mut scene = Canvas::new(1920, 1080);
    scene.fill_rect(0.0, 0.0, 1920.0, 1080.0, Rgba::hex(0x000000));
    let (top, bottom) = (Rgba::hex(0xff9a5a), Rgba::hex(0x1f5f8f));
    scene.fill_gradient(960.0, 0.0, 960.0, 540.0, top, bottom);
    let mut rgba = scene.to_overlay(0, 0).bgra;
    for o in [&body, &clock, &rows] {
        for y in 0..o.height {
            for x in 0..o.width {
                let s = ((y * o.width + x) * 4) as usize;
                let (sx, sy) = (x as i32 + o.x, y as i32 + o.y);
                let d = ((sy * 1920 + sx) * 4) as usize;
                let a = u32::from(o.bgra[s + 3]);
                for c in 0..3 {
                    let under = u32::from(rgba[d + c]) * (255 - a) / 255;
                    rgba[d + c] = (u32::from(o.bgra[s + c]) + under).min(255) as u8;
                }
            }
        }
    }
    rgba.as_chunks_mut::<4>()
        .0
        .iter_mut()
        .for_each(|p| p.swap(0, 2));
    let size = tiny_skia::IntSize::from_wh(1920, 1080).unwrap();
    let png = tiny_skia::Pixmap::from_vec(rgba, size).unwrap();
    let file = std::path::PathBuf::from(dir).join("guide-heavy.png");
    std::fs::write(file, png.encode_png().unwrap()).unwrap();
}

#[test]
fn the_rows_are_one_bitmap_92_pixels_a_row_at_the_grid() {
    use super::guide::{PITCH, VISIBLE, draw_rows};
    let rows = draw_rows(&sample_guide().rows);
    assert_eq!((rows.x, rows.y, rows.width), (96, 660, 1728));
    assert_eq!(rows.height, 4 * PITCH);
    assert_eq!(VISIBLE, 4 * PITCH);
    assert_eq!(PITCH % 4, 0, "scrolling by whole rows keeps the scanlines");
    assert_eq!(rows.y % 4, 0);
    assert!(premultiplied(&rows));
}

fn sample_menu() -> super::menu::MenuView {
    super::menu::MenuView {
        title: "PICTURE".into(),
        rows: vec![("CRT filter".into(), "HEAVY".into())],
        focus: 0,
        help:
            "Scanlines and glow on the guide, the banner and the watermark. Never on the program."
                .into(),
        keys: "OK CHANGE   BACK CLOSE".into(),
    }
}

#[test]
fn the_menu_covers_the_whole_screen() {
    let o = super::menu::draw(&sample_menu());
    assert_eq!((o.x, o.y, o.width, o.height), (0, 0, 1920, 1080));
    assert!(o.bgra.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
}

/// Writes the menu to `SURFCULT_PAINT_PREVIEW` with the pass at Heavy.
#[test]
fn preview_menu() {
    let Some(dir) = std::env::var_os("SURFCULT_PAINT_PREVIEW") else {
        return;
    };
    let mut o = super::menu::draw(&sample_menu());
    crt::apply(&mut o, Crt::Heavy);
    let mut rgba = o.bgra.clone();
    rgba.as_chunks_mut::<4>()
        .0
        .iter_mut()
        .for_each(|p| p.swap(0, 2));
    let size = tiny_skia::IntSize::from_wh(o.width, o.height).unwrap();
    let png = tiny_skia::Pixmap::from_vec(rgba, size).unwrap();
    let file = std::path::PathBuf::from(dir).join("menu-heavy.png");
    std::fs::write(file, png.encode_png().unwrap()).unwrap();
}
