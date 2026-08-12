//! Point math shared by every diagram front end.
//!
//! Both source formats describe the same four things (move, line, cubic,
//! close) in their own vocabulary, and both need the same answers out of the
//! result: is this closed, what shape is it, where is its middle. Only the
//! walk over the source differs, so only the walk lives in the front ends.

use crate::types::diagram::DiagramShape;

use super::Rect;

/// Points sampled per curve segment when flattening a path for area
/// measurement. Eight is enough to put a circle's measured area within a
/// percent of πr², which is well inside the gaps between the shape classes.
pub(super) const CURVE_SAMPLES: usize = 8;

/// Area-to-bounding-box ratios separating the shape classes. A rectangle fills
/// its box, an ellipse covers π/4 of it, and a quadrilateral standing on a
/// vertex covers half.
const BOX_AREA_RATIO: f32 = 0.9;
const ELLIPSE_AREA_RATIO: f32 = 0.7;
const DIAMOND_AREA_RATIO: f32 = 0.35;

/// Sub-pixel gap tolerated between a path's first and last point before it
/// stops counting as closed. `f32::EPSILON` would be useless here: at diagram
/// coordinates the rounding error already exceeds it.
const CLOSE_TOLERANCE: f32 = 1e-3;

/// Accumulates a flattened path, dropping the non-finite points that a
/// malformed source can produce.
#[derive(Debug, Default)]
pub(super) struct Polyline {
    points: Vec<(f32, f32)>,
}

impl Polyline {
    pub(super) fn push(&mut self, point: (f32, f32)) {
        if point.0.is_finite() && point.1.is_finite() {
            self.points.push(point);
        }
    }

    /// Sample a cubic from `cursor`, excluding the start point and including
    /// the end, so consecutive segments do not duplicate their shared vertex.
    pub(super) fn push_cubic(&mut self, cursor: (f32, f32), c1: (f32, f32), c2: (f32, f32), end: (f32, f32)) {
        for i in 1..=CURVE_SAMPLES {
            let t = i as f32 / CURVE_SAMPLES as f32;
            self.push(cubic_at(cursor, c1, c2, end, t));
        }
    }

    /// Only the SVG front end emits quadratic segments; the PDF front end
    /// (`pdf` without `svg`) never calls this.
    #[cfg(all(feature = "svg", feature = "xml"))]
    pub(super) fn push_quad(&mut self, cursor: (f32, f32), control: (f32, f32), end: (f32, f32)) {
        for i in 1..=CURVE_SAMPLES {
            let t = i as f32 / CURVE_SAMPLES as f32;
            self.push(quad_at(cursor, control, end, t));
        }
    }

    pub(super) fn points(&self) -> &[(f32, f32)] {
        &self.points
    }

    pub(super) fn len(&self) -> usize {
        self.points.len()
    }

    /// Whether the path returns to where it started, within tolerance. An
    /// explicit close operator is the other way a path can be closed, and each
    /// front end reports that from its own segment stream.
    pub(super) fn ends_where_it_started(&self) -> bool {
        let (Some(first), Some(last)) = (self.points.first(), self.points.last()) else {
            return false;
        };
        (first.0 - last.0).abs() < CLOSE_TOLERANCE && (first.1 - last.1).abs() < CLOSE_TOLERANCE
    }

    /// Axis-aligned bounds of the sampled points.
    ///
    /// Measured from the flattening rather than from the source's own bounding
    /// box, because a curve's control points sit outside the curve: taking a
    /// rounded rectangle's control-point bounds inflates every box it draws.
    pub(super) fn bounds(&self) -> Option<Rect> {
        let (first, rest) = self.points.split_first()?;
        let mut bbox = Rect {
            x0: first.0,
            y0: first.1,
            x1: first.0,
            y1: first.1,
        };
        for point in rest {
            bbox.x0 = bbox.x0.min(point.0);
            bbox.y0 = bbox.y0.min(point.1);
            bbox.x1 = bbox.x1.max(point.0);
            bbox.y1 = bbox.y1.max(point.1);
        }
        Some(bbox)
    }

    pub(super) fn into_points(self) -> Vec<(f32, f32)> {
        self.points
    }
}

