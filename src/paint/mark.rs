//! The surfcult mark, the Guide Wave with SC as its middle row, as one
//! path. The geometry is the 64-unit grid of `assets/brand/mark.svg`
//! (`tools/brand/build_marks.py`): the shapes run from (9, 11) to (56, 53).

use tiny_skia::{Path, PathBuilder, Rect};

/// The left edge of the shapes on the 64-unit grid.
pub const LEFT: f32 = 9.0;
/// The top edge of the shapes on the 64-unit grid.
pub const TOP: f32 = 11.0;
/// The width of the shapes, in grid units.
pub const WIDTH: f32 = 47.0;
/// The height of the shapes, in grid units.
pub const HEIGHT: f32 = 42.0;

/// How far a cubic's control points sit along the tangent to draw a quarter
/// circle of radius 1.
const QUARTER: f32 = 0.552_284_8;

/// The mark's shapes in one colour, on the 64-unit grid: the top row curling
/// over, S and C built from 3-unit bars, and the bottom row.
pub fn shapes() -> Path {
    let mut pb = PathBuilder::new();

    // The top row and its curl: a quarter circle of radius 13 round (43, 24)
    // outside, of radius 3 inside.
    let (r, k) = (13.0, 13.0 * QUARTER);
    let (ri, ki) = (3.0, 3.0 * QUARTER);
    pb.move_to(9.0, 11.0);
    pb.line_to(43.0, 11.0);
    pb.cubic_to(43.0 + k, 11.0, 43.0 + r, 24.0 - k, 43.0 + r, 24.0);
    pb.line_to(56.0, 33.0);
    pb.line_to(46.0, 33.0);
    pb.line_to(46.0, 24.0);
    pb.cubic_to(46.0, 24.0 - ki, 43.0 + ki, 24.0 - ri, 43.0, 21.0);
    pb.line_to(9.0, 21.0);
    pb.close();

    polygon(
        &mut pb,
        &[
            (9.0, 26.0),
            (20.0, 26.0),
            (20.0, 29.0),
            (12.0, 29.0),
            (12.0, 31.0),
            (20.0, 31.0),
            (20.0, 39.0),
            (9.0, 39.0),
            (9.0, 36.0),
            (17.0, 36.0),
            (17.0, 34.0),
            (9.0, 34.0),
        ],
    );
    polygon(
        &mut pb,
        &[
            (23.0, 26.0),
            (34.0, 26.0),
            (34.0, 29.0),
            (26.0, 29.0),
            (26.0, 36.0),
            (34.0, 36.0),
            (34.0, 39.0),
            (23.0, 39.0),
        ],
    );
    pb.push_rect(Rect::from_xywh(9.0, 44.0, 47.0, 9.0).expect("the bottom row"));
    pb.finish().expect("the mark's path")
}

fn polygon(pb: &mut PathBuilder, points: &[(f32, f32)]) {
    let (x, y) = points[0];
    pb.move_to(x, y);
    for &(x, y) in &points[1..] {
        pb.line_to(x, y);
    }
    pb.close();
}
