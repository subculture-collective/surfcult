//! A music channel's picture in the Cable Grid design
//! (`docs/design/cable-grid.md`): the cover card, which fills the screen
//! while a song plays on a `picture = "cover"` channel, and the now-playing
//! line over the visualizer of a `picture = "visualizer"` channel.
//!
//! The card is the violet field with the album cover centred in a bevelled
//! frame, an ochre band with the artist under it, then the title and the
//! album. It keeps clear of the bottom of the screen, where the channel
//! banner shows. A song without a cover gets the surfcult mark in its frame.
//! Text that does not fit is cut with an ellipsis.

use tiny_skia::{PathBuilder, Transform};

use super::palette::*;
use super::{Canvas, Font, Image, Rgba, Shadow, mark};
use crate::player::Overlay;

/// What the card and the line say about the song on screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NowPlaying {
    pub title: String,
    /// The artists, joined with commas; empty when the source names none.
    pub artists: String,
    pub album: Option<String>,
}

const SCREEN: (u32, u32) = (1920, 1080);
/// The cover's side, and its top on the screen.
const COVER: f32 = 440.0;
const COVER_TOP: f32 = 96.0;
/// The frame's width around the cover.
const FRAME: f32 = 10.0;
const EDGE: f32 = 4.0;
const BAND_WIDTH: f32 = 1120.0;
const BAND_HEIGHT: f32 = 60.0;
const SLANT: f32 = 24.0;
const SHADOW_OFFSET: f32 = 3.0;
/// Text may run this close to the screen's sides (the 5% a TV may cut).
const SIDE: f32 = 96.0;

/// The line's place: inside the 5% margin at the top of the screen.
const LINE_LEFT: u32 = 96;
const LINE_TOP: u32 = 54;
const LINE_HEIGHT: u32 = 64;
/// Transparent space around the line for the CRT pass's glow.
const MARGIN: u32 = 16;

fn shadow() -> Option<Shadow> {
    Some(Shadow {
        dx: SHADOW_OFFSET,
        dy: SHADOW_OFFSET,
        color: SHADOW,
    })
}

/// The cover card for `song` as a full-screen overlay, before the CRT pass.
pub fn card(song: &NowPlaying, cover: Option<&Image>) -> Overlay {
    let archivo = Font::archivo();
    let (w, h) = (SCREEN.0 as f32, SCREEN.1 as f32);
    let mut c = Canvas::new(SCREEN.0, SCREEN.1);
    c.fill_gradient(0.0, 0.0, w, h, FIELD_TOP, FIELD_BOTTOM);

    // The cover in its frame, or the mark.
    let x = (w - COVER) / 2.0;
    c.bevel(
        x - FRAME,
        COVER_TOP - FRAME,
        COVER + 2.0 * FRAME,
        COVER + 2.0 * FRAME,
        CELL,
        BEVEL_LIGHT,
        BEVEL_DARK,
        EDGE,
    );
    match cover {
        Some(image) => {
            // Fit inside the square, keeping the cover's shape.
            let scale = COVER / image.width().max(image.height()) as f32;
            let (iw, ih) = (image.width() as f32 * scale, image.height() as f32 * scale);
            c.image(
                image,
                x + (COVER - iw) / 2.0,
                COVER_TOP + (COVER - ih) / 2.0,
                iw,
                ih,
            );
        }
        None => {
            let side = COVER * 0.6;
            let scale = side / mark::WIDTH.max(mark::HEIGHT);
            let left = x + (COVER - mark::WIDTH * scale) / 2.0 - mark::LEFT * scale;
            let top = COVER_TOP + (COVER - mark::HEIGHT * scale) / 2.0 - mark::TOP * scale;
            c.fill_path(
                &mark::shapes(),
                YELLOW,
                Transform::from_row(scale, 0.0, 0.0, scale, left, top),
            );
        }
    }

    // The ochre band with the artist.
    let band_top = COVER_TOP + COVER + FRAME + 26.0;
    let bx = (w - BAND_WIDTH) / 2.0;
    let mut band = PathBuilder::new();
    band.move_to(bx + SLANT, band_top);
    band.line_to(bx + BAND_WIDTH, band_top);
    band.line_to(bx + BAND_WIDTH - SLANT, band_top + BAND_HEIGHT);
    band.line_to(bx, band_top + BAND_HEIGHT);
    band.close();
    if let Some(band) = band.finish() {
        c.fill_path_gradient(&band, band_top, BAND_HEIGHT, OCHRE_TOP, OCHRE_BOTTOM);
    }
    let artist = archivo.fit(&song.artists, 38.0, 0.5, BAND_WIDTH - 2.0 * SLANT - 40.0);
    centred(
        &mut c,
        &archivo,
        &artist,
        38.0,
        0.5,
        band_top + 44.0,
        OCHRE_INK,
        None,
    );

    // The title and the album.
    let wide = w - 2.0 * SIDE;
    let title = archivo.fit(&song.title, 72.0, 0.0, wide);
    let title_base = band_top + BAND_HEIGHT + 96.0;
    centred(
        &mut c,
        &archivo,
        &title,
        72.0,
        0.0,
        title_base,
        WHITE,
        shadow(),
    );
    if let Some(album) = &song.album {
        let album = archivo.fit(album, 38.0, 0.5, wide);
        centred(
            &mut c,
            &archivo,
            &album,
            38.0,
            0.5,
            title_base + 60.0,
            YELLOW,
            shadow(),
        );
    }
    c.to_overlay(0, 0)
}

