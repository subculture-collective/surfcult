//! The CRT pass: scanlines, glow and a slight colour fringe, baked into an
//! interface overlay before the player shows it.
//!
//! MPV draws overlays after its shaders, so the video's own filters never
//! reach the interface, and the interface's look never reaches the video.
//! The pass runs only when the interface is redrawn, never per video frame.
//! See `docs/design/cable-grid.md#the-crt-pass`.
//!
//! The steps, in order:
//!
//! 1. a slight horizontal blur;
//! 2. the red channel moves left and the blue right (misconvergence);
//! 3. glow: a quarter-size copy, box-blurred twice, scaled back up and
//!    screened over the picture, so bright parts bloom past their edges;
//! 4. scanlines: in every 4 screen rows, the third and fourth are darkened.
//!    Only the colour is darkened, never the alpha, so the video does not
//!    show through in stripes.
//!
//! Scanlines follow the screen's rows, not the overlay's, so an overlay that
//! starts at a row that is not a multiple of 4 still lines up with the rest.
//! A scrolling overlay must move in steps of 4 rows for the same reason.

use crate::player::Overlay;

/// How strong the pass is. The viewer chooses; Heavy is the default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Crt {
    #[default]
    Heavy,
    Light,
    /// No pass: the interface is shown as drawn.
    Off,
}

impl Crt {
    /// `heavy`, `light` or `off`, in any case.
    pub fn parse(text: &str) -> Option<Self> {
        match text.trim().to_ascii_lowercase().as_str() {
            "heavy" => Some(Self::Heavy),
            "light" => Some(Self::Light),
            "off" => Some(Self::Off),
            _ => None,
        }
    }

    fn params(self) -> Option<Params> {
        match self {
            Self::Heavy => Some(Params {
                soften: 51,
                shift: 2,
                glow: 0.55,
                row2: 205,
                row3: 102,
            }),
            Self::Light => Some(Params {
                soften: 31,
                shift: 1,
                glow: 0.30,
                row2: 218,
                row3: 140,
            }),
            Self::Off => None,
        }
    }
}

/// One level's strengths. Weights and row factors are out of 256.
struct Params {
    /// The weight of each neighbour in the horizontal blur.
    soften: u32,
    /// How many pixels red moves left and blue moves right.
    shift: usize,
    /// How much of the glow is screened over the picture, 0 to 1.
    glow: f32,
    /// The brightness of the third row in every four.
    row2: u32,
    /// The brightness of the fourth row in every four.
    row3: u32,
}

/// The side of the square of pixels averaged into one glow pixel.
const GLOW_SCALE: usize = 4;
/// The radius of each box blur on the quarter-size copy.
const GLOW_RADIUS: usize = 2;

/// Runs the pass on `overlay` at `level`. An overlay whose pixel buffer does
/// not match its size is left alone.
pub fn apply(overlay: &mut Overlay, level: Crt) {
    let Some(p) = level.params() else {
        return;
    };
    let (w, h) = (overlay.width as usize, overlay.height as usize);
    if w == 0 || h == 0 || overlay.bgra.len() != w * h * 4 {
        return;
    }
    let px = &mut overlay.bgra;
    soften(px, w, p.soften);
    misconverge(px, w, p.shift);
    glow(px, w, h, p.glow);
    scanlines(px, w, overlay.y, &p);
}

/// Blurs each row with the weights `side`, `256 - 2 * side`, `side`.
/// Outside the overlay counts as transparent.
fn soften(px: &mut [u8], w: usize, side: u32) {
    let centre = 256 - 2 * side;
    let mut row = vec![0u8; w * 4];
    for line in px.chunks_exact_mut(w * 4) {
        row.copy_from_slice(line);
        for x in 0..w {
            for c in 0..4 {
                let at = |i: usize| u32::from(row[i * 4 + c]);
                let left = if x > 0 { at(x - 1) } else { 0 };
                let right = if x + 1 < w { at(x + 1) } else { 0 };
                let sum = side * left + centre * at(x) + side * right;
                line[x * 4 + c] = ((sum + 128) >> 8) as u8;
            }
        }
    }
}

/// Moves red `shift` pixels left and blue `shift` pixels right. Each pixel's
/// alpha becomes the largest of the three it takes colour from, so colour
/// never exceeds alpha.
fn misconverge(px: &mut [u8], w: usize, shift: usize) {
    let mut row = vec![0u8; w * 4];
    for line in px.chunks_exact_mut(w * 4) {
        row.copy_from_slice(line);
        for x in 0..w {
            let from_right = (x + shift < w).then(|| x + shift);
            let from_left = x.checked_sub(shift);
            let red = from_right.map_or(0, |i| row[i * 4 + 2]);
            let blue = from_left.map_or(0, |i| row[i * 4]);
            let alpha = row[x * 4 + 3]
                .max(from_right.map_or(0, |i| row[i * 4 + 3]))
                .max(from_left.map_or(0, |i| row[i * 4 + 3]));
            line[x * 4] = blue;
            line[x * 4 + 2] = red;
            line[x * 4 + 3] = alpha;
        }
    }
}

