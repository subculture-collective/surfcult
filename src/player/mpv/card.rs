//! The stand-by card's band and text as an ASS subtitle file.
//!
//! MPV blends a subtitle track into the picture before user shaders run when
//! `blend-subtitles=yes` is set, so the CRT shader bends and darkens the band
//! and text with the bars. (Its OSD, `osd-overlay` included, is drawn after
//! the shaders.)
//!
//! The default text, PLEASE STANDBY, is drawn as vector outlines derived
//! from the Oswald typeface (SIL Open Font License 1.1) by
//! `tools/standby/gen.py`, so the Pi needs no font for it. The picture copies
//! a card whose red channel is shifted right: the text is a cyan copy and a
//! red copy 23 pixels to its right, white where they overlap, and the band
//! has a red strip on its left edge. Any other text is set in DejaVu Sans
//! Bold, a font the Pi has, as a cyan, a red and a white copy.
//!
//! Coordinates are for a 1920x1080 screen; MPV scales them to the real one.
//! Drawings use `\p3`, so one unit is a quarter of a pixel.

use std::fmt::Write as _;

use super::STANDBY_TEXT;
use super::glitch::{Flicker, Slice};

/// The cyan copy of PLEASE STANDBY, from `tools/standby/gen.py`.
const LETTERS: &str = include_str!("../../../assets/standby/text.drawing");

/// The black band: left, top, right and bottom, in `\p3` units.
const BAND: [i32; 4] = [594, 1025, 7086, 1898];
/// The red strip runs from here to the band's left edge, in `\p3` units.
const FRINGE_LEFT: i32 = 502;
/// How far right of the cyan copy the red copy sits, in `\p3` units.
const RED_SHIFT: i32 = 92;
/// `\p3` units per pixel.
const UNIT: i32 = 4;

/// The rows of the band, in whole pixels, for the glitch's slices.
pub(super) const BAND_ROWS: (u32, u32) = (257, 474);

/// The screen the card is laid out on.
const WIDTH: i32 = 1920;
const HEIGHT: i32 = 1080;

/// Where a custom text is centred, and how far its cyan and red copies sit
/// to either side of the white one, in pixels.
const TEXT_CENTRE: (i32, i32) = (960, 365);
const TEXT_OFFSET: i32 = 8;
/// The most characters of a custom text that fit the band at full size, as
/// many as PLEASE STANDBY has.
const FITS_AT_FULL_SIZE: usize = 14;

/// ASS colours are `&HBBGGRR&`.
const BLACK: &str = "000000";
const RED: &str = "0000FF";
const CYAN: &str = "FFFF00";
const WHITE: &str = "FFFFFF";

/// Points of a quadratic-turned-cubic curve kept when a drawing is cut.
const CURVE_STEPS: u32 = 8;

const HEADER: &str = "[Script Info]
; The surfcult stand-by card. The player writes this file.
ScriptType: v4.00+
PlayResX: 1920
PlayResY: 1080
WrapStyle: 2
ScaledBorderAndShadow: yes

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Card,DejaVu Sans,200,&H00FFFFFF,&H00FFFFFF,&H00000000,&H00000000,-1,0,0,0,100,100,0,0,1,0,0,5,0,0,0,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
";

/// Every event lasts from the start of the bars to long after any card
/// comes down: the generator's clock starts at 0 when the card is shown.
const SHOWN: &str = "0:00:00.00,9999:00:00.00";

/// Whether `text` is drawn with the Oswald outlines.
pub(super) fn uses_drawing(text: &str) -> bool {
    text.trim().to_uppercase() == STANDBY_TEXT
}

