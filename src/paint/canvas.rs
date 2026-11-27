//! A bitmap to draw the interface into.

use tiny_skia::{
    Color, FillRule, GradientStop, LinearGradient, Paint, Path, Pixmap, Point, Rect, Shader,
    SpreadMode, Transform,
};

use super::Font;
use crate::player::Overlay;

/// A colour with straight (not premultiplied) alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    /// An opaque colour from `0xRRGGBB`.
    pub const fn hex(rgb: u32) -> Self {
        Self {
            r: (rgb >> 16) as u8,
            g: (rgb >> 8) as u8,
            b: rgb as u8,
            a: 255,
        }
    }

    /// The same colour with alpha `a`.
    pub const fn with_alpha(self, a: u8) -> Self {
        Self { a, ..self }
    }

    fn color(self) -> Color {
        Color::from_rgba8(self.r, self.g, self.b, self.a)
    }

    fn paint(self) -> Paint<'static> {
        let mut paint = Paint::default();
        paint.set_color(self.color());
        paint.anti_alias = true;
        paint
    }
}

/// A hard text shadow: the text again, `dx`, `dy` pixels away, in `color`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Shadow {
    pub dx: f32,
    pub dy: f32,
    pub color: Rgba,
}

/// A transparent bitmap `width` by `height` pixels that the interface is
/// drawn into. Coordinates are pixels from the top left. Anything drawn
/// outside the bitmap is clipped.
pub struct Canvas {
    pixmap: Pixmap,
}

impl Canvas {
    /// A transparent canvas. A zero side is made 1 pixel.
    pub fn new(width: u32, height: u32) -> Self {
        let pixmap = Pixmap::new(width.max(1), height.max(1)).expect("a canvas of at least 1x1");
        Self { pixmap }
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// Fills a rectangle. An empty or negative one draws nothing.
    pub fn fill_rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba) {
        if let Some(rect) = Rect::from_xywh(x, y, w, h) {
            self.pixmap
                .fill_rect(rect, &color.paint(), Transform::identity(), None);
        }
    }

    /// Fills a rectangle with a vertical gradient from `top` to `bottom`.
    pub fn fill_gradient(&mut self, x: f32, y: f32, w: f32, h: f32, top: Rgba, bottom: Rgba) {
        let Some(rect) = Rect::from_xywh(x, y, w, h) else {
            return;
        };
        let shader = LinearGradient::new(
            Point::from_xy(x, y),
            Point::from_xy(x, y + h),
            vec![
                GradientStop::new(0.0, top.color()),
                GradientStop::new(1.0, bottom.color()),
            ],
            SpreadMode::Pad,
            Transform::identity(),
        );
        let paint = Paint {
            shader: shader.unwrap_or(Shader::SolidColor(top.color())),
            anti_alias: true,
            ..Paint::default()
        };
        self.pixmap
            .fill_rect(rect, &paint, Transform::identity(), None);
    }

    /// A bevelled cell: `fill`, with an `edge`-pixel light edge along the top
    /// and left and a dark one along the bottom and right.
    #[allow(clippy::too_many_arguments)]
    pub fn bevel(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        h: f32,
        fill: Rgba,
        light: Rgba,
        dark: Rgba,
        edge: f32,
    ) {
        self.fill_rect(x, y, w, h, fill);
        self.fill_rect(x, y, w, edge, light);
        self.fill_rect(x, y, edge, h, light);
        self.fill_rect(x, y + h - edge, w, edge, dark);
        self.fill_rect(x + w - edge, y, edge, h, dark);
    }

    /// A `width`-pixel outline inside the rectangle, as the focus box.
    pub fn outline(&mut self, x: f32, y: f32, w: f32, h: f32, color: Rgba, width: f32) {
        self.fill_rect(x, y, w, width, color);
        self.fill_rect(x, y + h - width, w, width, color);
        self.fill_rect(x, y, width, h, color);
        self.fill_rect(x + w - width, y, width, h, color);
    }

    /// Fills `path`, moved and scaled by `transform`.
    pub fn fill_path(&mut self, path: &Path, color: Rgba, transform: Transform) {
        self.pixmap
            .fill_path(path, &color.paint(), FillRule::Winding, transform, None);
    }

    /// Draws one line of `text` in `font` at `px`, with its baseline starting
    /// at `x`, `baseline`, and an optional hard shadow under it. Returns the
    /// text's width.
    #[allow(clippy::too_many_arguments)]
    pub fn text(
        &mut self,
        font: &Font,
        text: &str,
        px: f32,
        tracking: f32,
        x: f32,
        baseline: f32,
        color: Rgba,
        shadow: Option<Shadow>,
    ) -> f32 {
        if let Some(path) = font.outline(text, px, tracking, x, baseline) {
            if let Some(s) = shadow {
                self.fill_path(&path, s.color, Transform::from_translate(s.dx, s.dy));
            }
            self.fill_path(&path, color, Transform::identity());
        }
        font.width(text, px, tracking)
    }

    /// The pixel at `x`, `y` as premultiplied red, green, blue and alpha.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        self.pixmap
            .pixel(x, y)
            .map(|p| [p.red(), p.green(), p.blue(), p.alpha()])
    }

    /// The canvas as an overlay with its top left at `x`, `y` on the screen:
    /// the same premultiplied pixels, reordered to blue, green, red, alpha.
    pub fn to_overlay(&self, x: i32, y: i32) -> Overlay {
        let mut bgra = self.pixmap.data().to_vec();
        for px in bgra.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
        }
        Overlay {
            x,
            y,
            width: self.width(),
            height: self.height(),
            bgra,
        }
    }

    /// Encodes the canvas as a PNG, to look at a drawing in tests.
    #[cfg(test)]
    pub fn png(&self) -> Vec<u8> {
        self.pixmap.encode_png().expect("a PNG of the canvas")
    }
}