/// The now-playing line for `song` across the top of the screen, as an
/// overlay at its place, before the CRT pass: an ochre band with the
/// artists and title, and the album after them.
pub fn line(song: &NowPlaying) -> Overlay {
    let archivo = Font::archivo();
    let width = SCREEN.0 - 2 * LINE_LEFT;
    let extra = SHADOW_OFFSET.ceil() as u32;
    let mut c = Canvas::new(width + 2 * MARGIN + extra, LINE_HEIGHT + 2 * MARGIN + extra);
    let (x0, y0) = (MARGIN as f32, MARGIN as f32);
    let (w, h) = (width as f32, LINE_HEIGHT as f32);
    let mut band = PathBuilder::new();
    band.move_to(x0, y0);
    band.line_to(x0 + w, y0);
    band.line_to(x0 + w - SLANT, y0 + h);
    band.line_to(x0, y0 + h);
    band.close();
    if let Some(band) = band.finish() {
        c.fill_path_gradient(&band, y0, h, OCHRE_TOP, OCHRE_BOTTOM);
    }
    let mut text = if song.artists.is_empty() {
        song.title.clone()
    } else {
        format!("{} – {}", song.artists, song.title)
    };
    if let Some(album) = &song.album {
        text = format!("{text} · {album}");
    }
    let text = archivo.fit(&text, 38.0, 0.5, w - 2.0 * 24.0 - SLANT);
    c.text(
        &archivo,
        &text,
        38.0,
        0.5,
        x0 + 24.0,
        y0 + 46.0,
        OCHRE_INK,
        None,
    );
    c.to_overlay(
        LINE_LEFT as i32 - MARGIN as i32,
        LINE_TOP as i32 - MARGIN as i32,
    )
}

/// One line of text centred across the screen.
#[allow(clippy::too_many_arguments)]
fn centred(
    c: &mut Canvas,
    font: &Font,
    text: &str,
    px: f32,
    tracking: f32,
    baseline: f32,
    color: Rgba,
    shadow: Option<Shadow>,
) {
    let width = font.width(text, px, tracking);
    let x = (SCREEN.0 as f32 - width) / 2.0;
    c.text(font, text, px, tracking, x, baseline, color, shadow);
}