/// The stand-by card as an ASS file: the band, its red strip and `text`,
/// moved by `flicker` when it is given.
pub(super) fn card_ass(text: &str, flicker: Option<&Flicker>) -> String {
    let drawn = uses_drawing(text);
    let size = text_size(text);
    let text = ass_escape(&text.to_uppercase());
    let spread = flicker.map_or(0, |f| f.spread as i32);
    let mut events = Vec::new();
    for part in parts(flicker) {
        let dx = part.shift * UNIT;
        let (top, bottom) = (
            BAND[1].max(part.top * UNIT),
            BAND[3].min(part.bottom * UNIT),
        );
        if top < bottom {
            events.push(shape(BLACK, &rect(BAND[0] + dx, top, BAND[2] + dx, bottom)));
            events.push(shape(
                RED,
                &rect(FRINGE_LEFT + dx, top, BAND[0] + dx, bottom),
            ));
        }
        if drawn {
            // The two copies move apart by `spread` pixels, half each way.
            let cyan_dx = dx - spread * UNIT / 2;
            let red_dx = dx + RED_SHIFT + spread * UNIT / 2;
            let (cyan, red) = if part.whole() {
                (shift(LETTERS, cyan_dx), shift(LETTERS, red_dx))
            } else {
                let rows = (part.top * UNIT, part.bottom * UNIT);
                (cut(LETTERS, rows, cyan_dx), cut(LETTERS, rows, red_dx))
            };
            events.push(shape(CYAN, &cyan));
            events.push(shape(RED, &red));
            // White where the copies overlap: the red one, clipped to the
            // whole cyan one.
            events.push(format!(
                r"{{\an7\pos(0,0)\bord0\shad0\blur0\1c&H{WHITE}&\clip(3,{})\p3}}{red}",
                shift(LETTERS, cyan_dx)
            ));
        } else {
            let clip = if part.whole() {
                String::new()
            } else {
                format!(r"\clip(0,{},{WIDTH},{})", part.top, part.bottom)
            };
            let offset = TEXT_OFFSET + (spread + 1) / 2;
            let (x, y) = (TEXT_CENTRE.0 + part.shift, TEXT_CENTRE.1);
            for (x, colour) in [(x - offset, CYAN), (x + offset, RED), (x, WHITE)] {
                events.push(format!(
                    r"{{\an5\pos({x},{y}){clip}\fnDejaVu Sans\b1\fs{size}\fscx80\bord0\shad0\1c&H{colour}&}}{text}"
                ));
            }
        }
    }
    let mut ass = String::from(HEADER);
    for (layer, event) in events.iter().enumerate() {
        let _ = writeln!(ass, "Dialogue: {layer},{SHOWN},Card,,0,0,0,,{event}");
    }
    ass
}

/// The font size of a custom `text`: 200, or smaller for a text longer than
/// [`FITS_AT_FULL_SIZE`] characters, so it stays inside the band.
fn text_size(text: &str) -> usize {
    let length = text.trim().chars().count().max(FITS_AT_FULL_SIZE);
    200 * FITS_AT_FULL_SIZE / length
}

/// Rows of the screen drawn with one sideways shift, in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Part {
    top: i32,
    bottom: i32,
    shift: i32,
}

impl Part {
    fn whole(&self) -> bool {
        self.top <= 0 && self.bottom >= HEIGHT
    }
}

/// The screen cut into rows: the flicker's slices with their shifts and the
/// rows around them unshifted. Without a flicker, one part.
fn parts(flicker: Option<&Flicker>) -> Vec<Part> {
    let mut slices: Vec<Slice> = flicker.map(|f| f.slices.clone()).unwrap_or_default();
    slices.sort_by_key(|s| s.top);
    let mut parts = Vec::new();
    let mut row = 0;
    for slice in slices {
        let (top, bottom) = (slice.top as i32, (slice.top + slice.height) as i32);
        if top < row || bottom > HEIGHT {
            continue;
        }
        if top > row {
            parts.push(Part {
                top: row,
                bottom: top,
                shift: 0,
            });
        }
        parts.push(Part {
            top,
            bottom,
            shift: slice.shift,
        });
        row = bottom;
    }
    parts.push(Part {
        top: row,
        bottom: HEIGHT,
        shift: 0,
    });
    parts
}