/// Screens a blurred quarter-size copy over the picture at `strength`.
fn glow(px: &mut [u8], w: usize, h: usize, strength: f32) {
    let (sw, sh) = (w.div_ceil(GLOW_SCALE), h.div_ceil(GLOW_SCALE));
    let mut small = vec![0f32; sw * sh * 4];
    for y in 0..h {
        for x in 0..w {
            let s = ((y / GLOW_SCALE) * sw + x / GLOW_SCALE) * 4;
            for c in 0..4 {
                small[s + c] += f32::from(px[(y * w + x) * 4 + c]);
            }
        }
    }
    let block = (GLOW_SCALE * GLOW_SCALE) as f32;
    small.iter_mut().for_each(|v| *v /= block);
    for _ in 0..2 {
        box_blur(&mut small, sw, sh, true);
        box_blur(&mut small, sw, sh, false);
    }
    // Where each output column samples the small copy, worked out once.
    let columns: Vec<(usize, usize, f32)> = (0..w)
        .map(|x| {
            let fx = ((x as f32 + 0.5) / GLOW_SCALE as f32 - 0.5).clamp(0.0, (sw - 1) as f32);
            let x0 = fx.floor() as usize;
            (x0, (x0 + 1).min(sw - 1), fx.fract())
        })
        .collect();
    let mut line = vec![0f32; sw * 4];
    for y in 0..h {
        // The small copy's row at this height, interpolated once per row.
        let fy = ((y as f32 + 0.5) / GLOW_SCALE as f32 - 0.5).clamp(0.0, (sh - 1) as f32);
        let (y0, ty) = (fy.floor() as usize, fy.fract());
        let y1 = (y0 + 1).min(sh - 1);
        for (i, v) in line.iter_mut().enumerate() {
            *v = (small[y0 * sw * 4 + i] * (1.0 - ty) + small[y1 * sw * 4 + i] * ty) * strength;
        }
        for (x, &(x0, x1, tx)) in columns.iter().enumerate() {
            let i = (y * w + x) * 4;
            let mut out = [0f32; 4];
            for (c, o) in out.iter_mut().enumerate() {
                let g = line[x0 * 4 + c] * (1.0 - tx) + line[x1 * 4 + c] * tx;
                let v = f32::from(px[i + c]);
                *o = v + g * (1.0 - v / 255.0);
            }
            let alpha = out[3].round().clamp(0.0, 255.0);
            px[i + 3] = alpha as u8;
            for c in 0..3 {
                px[i + c] = out[c].round().clamp(0.0, alpha) as u8;
            }
        }
    }
}

/// One box blur of radius [`GLOW_RADIUS`] along rows (`horizontal`) or
/// columns. Outside the copy counts as transparent.
fn box_blur(v: &mut [f32], w: usize, h: usize, horizontal: bool) {
    let (len, lines) = if horizontal { (w, h) } else { (h, w) };
    let index = |line: usize, i: usize| {
        if horizontal {
            (line * w + i) * 4
        } else {
            (i * w + line) * 4
        }
    };
    let width = (2 * GLOW_RADIUS + 1) as f32;
    let mut copy = vec![0f32; len * 4];
    for line in 0..lines {
        for i in 0..len {
            copy[i * 4..i * 4 + 4].copy_from_slice(&v[index(line, i)..index(line, i) + 4]);
        }
        for i in 0..len {
            let from = i.saturating_sub(GLOW_RADIUS);
            let to = (i + GLOW_RADIUS).min(len - 1);
            for c in 0..4 {
                let sum: f32 = (from..=to).map(|j| copy[j * 4 + c]).sum();
                v[index(line, i) + c] = sum / width;
            }
        }
    }
}

/// Darkens the colour of the third and fourth row in every four screen rows.
/// `top` is the overlay's top row on the screen.
fn scanlines(px: &mut [u8], w: usize, top: i32, p: &Params) {
    for (y, line) in px.chunks_exact_mut(w * 4).enumerate() {
        let factor = match (i64::from(top) + y as i64).rem_euclid(4) {
            2 => p.row2,
            3 => p.row3,
            _ => continue,
        };
        for pixel in line.chunks_exact_mut(4) {
            for c in &mut pixel[..3] {
                *c = ((u32::from(*c) * factor + 128) >> 8) as u8;
            }
        }
    }
}
