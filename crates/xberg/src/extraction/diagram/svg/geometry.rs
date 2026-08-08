//! Shape and connector geometry, read from the converted `usvg` tree.
//!
//! `usvg` resolves `use`, styles, units and the whole transform chain before
//! this module sees anything, so every path arrives in canvas coordinates.
//! What is left is deciding which paths are nodes, which are connectors, and
//! what shape each node is.

use usvg::tiny_skia_path::PathSegment;

use super::super::polyline::{Polyline, classify, halfway_along};
use super::super::{Connector, Outline, Rect};

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
                let flattened = flatten(&absolute);
                if flattened.len() < 2 {
                    continue;
                }

                let stroke = path.stroke();
                let stroke_color = stroke.and_then(|s| paint_color(s.paint()));
                let dashed = stroke.is_some_and(|s| s.dasharray().is_some());

                let closed =
                    absolute.segments().any(|s| matches!(s, PathSegment::Close)) || flattened.ends_where_it_started();

                if closed {
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
                        shape: classify(flattened.points(), &bbox),
                        bbox,
                        fill: path.fill().and_then(|f| paint_color(f.paint())),
                        stroke: stroke_color,
                        stroke_width: stroke.map(|s| s.width().get()),
                        dashed,
                    });
                } else {
                    let points = flattened.into_points();
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
fn flatten(path: &usvg::tiny_skia_path::Path) -> Polyline {
    let mut line = Polyline::default();
    let mut cursor = (0.0f32, 0.0f32);
    let mut subpath_start = (0.0f32, 0.0f32);

    for segment in path.segments() {
        match segment {
            PathSegment::MoveTo(p) => {
                cursor = (p.x, p.y);
                subpath_start = cursor;
                line.push(cursor);
            }
            PathSegment::LineTo(p) => {
                cursor = (p.x, p.y);
                line.push(cursor);
            }
            PathSegment::QuadTo(c, p) => {
                line.push_quad(cursor, (c.x, c.y), (p.x, p.y));
                cursor = (p.x, p.y);
            }
            PathSegment::CubicTo(c1, c2, p) => {
                line.push_cubic(cursor, (c1.x, c1.y), (c2.x, c2.y), (p.x, p.y));
                cursor = (p.x, p.y);
            }
            PathSegment::Close => {
                cursor = subpath_start;
                line.push(cursor);
            }
        }
    }

    line
}