/// A filled drawing in `colour`.
fn shape(colour: &str, drawing: &str) -> String {
    format!(r"{{\an7\pos(0,0)\bord0\shad0\blur0\1c&H{colour}&\p3}}{drawing}")
}

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> String {
    format!("m {left} {top} l {right} {top} {right} {bottom} {left} {bottom}")
}

/// `drawing` moved `dx` units to the right.
fn shift(drawing: &str, dx: i32) -> String {
    let mut out = Vec::new();
    let mut is_x = true;
    for token in drawing.split_whitespace() {
        match token.parse::<i32>() {
            Ok(n) => {
                out.push((if is_x { n + dx } else { n }).to_string());
                is_x = !is_x;
            }
            Err(_) => {
                out.push(token.to_owned());
                is_x = true;
            }
        }
    }
    out.join(" ")
}

/// The part of `drawing` between the rows `top` and `bottom` (units), moved
/// `dx` units to the right. Curves become straight segments. Each outline
/// is cut on its own; that keeps the counters of the letters, because a
/// region between two horizontal lines is convex.
fn cut(drawing: &str, (top, bottom): (i32, i32), dx: i32) -> String {
    let (top, bottom) = (f64::from(top), f64::from(bottom));
    let mut out = Vec::new();
    for outline in outlines(drawing) {
        let kept = clip_rows(&clip_rows(&outline, top, true), bottom, false);
        if kept.len() < 3 {
            continue;
        }
        for (i, (x, y)) in kept.iter().enumerate() {
            let (x, y) = (x.round() as i32 + dx, y.round() as i32);
            out.push(match i {
                0 => format!("m {x} {y}"),
                1 => format!("l {x} {y}"),
                _ => format!("{x} {y}"),
            });
        }
    }
    out.join(" ")
}

/// The outlines of `drawing` as polygons. `b` curves are flattened.
fn outlines(drawing: &str) -> Vec<Vec<(f64, f64)>> {
    let mut outlines = Vec::new();
    let mut command = "m";
    let mut numbers = Vec::new();
    for token in drawing.split_whitespace() {
        match token.parse::<f64>() {
            Ok(n) => numbers.push(n),
            Err(_) => {
                add_points(&mut outlines, command, &numbers);
                numbers.clear();
                command = token;
            }
        }
    }
    add_points(&mut outlines, command, &numbers);
    outlines
}

/// Adds the points of one drawing command to `outlines`.
fn add_points(outlines: &mut Vec<Vec<(f64, f64)>>, command: &str, numbers: &[f64]) {
    let points: Vec<(f64, f64)> = numbers
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&[x, y]| (x, y))
        .collect();
    match (command, outlines.last_mut()) {
        ("m", _) => outlines.extend(points.first().map(|&p| vec![p])),
        ("l", Some(outline)) => outline.extend(points),
        ("b", Some(outline)) => {
            for curve in points.as_chunks::<3>().0 {
                let start = *outline.last().unwrap_or(&curve[0]);
                outline.extend(flatten(start, curve[0], curve[1], curve[2]));
            }
        }
        _ => {}
    }
}

/// Points along the cubic curve from `p0` to `p3`, without `p0`.
fn flatten(p0: (f64, f64), p1: (f64, f64), p2: (f64, f64), p3: (f64, f64)) -> Vec<(f64, f64)> {
    (1..=CURVE_STEPS)
        .map(|k| {
            let t = f64::from(k) / f64::from(CURVE_STEPS);
            let u = 1.0 - t;
            let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
            (
                a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
                a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
            )
        })
        .collect()
}

/// The polygon cut at row `y` (Sutherland-Hodgman), keeping what is below
/// it when `below` is true and what is above it otherwise.
fn clip_rows(polygon: &[(f64, f64)], y: f64, below: bool) -> Vec<(f64, f64)> {
    let inside = |p: &(f64, f64)| if below { p.1 >= y } else { p.1 <= y };
    let mut out = Vec::new();
    for (i, current) in polygon.iter().enumerate() {
        let previous = polygon[(i + polygon.len() - 1) % polygon.len()];
        let crossing = || {
            let t = (y - previous.1) / (current.1 - previous.1);
            (previous.0 + t * (current.0 - previous.0), y)
        };
        match (inside(&previous), inside(current)) {
            (true, true) => out.push(*current),
            (true, false) => out.push(crossing()),
            (false, true) => {
                out.push(crossing());
                out.push(*current);
            }
            (false, false) => {}
        }
    }
    out
}

