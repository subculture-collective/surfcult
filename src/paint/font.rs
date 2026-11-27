//! The design's two fonts, compiled in, and their glyph outlines.

use std::fmt;

use tiny_skia::{Path, PathBuilder};
use ttf_parser::{Face, OutlineBuilder};

const ARCHIVO_NARROW_BOLD: &[u8] = include_bytes!("../../assets/fonts/ArchivoNarrow-Bold.ttf");
const VCR_OSD_MONO: &[u8] = include_bytes!("../../assets/fonts/VCR_OSD_MONO_1.001.ttf");

/// A font face that can measure and outline a line of text.
///
/// Characters the font lacks are skipped. Kerning is not applied; the
/// interface's strings are short and mostly capitals.
pub struct Font {
    face: Face<'static>,
    name: &'static str,
}

impl fmt::Debug for Font {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Font").field(&self.name).finish()
    }
}

impl Font {
    /// Archivo Narrow Bold: every title, label and heading on the TV.
    pub fn archivo() -> Self {
        Self::load(ARCHIVO_NARROW_BOLD, "Archivo Narrow Bold")
    }

    /// VCR OSD Mono: clocks and the footer line of menus.
    pub fn vcr() -> Self {
        Self::load(VCR_OSD_MONO, "VCR OSD Mono")
    }

    fn load(data: &'static [u8], name: &'static str) -> Self {
        // The fonts are compiled in and checked by the tests, so a parse
        // failure is a build mistake, not a runtime condition.
        let face = Face::parse(data, 0).unwrap_or_else(|e| panic!("{name} does not parse: {e}"));
        Self { face, name }
    }

    fn scale(&self, px: f32) -> f32 {
        px / f32::from(self.face.units_per_em())
    }

    /// The height of a capital letter at `px`, from the font's own metrics.
    pub fn cap_height(&self, px: f32) -> f32 {
        let cap = self
            .face
            .capital_height()
            .unwrap_or_else(|| self.face.ascender());
        f32::from(cap) * self.scale(px)
    }

    /// The width of `text` at `px`, with `tracking` extra pixels between
    /// characters.
    pub fn width(&self, text: &str, px: f32, tracking: f32) -> f32 {
        let scale = self.scale(px);
        let mut width = 0.0;
        let mut count = 0usize;
        for ch in text.chars() {
            if let Some(id) = self.face.glyph_index(ch) {
                width += f32::from(self.face.glyph_hor_advance(id).unwrap_or(0)) * scale;
                count += 1;
            }
        }
        width + tracking * count.saturating_sub(1) as f32
    }

    /// The outline of `text` at `px` with its baseline starting at `x`,
    /// `baseline`, or `None` when nothing in it has an outline (spaces only,
    /// or no character the font has).
    pub fn outline(
        &self,
        text: &str,
        px: f32,
        tracking: f32,
        x: f32,
        baseline: f32,
    ) -> Option<Path> {
        let scale = self.scale(px);
        let mut pen = Pen {
            builder: PathBuilder::new(),
            scale,
            x,
            y: baseline,
        };
        for ch in text.chars() {
            let Some(id) = self.face.glyph_index(ch) else {
                continue;
            };
            self.face.outline_glyph(id, &mut pen);
            pen.x += f32::from(self.face.glyph_hor_advance(id).unwrap_or(0)) * scale + tracking;
        }
        pen.builder.finish()
    }
}

/// Turns font units into a path on the canvas: scaled, flipped (fonts point
/// up) and moved to the pen's position.
struct Pen {
    builder: PathBuilder,
    scale: f32,
    x: f32,
    y: f32,
}

impl Pen {
    fn at(&self, x: f32, y: f32) -> (f32, f32) {
        (self.x + x * self.scale, self.y - y * self.scale)
    }
}

impl OutlineBuilder for Pen {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.builder.move_to(x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.at(x, y);
        self.builder.line_to(x, y);
    }

    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        let (x1, y1) = self.at(x1, y1);
        let (x, y) = self.at(x, y);
        self.builder.quad_to(x1, y1, x, y);
    }

    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (x1, y1) = self.at(x1, y1);
        let (x2, y2) = self.at(x2, y2);
        let (x, y) = self.at(x, y);
        self.builder.cubic_to(x1, y1, x2, y2, x, y);
    }

    fn close(&mut self) {
        self.builder.close();
    }
}
