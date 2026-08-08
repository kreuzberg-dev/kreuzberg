//! Shape and connector geometry, read from the converted `usvg` tree.
//!
//! `usvg` resolves `use`, styles, units and the whole transform chain before
//! this module sees anything, so every path arrives in canvas coordinates.
//! What is left is deciding which paths are nodes, which are connectors, and
//! what shape each node is.

use usvg::tiny_skia_path::PathSegment;

use super::super::{Connector, Outline, Rect};
use crate::types::diagram::DiagramShape;

/// Points sampled per curve segment when flattening a path for area
/// measurement. Eight is enough to put a circle's measured area within a
/// percent of πr², which is well inside the gaps between the shape classes.
const CURVE_SAMPLES: usize = 8;

/// Area-to-bounding-box ratios separating the shape classes. A rectangle fills
/// its box, an ellipse covers π/4 of it, and a quadrilateral standing on a
/// vertex covers half.
const BOX_AREA_RATIO: f32 = 0.9;
const ELLIPSE_AREA_RATIO: f32 = 0.7;
const DIAMOND_AREA_RATIO: f32 = 0.35;

/// Walk the converted tree, sorting every path into an outline or a connector.
///
/// Closedness is the discriminator, not fill. SVG's initial `fill` is black, so
/// `usvg` hands a bare `<line>` a fill just as it does a `<rect>`, and treating
/// filled paths as nodes classifies every connector as one.
pub(super) fn collect_geometry(group: &usvg::Group, outlines: &mut Vec<Outline>, connectors: &mut Vec<Connector>) {
    for child in group.children() {
        match child {
            usvg::Node::Group(inner) => collect_geometry(inner, outlines, connectors),
            usvg::Node::Path(path) => {
                if !path.is_visible() {
                    continue;
                }
                let Some(absolute) = path.data().clone().transform(path.abs_transform()) else {
                    continue;
                };
                let points = flatten(&absolute);
                if points.len() < 2 {
                    continue;
                }

                let stroke = path.stroke();
                let stroke_color = stroke.and_then(|s| paint_color(s.paint()));
                let dashed = stroke.is_some_and(|s| s.dasharray().is_some());

                if is_closed(&absolute, &points) {
                    // Tight bounds, not the control-point bounds `bounds()`
                    // returns: a rounded rectangle's corner controls sit outside
                    // the shape and would inflate every box.
                    let Some(bounds) = absolute.compute_tight_bounds() else {
                        continue;
                    };
                    let bbox = Rect {
                        x0: bounds.left(),
                        y0: bounds.top(),
                        x1: bounds.right(),
                        y1: bounds.bottom(),
                    };
                    outlines.push(Outline {
                        shape: classify(&points, &bbox),
                        bbox,
                        fill: path.fill().and_then(|f| paint_color(f.paint())),
                        stroke: stroke_color,
                        stroke_width: stroke.map(|s| s.width().get()),
                        dashed,
                    });
                } else {
                    connectors.push(Connector {
                        start: points[0],
                        end: points[points.len() - 1],
                        midpoint: halfway_along(&points),
                        stroke: stroke_color,
                        dashed,
                    });
                }
            }
            _ => {}
        }
    }
}

/// Flat `#rrggbb` for a solid paint. Gradients and patterns have no single
/// colour, so they are reported as absent rather than as an arbitrary stop.
fn paint_color(paint: &usvg::Paint) -> Option<String> {
    match paint {
        usvg::Paint::Color(c) => Some(format!("#{:02x}{:02x}{:02x}", c.red, c.green, c.blue)),
        _ => None,
    }
}

/// Sample a path into a polyline, subdividing curves so that area measurement
/// sees the real outline rather than its control polygon.
fn flatten(path: &usvg::tiny_skia_path::Path) -> Vec<(f32, f32)> {
    let mut points: Vec<(f32, f32)> = Vec::new();
    let mut cursor = (0.0f32, 0.0f32);
    let mut subpath_start = (0.0f32, 0.0f32);

    let push = |points: &mut Vec<(f32, f32)>, p: (f32, f32)| {
        if p.0.is_finite() && p.1.is_finite() {
            points.push(p);
        }
    };

    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                cursor = (p.x, p.y);
                subpath_start = cursor;
                push(&mut points, cursor);
            }
            PathSegment::LineTo(p) => {
                cursor = (p.x, p.y);
                push(&mut points, cursor);
            }
            PathSegment::QuadTo(c, p) => {
                for i in 1..=CURVE_SAMPLES {
                    let t = i as f32 / CURVE_SAMPLES as f32;
                    push(&mut points, quad_at(cursor, (c.x, c.y), (p.x, p.y), t));
                }
                cursor = (p.x, p.y);
            }
            PathSegment::CubicTo(c1, c2, p) => {
                for i in 1..=CURVE_SAMPLES {
                    let t = i as f32 / CURVE_SAMPLES as f32;
                    push(&mut points, cubic_at(cursor, (c1.x, c1.y), (c2.x, c2.y), (p.x, p.y), t));
                }
                cursor = (p.x, p.y);
            }
            PathSegment::Close => {
                cursor = subpath_start;
                push(&mut points, cursor);
            }
        }
    }

    points
}

/// The point half way along a polyline, measured by arc length.
///
/// Indexing to the middle of the point list is not the same thing. A straight
/// `<line>` flattens to exactly two points, so the middle index is its end, and
/// curves are sampled uniformly in `t` rather than in length.
fn halfway_along(points: &[(f32, f32)]) -> (f32, f32) {
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

/// Sub-pixel gap tolerated between a path's first and last point before it
/// stops counting as closed. `f32::EPSILON` would be useless here: at diagram
/// coordinates the rounding error already exceeds it.
const CLOSE_TOLERANCE: f32 = 1e-3;

/// An explicit `Z` closes a path; so does ending where it started, which is how
/// a hand-written `M … L … L … L …` back to the origin comes through.
fn is_closed(path: &usvg::tiny_skia_path::Path, points: &[(f32, f32)]) -> bool {
    if path.segments().any(|s| matches!(s, PathSegment::Close)) {
        return true;
    }
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return false;
    };
    (first.0 - last.0).abs() < CLOSE_TOLERANCE && (first.1 - last.1).abs() < CLOSE_TOLERANCE
}

/// Name the outline from how much of its bounding box it fills.
///
/// This measures the shape rather than trusting the source element, which
/// matters because by the time `usvg` is done there is no source element left:
/// `<rect>`, `<circle>` and `<polygon>` are all just paths.
fn classify(points: &[(f32, f32)], bbox: &Rect) -> DiagramShape {
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

    /// A straight `<line>` flattens to exactly two points, so the middle
    /// *index* is its end. Getting this wrong meant no straight connector could
    /// ever carry a label.
    #[test]
    fn the_midpoint_is_measured_along_the_line_not_indexed() {
        assert_eq!(halfway_along(&[(0.0, 0.0), (10.0, 0.0)]), (5.0, 0.0));
        // Uneven segments: half the length falls inside the long one.
        assert_eq!(halfway_along(&[(0.0, 0.0), (2.0, 0.0), (12.0, 0.0)]), (6.0, 0.0));
        assert_eq!(halfway_along(&[(0.0, 0.0), (0.0, 0.0)]), (0.0, 0.0));
    }
}