/// `text` as literal ASS text: a backslash cannot start an escape, a brace
/// cannot open an override block, and a line break becomes `\N`.
pub(super) fn ass_escape(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            // A word joiner after the backslash keeps it from escaping the
            // next character, as MPV does for its own OSD text.
            '\\' => escaped.push_str("\\\u{2060}"),
            '{' => escaped.push_str(r"\{"),
            '}' => escaped.push_str(r"\}"),
            '\n' => escaped.push_str(r"\N"),
            '\r' => {}
            c => escaped.push(c),
        }
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(ass: &str) -> Vec<&str> {
        ass.lines()
            .filter_map(|l| l.strip_prefix("Dialogue: "))
            .collect()
    }

    /// The numbers of a drawing as (x, y) pairs.
    fn points(drawing: &str) -> Vec<(i32, i32)> {
        let n: Vec<i32> = drawing
            .split_whitespace()
            .filter_map(|t| t.parse().ok())
            .collect();
        n.as_chunks::<2>().0.iter().map(|&[x, y]| (x, y)).collect()
    }

    #[test]
    fn the_default_text_is_drawn_from_the_oswald_outlines() {
        let ass = card_ass("please standby", None);
        assert!(ass.starts_with("[Script Info]\n"));
        assert!(ass.contains("PlayResX: 1920\nPlayResY: 1080\n"));
        let events = events(&ass);
        let letters = LETTERS.trim();
        let red = shift(letters, RED_SHIFT);
        assert_eq!(
            events,
            [
                format!(
                    r"0,{SHOWN},Card,,0,0,0,,{{\an7\pos(0,0)\bord0\shad0\blur0\1c&H000000&\p3}}m 594 1025 l 7086 1025 7086 1898 594 1898"
                ),
                format!(
                    r"1,{SHOWN},Card,,0,0,0,,{{\an7\pos(0,0)\bord0\shad0\blur0\1c&H0000FF&\p3}}m 502 1025 l 594 1025 594 1898 502 1898"
                ),
                format!(
                    r"2,{SHOWN},Card,,0,0,0,,{{\an7\pos(0,0)\bord0\shad0\blur0\1c&HFFFF00&\p3}}{letters}"
                ),
                format!(
                    r"3,{SHOWN},Card,,0,0,0,,{{\an7\pos(0,0)\bord0\shad0\blur0\1c&H0000FF&\p3}}{red}"
                ),
                format!(
                    r"4,{SHOWN},Card,,0,0,0,,{{\an7\pos(0,0)\bord0\shad0\blur0\1c&HFFFFFF&\clip(3,{letters})\p3}}{red}"
                ),
            ]
        );
        assert!(uses_drawing(" Please Standby "));
        assert!(!uses_drawing("PLEASE STAND BY"));
    }

    #[test]
    fn shifting_a_drawing_moves_only_its_x_coordinates() {
        assert_eq!(
            shift("m 1 2 l 3 4 5 6 b 7 8 9 10 11 12", 100),
            "m 101 2 l 103 4 105 6 b 107 8 109 10 111 12"
        );
    }

    #[test]
    fn a_long_custom_text_is_set_smaller_to_stay_inside_the_band() {
        assert_eq!(text_size("NO SIGNAL"), 200);
        assert_eq!(text_size(" PLEASE  WAIT! "), 200);
        assert_eq!(text_size("TECHNICAL DIFFICULTIES"), 127);
    }

    #[test]
    fn a_custom_text_is_set_in_dejavu_and_cannot_inject_ass() {
        let ass = card_ass("off {\\p1} air\\n\nnow", None);
        let events = events(&ass);
        assert_eq!(events.len(), 5);
        assert_eq!(
            events[2],
            format!(
                r"2,{SHOWN},Card,,0,0,0,,{{\an5\pos(952,365)\fnDejaVu Sans\b1\fs147\fscx80\bord0\shad0\1c&HFFFF00&}}OFF \{{\{}P1\}} AIR\{}N\NNOW",
                '\u{2060}', '\u{2060}'
            )
        );
        assert!(events[3].contains(r"\pos(968,365)") && events[3].contains("&H0000FF&"));
        assert!(events[4].contains(r"\pos(960,365)") && events[4].contains("&HFFFFFF&"));
        // One line per event: the text cannot start another.
        assert_eq!(ass.lines().count(), HEADER.lines().count() + 5);
    }

    #[test]
    fn a_flicker_moves_thin_slices_of_the_band_and_spreads_the_colours() {
        let flicker = Flicker {
            slices: vec![
                Slice {
                    top: 400,
                    height: 6,
                    shift: -5,
                },
                Slice {
                    top: 300,
                    height: 10,
                    shift: 8,
                },
            ],
            spread: 4,
        };
        let ass = card_ass(STANDBY_TEXT, Some(&flicker));
        let events = events(&ass);
        // Above, the slice at 300, between, the slice at 400 and below: five
        // parts of five events each.
        assert_eq!(events.len(), 25);
        let drawing = |event: &str| event.rsplit_once('}').unwrap().1.to_owned();
        // The band above the first slice ends where it starts.
        assert_eq!(
            drawing(events[0]),
            "m 594 1025 l 7086 1025 7086 1200 594 1200"
        );
        // The first slice's band moves 8 pixels right.
        assert_eq!(
            drawing(events[5]),
            "m 626 1200 l 7118 1200 7118 1240 626 1240"
        );
        assert_eq!(
            drawing(events[6]),
            "m 534 1200 l 626 1200 626 1240 534 1240"
        );
        // Its letters stay within its rows.
        for event in &events[7..10] {
            let points = points(&drawing(event));
            assert!(!points.is_empty());
            assert!(
                points.iter().all(|&(_, y)| (1200..=1240).contains(&y)),
                "{event}"
            );
        }
        // Outside the slices, the cyan copy moves 2 pixels left and the red
        // copy 2 pixels right of where they normally are.
        let cyan = points(&drawing(events[2]));
        let normal: Vec<(i32, i32)> = points(LETTERS);
        let left = normal.iter().map(|p| p.0).min().unwrap();
        assert_eq!(cyan.iter().map(|p| p.0).min().unwrap(), left - 8);
        let red = points(&drawing(events[3]));
        assert_eq!(red.iter().map(|p| p.0).min().unwrap(), left + RED_SHIFT + 8);
        // The last part reaches the bottom of the screen unshifted.
        assert_eq!(
            drawing(events[20]),
            "m 594 1624 l 7086 1624 7086 1898 594 1898"
        );
    }

    #[test]
    fn a_flicker_on_a_custom_text_clips_each_copy_to_its_slice() {
        let flicker = Flicker {
            slices: vec![Slice {
                top: 350,
                height: 8,
                shift: 6,
            }],
            spread: 3,
        };
        let ass = card_ass("off air", Some(&flicker));
        let events = events(&ass);
        assert_eq!(events.len(), 15);
        assert!(events[2].contains(r"\pos(950,365)\clip(0,0,1920,350)"));
        assert!(events[7].contains(r"\pos(956,365)\clip(0,350,1920,358)"));
        assert!(events[8].contains(r"\pos(976,365)\clip(0,350,1920,358)"));
        assert!(events[14].contains(r"\pos(960,365)\clip(0,358,1920,1080)"));
    }

    #[test]
    fn cutting_keeps_the_rows_of_a_square() {
        assert_eq!(
            cut("m 0 0 l 100 0 100 100 0 100", (20, 50), 10),
            "m 10 50 l 10 20 110 20 110 50"
        );
        assert_eq!(cut("m 0 0 l 100 0 100 10 0 10", (20, 50), 0), "");
    }
}