/// The point half way along a polyline, measured by arc length.
///
/// Indexing to the middle of the point list is not the same thing. A straight
/// line flattens to exactly two points, so the middle index is its end, and
/// curves are sampled uniformly in `t` rather than in length.
pub(super) fn halfway_along(points: &[(f32, f32)]) -> (f32, f32) {
    let total: f32 = points
        .windows(2)
        .map(|w| (w[1].0 - w[0].0).hypot(w[1].1 - w[0].1))
        .sum();
    if total <= 0.0 {
        return points[0];
    }
    let mut travelled = 0.0f32;
    for window in points.windows(2) {
        let (a, b) = (window[0], window[1]);
        let step = (b.0 - a.0).hypot(b.1 - a.1);
        if travelled + step >= total / 2.0 {
            let along = if step > 0.0 {
                (total / 2.0 - travelled) / step
            } else {
                0.0
            };
            return (a.0 + (b.0 - a.0) * along, a.1 + (b.1 - a.1) * along);
        }
        travelled += step;
    }
    points[points.len() - 1]
}

#[cfg(all(feature = "svg", feature = "xml"))]
fn quad_at(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    (
        u * u * p0.0 + 2.0 * u * t * p1.0 + t * t * p2.0,
        u * u * p0.1 + 2.0 * u * t * p1.1 + t * t * p2.1,
    )
}

fn cubic_at(p0: (f32, f32), p1: (f32, f32), p2: (f32, f32), p3: (f32, f32), t: f32) -> (f32, f32) {
    let u = 1.0 - t;
    let (a, b, c, d) = (u * u * u, 3.0 * u * u * t, 3.0 * u * t * t, t * t * t);
    (
        a * p0.0 + b * p1.0 + c * p2.0 + d * p3.0,
        a * p0.1 + b * p1.1 + c * p2.1 + d * p3.1,
    )
}

/// Name the outline from how much of its bounding box it fills.
///
/// This measures the shape rather than trusting the source element, which
/// matters because by the time either front end is done there is no source
/// element left: `<rect>`, `<circle>` and a PDF `re` operator are all just
/// runs of points.
pub(super) fn classify(points: &[(f32, f32)], bbox: &Rect) -> DiagramShape {
    let box_area = (bbox.x1 - bbox.x0) * (bbox.y1 - bbox.y0);
    if box_area <= 0.0 {
        return DiagramShape::Polygon;
    }
    let ratio = polygon_area(points) / box_area;

    if ratio >= BOX_AREA_RATIO {
        DiagramShape::Box
    } else if ratio >= ELLIPSE_AREA_RATIO {
        DiagramShape::Ellipse
    } else if ratio >= DIAMOND_AREA_RATIO && corner_count(points) == 4 {
        DiagramShape::Diamond
    } else {
        DiagramShape::Polygon
    }
}

/// Shoelace area of the sampled outline.
fn polygon_area(points: &[(f32, f32)]) -> f32 {
    if points.len() < 3 {
        return 0.0;
    }
    let mut sum = 0.0f32;
    for window in points.windows(2) {
        sum += window[0].0 * window[1].1 - window[1].0 * window[0].1;
    }
    let (first, last) = (points[0], points[points.len() - 1]);
    sum += last.0 * first.1 - first.0 * last.1;
    (sum / 2.0).abs()
}

/// Distinct vertices, used only to separate a diamond from a triangle, which
/// cover the same fraction of their bounding box.
fn corner_count(points: &[(f32, f32)]) -> usize {
    let mut corners: Vec<(f32, f32)> = Vec::new();
    for &p in points {
        if !corners
            .iter()
            .any(|c| (c.0 - p.0).abs() < 0.01 && (c.1 - p.1).abs() < 0.01)
        {
            corners.push(p);
        }
    }
    corners.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A straight line flattens to exactly two points, so the middle *index*
    /// is its end. Getting this wrong meant no straight connector could ever
    /// carry a label.
    #[test]
    fn the_midpoint_is_measured_along_the_line_not_indexed() {
        assert_eq!(halfway_along(&[(0.0, 0.0), (10.0, 0.0)]), (5.0, 0.0));
        // Uneven segments: half the length falls inside the long one.
        assert_eq!(halfway_along(&[(0.0, 0.0), (2.0, 0.0), (12.0, 0.0)]), (6.0, 0.0));
        assert_eq!(halfway_along(&[(0.0, 0.0), (0.0, 0.0)]), (0.0, 0.0));
    }

    #[test]
    fn bounds_come_from_the_sampled_points() {
        let mut line = Polyline::default();
        line.push((10.0, 4.0));
        line.push((2.0, 9.0));
        let bbox = line.bounds().expect("two points bound");
        assert_eq!((bbox.x0, bbox.y0, bbox.x1, bbox.y1), (2.0, 4.0, 10.0, 9.0));
        assert!(Polyline::default().bounds().is_none());
    }

    #[test]
    fn non_finite_points_are_dropped() {
        let mut line = Polyline::default();
        line.push((f32::NAN, 0.0));
        line.push((0.0, f32::INFINITY));
        line.push((1.0, 1.0));
        assert_eq!(line.len(), 1);
    }
}
