//! Diagram recovery from SVG.
//!
//! Shapes come from `usvg`, which resolves `use`, styles, units and the whole
//! transform chain, so every outline and connector arrives in canvas
//! coordinates already. Text does not: `usvg` is built here without its `text`
//! feature, which needs a font database and drops text elements during
//! conversion. Labels therefore come from a second, small pass over the source
//! XML that reproduces only the part of `usvg` we lost, namely the transform
//! chain down to each `<text>` anchor.

use quick_xml::events::Event;
use usvg::tiny_skia_path::{PathSegment, Transform};

use super::{Connector, Label, Outline, Rect};
use crate::types::diagram::{DiagramGraph, DiagramShape};
use crate::utils::xml_utils::EntityReader;

/// Maximum input byte length accepted. Matches the cap `core::image_encode`
/// applies before handing an SVG to `usvg`, and for the same reason: usvg
/// expands the source into an in-memory tree synchronously, so a small source
/// with many `<use>` references can cost far more than its byte count suggests.
const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;

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

/// Recover a graph from SVG bytes, or `None` when the source is not a diagram.
pub(crate) fn recover(data: &[u8]) -> Option<DiagramGraph> {
    if data.is_empty() || data.len() > MAX_INPUT_BYTES {
        return None;
    }

    let options = usvg::Options {
        resources_dir: None,
        image_href_resolver: usvg::ImageHrefResolver {
            resolve_data: Box::new(|_, _, _| None),
            resolve_string: Box::new(|_, _| None),
        },
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(data, &options).ok()?;
    let canvas = (tree.size().width(), tree.size().height());

    let mut outlines = Vec::new();
    let mut connectors = Vec::new();
    collect_geometry(tree.root(), &mut outlines, &mut connectors);

    let source = String::from_utf8_lossy(data);
    let text = TextPass::default().run(&source, canvas);

    super::assemble(text.title, canvas, outlines, connectors, text.labels)
}

/// Walk the converted tree, sorting every path into an outline or a connector.
///
/// Closedness is the discriminator, not fill. SVG's initial `fill` is black, so
/// `usvg` hands a bare `<line>` a fill just as it does a `<rect>`, and treating
/// filled paths as nodes classifies every connector as one.
fn collect_geometry(group: &usvg::Group, outlines: &mut Vec<Outline>, connectors: &mut Vec<Connector>) {
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

/// Elements whose text is never drawn on the canvas.
const NON_RENDERING: &[&str] = &["defs", "clipPath", "mask", "marker", "pattern", "symbol", "metadata"];

/// Reads `<text>` anchors and the document title out of the source.
///
/// The transform stack here mirrors what `usvg` applied to the shapes: the
/// viewBox-to-viewport transform at the root, then every `transform` attribute
/// on the way down. Without it a diagram drawn inside a translated `<g>` would
/// have its labels land nowhere near its boxes.
#[derive(Default)]
struct TextPass {
    title: Option<String>,
    labels: Vec<Label>,
    /// Accumulated transform per open element, root transform at the bottom.
    transforms: Vec<Transform>,
    /// Depth of each open element, used only for its length.
    depth: Vec<String>,
    /// Depth at which a non-rendering subtree started, while one is open.
    skipping: Option<usize>,
    /// The label being built, if a `<text>` is open.
    pending: Option<Label>,
    in_title: bool,
}

impl TextPass {
    fn transform(&self) -> Transform {
        self.transforms.last().copied().unwrap_or_else(Transform::identity)
    }

    /// Handle an opening tag, returning the transform its children inherit.
    fn open(&mut self, e: &quick_xml::events::BytesStart<'_>, name: &str) -> Transform {
        let local = attribute(e, "transform")
            .and_then(|v| parse_transform(&v))
            .unwrap_or_else(Transform::identity);
        let combined = self.transform().pre_concat(local);

        if self.skipping.is_none() && NON_RENDERING.contains(&name) {
            self.skipping = Some(self.depth.len());
            return combined;
        }
        if self.skipping.is_some() {
            return combined;
        }

        match name {
            // The document title, or the accessible name of the outermost
            // group, which is where Graphviz and Mermaid put the diagram's
            // name. Deeper than that a `<title>` is a tooltip on one shape.
            "title" if self.depth.len() <= 2 && self.title.is_none() => self.in_title = true,
            "text" | "tspan" => {
                let position = (
                    attribute(e, "x").and_then(|v| first_number(&v)),
                    attribute(e, "y").and_then(|v| first_number(&v)),
                );
                if let (Some(x), Some(y)) = position {
                    // A repositioned `<tspan>` starts a new run: an org chart
                    // draws a name and a job title as two anchors, and joining
                    // them into one string would lose the line break.
                    self.flush();
                    let (x, y) = map_point(&combined, x, y);
                    self.pending = Some(Label {
                        x,
                        y,
                        text: String::new(),
                    });
                } else if name == "text" && self.pending.is_none() {
                    let (x, y) = map_point(&combined, 0.0, 0.0);
                    self.pending = Some(Label {
                        x,
                        y,
                        text: String::new(),
                    });
                }
            }
            _ => {}
        }
        combined
    }

    /// Handle a closing tag for an element opened at `depth`.
    fn close(&mut self, name: &str, depth: usize) {
        if self.skipping == Some(depth) {
            self.skipping = None;
        }
        match name {
            "title" => self.in_title = false,
            "text" => self.flush(),
            _ => {}
        }
    }

    fn push_text(&mut self, raw: &str) {
        let trimmed = raw.trim();
        if self.skipping.is_some() || trimmed.is_empty() {
            return;
        }
        if self.in_title {
            self.title.get_or_insert_with(|| trimmed.to_string());
        } else if let Some(label) = self.pending.as_mut() {
            if !label.text.is_empty() {
                label.text.push(' ');
            }
            label.text.push_str(trimmed);
        }
    }

    fn flush(&mut self) {
        if let Some(label) = self.pending.take()
            && !label.text.is_empty()
        {
            self.labels.push(label);
        }
    }

    fn run(mut self, source: &str, canvas: (f32, f32)) -> Self {
        self.transforms.push(root_transform(source, canvas));

        let mut reader = EntityReader::from_str(source);
        reader.config_mut().check_end_names = false;

        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => {
                    let name = local_name(e.name().as_ref());
                    let combined = self.open(&e, &name);
                    self.transforms.push(combined);
                    self.depth.push(name);
                }
                // A self-closing element opens and closes in one event, and
                // never passes its transform to a child.
                Ok(Event::Empty(e)) => {
                    let name = local_name(e.name().as_ref());
                    self.open(&e, &name);
                    self.close(&name, self.depth.len());
                }
                Ok(Event::End(_)) => {
                    let name = self.depth.pop().unwrap_or_default();
                    self.transforms.pop();
                    self.close(&name, self.depth.len());
                }
                Ok(Event::Text(e)) => {
                    let raw = String::from_utf8_lossy(e.as_ref());
                    self.push_text(&raw);
                }
                // A malformed tail costs the labels after it and nothing else;
                // the shapes come from a parse `usvg` already accepted.
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }

        self.flush();
        self
    }
}

fn map_point(transform: &Transform, x: f32, y: f32) -> (f32, f32) {
    let mut point = usvg::tiny_skia_path::Point::from_xy(x, y);
    transform.map_point(&mut point);
    (point.x, point.y)
}

/// Strip any namespace prefix: `svg:text` and `text` are the same element.
fn local_name(raw: &[u8]) -> String {
    let name = String::from_utf8_lossy(raw);
    match name.rsplit_once(':') {
        Some((_, local)) => local.to_string(),
        None => name.into_owned(),
    }
}

fn attribute(e: &quick_xml::events::BytesStart<'_>, wanted: &str) -> Option<String> {
    e.attributes().flatten().find_map(|attr| {
        (local_name(attr.key.as_ref()) == wanted).then(|| String::from_utf8_lossy(&attr.value).trim().to_string())
    })
}

/// First number of an SVG attribute that may hold a list, e.g. `x="10 20 30"`
/// on a `<text>` that positions each glyph.
fn first_number(value: &str) -> Option<f32> {
    let token = value.split([' ', ',', '\t', '\n', '\r']).find(|t| !t.is_empty())?;
    let end = token
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'))
        .unwrap_or(token.len());
    token[..end].parse().ok().filter(|v: &f32| v.is_finite())
}

/// The viewBox-to-viewport transform `usvg` puts at the root of the tree.
///
/// Only the default `xMidYMid meet` and the explicit `none` are handled; the
/// remaining alignments are vanishingly rare in diagrams, and getting one of
/// them wrong shifts labels rather than corrupting the graph.
fn root_transform(source: &str, canvas: (f32, f32)) -> Transform {
    let Some(view_box) = root_attribute(source, "viewBox") else {
        return Transform::identity();
    };
    let numbers: Vec<f32> = view_box
        .split([' ', ',', '\t', '\n', '\r'])
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse::<f32>().ok())
        .collect();
    let [min_x, min_y, width, height] = numbers[..] else {
        return Transform::identity();
    };
    if !(width > 0.0 && height > 0.0) {
        return Transform::identity();
    }

    let (canvas_w, canvas_h) = canvas;
    let (scale_x, scale_y) = (canvas_w / width, canvas_h / height);
    let uniform = !root_attribute(source, "preserveAspectRatio").is_some_and(|v| v.trim().starts_with("none"));

    if uniform {
        let scale = scale_x.min(scale_y);
        Transform::from_translate(
            -min_x * scale + (canvas_w - width * scale) / 2.0,
            -min_y * scale + (canvas_h - height * scale) / 2.0,
        )
        .pre_scale(scale, scale)
    } else {
        Transform::from_translate(-min_x * scale_x, -min_y * scale_y).pre_scale(scale_x, scale_y)
    }
}

/// Read an attribute off the root `<svg>` element without a second full parse.
fn root_attribute(source: &str, wanted: &str) -> Option<String> {
    let start = source.find("<svg")?;
    let end = source[start..].find('>')? + start;
    let tag = &source[start..end];
    let key = format!("{wanted}=");
    let at = tag.find(&key)? + key.len();
    let rest = tag[at..].trim_start();
    let quote = rest.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    rest[1..].split(quote).next().map(str::to_string)
}

/// Parse an SVG `transform` attribute into a single matrix.
fn parse_transform(value: &str) -> Option<Transform> {
    let mut result = Transform::identity();
    let mut rest = value.trim();
    let mut any = false;

    while let Some(open) = rest.find('(') {
        let name = rest[..open].trim().trim_start_matches(',').trim();
        let close = rest[open..].find(')')? + open;
        let args: Vec<f32> = rest[open + 1..close]
            .split([' ', ',', '\t', '\n', '\r'])
            .filter(|t| !t.is_empty())
            .filter_map(|t| t.parse::<f32>().ok())
            .collect();
        rest = &rest[close + 1..];

        let step = match (name, args.as_slice()) {
            ("translate", [tx]) => Transform::from_translate(*tx, 0.0),
            ("translate", [tx, ty, ..]) => Transform::from_translate(*tx, *ty),
            ("scale", [s]) => Transform::from_scale(*s, *s),
            ("scale", [sx, sy, ..]) => Transform::from_scale(*sx, *sy),
            ("rotate", [angle]) => Transform::from_rotate(*angle),
            ("rotate", [angle, cx, cy, ..]) => Transform::from_rotate_at(*angle, *cx, *cy),
            ("skewX", [angle]) => Transform::from_row(1.0, 0.0, angle.to_radians().tan(), 1.0, 0.0, 0.0),
            ("skewY", [angle]) => Transform::from_row(1.0, angle.to_radians().tan(), 0.0, 1.0, 0.0, 0.0),
            ("matrix", [a, b, c, d, e, f]) => Transform::from_row(*a, *b, *c, *d, *e, *f),
            _ => continue,
        };
        result = result.pre_concat(step);
        any = true;
    }

    any.then_some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recovered(source: &str) -> DiagramGraph {
        recover(source.as_bytes()).expect("expected a graph")
    }

    const TWO_BOXES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
      <title>Two Boxes</title>
      <rect x="100" y="20" width="120" height="60" fill="#2c3e50"/>
      <text x="160" y="55" text-anchor="middle">Start</text>
      <rect x="100" y="200" width="120" height="60" fill="#27ae60"/>
      <text x="160" y="235" text-anchor="middle">End</text>
      <line x1="160" y1="80" x2="160" y2="200" stroke="#333"/>
    </svg>"##;

    #[test]
    fn recovers_nodes_edges_labels_and_fills() {
        let graph = recovered(TWO_BOXES);

        assert_eq!(graph.name.as_deref(), Some("Two Boxes"));
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].label, "Start");
        assert_eq!(graph.nodes[0].fill.as_deref(), Some("#2c3e50"));
        assert_eq!(graph.nodes[0].shape, DiagramShape::Box);
        assert_eq!(graph.nodes[1].label, "End");
        assert_eq!(graph.edges.len(), 1);
        assert_eq!((graph.edges[0].from, graph.edges[0].to), (0, 1));
    }

    #[test]
    fn recovery_is_deterministic() {
        assert_eq!(recovered(TWO_BOXES), recovered(TWO_BOXES));
    }

    #[test]
    fn a_translated_group_still_matches_labels_to_shapes() {
        // Same drawing as TWO_BOXES, moved by a group transform. usvg bakes the
        // transform into the shapes, so the text pass has to apply it too.
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
              <g transform="translate(40,30) scale(1.5)">
                <rect x="10" y="10" width="120" height="60" fill="#2c3e50"/>
                <text x="70" y="45" text-anchor="middle">Start</text>
                <rect x="10" y="120" width="120" height="60" fill="#27ae60"/>
                <text x="70" y="155" text-anchor="middle">End</text>
                <line x1="70" y1="70" x2="70" y2="120" stroke="#333"/>
              </g>
            </svg>"##,
        );

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].label, "Start");
        assert_eq!(graph.nodes[1].label, "End");
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn a_viewbox_scale_still_matches_labels_to_shapes() {
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="800" height="800" viewBox="0 0 400 400">
              <rect x="100" y="20" width="120" height="60"/>
              <text x="160" y="55" text-anchor="middle">Start</text>
              <rect x="100" y="200" width="120" height="60"/>
              <text x="160" y="235" text-anchor="middle">End</text>
              <line x1="160" y1="80" x2="160" y2="200" stroke="#333"/>
            </svg>"##,
        );

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].label, "Start");
        assert_eq!(graph.nodes[1].label, "End");
    }

    #[test]
    fn shapes_are_named_from_their_outline() {
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
              <rect x="10" y="10" width="100" height="60"/>
              <ellipse cx="200" cy="140" rx="50" ry="30"/>
              <polygon points="60,200 110,240 60,280 10,240"/>
              <line x1="60" y1="70" x2="60" y2="200" stroke="#333"/>
              <line x1="110" y1="40" x2="200" y2="140" stroke="#333"/>
            </svg>"##,
        );

        let shapes: Vec<DiagramShape> = graph.nodes.iter().map(|n| n.shape).collect();
        assert_eq!(
            shapes,
            vec![DiagramShape::Box, DiagramShape::Ellipse, DiagramShape::Diamond]
        );
    }

    #[test]
    fn dashed_and_stroked_styling_survives() {
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
              <rect x="100" y="20" width="120" height="60" stroke="#ff0000" stroke-width="3" stroke-dasharray="4 2"/>
              <rect x="100" y="200" width="120" height="60"/>
              <line x1="160" y1="80" x2="160" y2="200" stroke="#0000ff" stroke-dasharray="5"/>
            </svg>"##,
        );

        assert_eq!(graph.nodes[0].stroke.as_deref(), Some("#ff0000"));
        assert_eq!(graph.nodes[0].stroke_width, Some(3.0));
        assert!(graph.nodes[0].dashed);
        assert!(!graph.nodes[1].dashed);
        assert_eq!(graph.edges[0].stroke.as_deref(), Some("#0000ff"));
        assert!(graph.edges[0].dashed);
    }

    #[test]
    fn a_drawing_without_connectors_is_not_a_graph() {
        assert!(
            recover(
                br##"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="200" viewBox="0 0 200 200">
                  <rect x="10" y="10" width="80" height="80" fill="blue"/>
                  <circle cx="150" cy="50" r="40" fill="red"/>
                  <text x="100" y="150">Hello SVG</text>
                </svg>"##
            )
            .is_none()
        );
    }

    #[test]
    fn text_in_defs_is_not_a_label() {
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
              <defs><text x="160" y="55">Hidden</text></defs>
              <rect x="100" y="20" width="120" height="60"/>
              <rect x="100" y="200" width="120" height="60"/>
              <line x1="160" y1="80" x2="160" y2="200" stroke="#333"/>
            </svg>"##,
        );

        assert!(graph.nodes[0].label.is_empty());
    }

    /// A path drawn back to its own start is a node even without a `Z`.
    #[test]
    fn an_unterminated_path_that_returns_to_its_start_is_closed() {
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
              <path d="M 100 20 L 220 20 L 220 80 L 100 80 L 100 20" fill="none" stroke="#000"/>
              <rect x="100" y="200" width="120" height="60"/>
              <line x1="160" y1="80" x2="160" y2="200" stroke="#333"/>
            </svg>"##,
        );

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].shape, DiagramShape::Box);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn malformed_input_yields_no_graph() {
        assert!(recover(b"").is_none());
        assert!(recover(b"not svg at all").is_none());
        assert!(recover(b"<svg xmlns=\"http://www.w3.org/2000/svg\"><rect").is_none());
    }

    #[test]
    fn transform_attribute_parses_each_primitive() {
        let translate = parse_transform("translate(10, 20)").expect("translate");
        assert_eq!(map_point(&translate, 1.0, 1.0), (11.0, 21.0));

        let scale = parse_transform("scale(2)").expect("scale");
        assert_eq!(map_point(&scale, 3.0, 4.0), (6.0, 8.0));

        let chained = parse_transform("translate(10,0) scale(2)").expect("chained");
        assert_eq!(map_point(&chained, 5.0, 0.0), (20.0, 0.0));

        let matrix = parse_transform("matrix(1 0 0 1 5 5)").expect("matrix");
        assert_eq!(map_point(&matrix, 0.0, 0.0), (5.0, 5.0));

        assert!(parse_transform("").is_none());
        assert!(parse_transform("nonsense").is_none());
    }

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

    #[test]
    fn an_edge_label_on_a_straight_connector_is_found() {
        let graph = recovered(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
              <rect x="100" y="20" width="120" height="60"/>
              <rect x="100" y="200" width="120" height="60"/>
              <line x1="160" y1="80" x2="160" y2="200" stroke="#333"/>
              <text x="168" y="140">on error</text>
            </svg>"##,
        );

        assert_eq!(graph.edges[0].label.as_deref(), Some("on error"));
    }

    #[test]
    fn attribute_lists_take_their_first_value() {
        assert_eq!(first_number("10 20 30"), Some(10.0));
        assert_eq!(first_number("  -4.5,2"), Some(-4.5));
        assert_eq!(first_number("12px"), Some(12.0));
        assert_eq!(first_number(""), None);
    }

    fn fixture(name: &str) -> Option<Vec<u8>> {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../test_documents/xml/{name}"));
        std::fs::read(path).ok()
    }

    /// The two diagram fixtures the repository already ships. Hand-checked
    /// against the source: every node, label, fill and edge below is what the
    /// SVG actually draws.
    #[test]
    fn recovers_the_shipped_org_chart() {
        // Self-skips when the submodule is absent, matching the repo convention.
        let Some(data) = fixture("org_chart.svg") else {
            eprintln!("test_documents not populated, skipping");
            return;
        };
        let graph = recover(&data).expect("org_chart is a diagram");

        assert_eq!(graph.name.as_deref(), Some("Organization Chart"));
        assert_eq!(graph.nodes.len(), 9);
        assert_eq!(graph.nodes[0].label, "Jane Smith\nChief Executive Officer");
        assert_eq!(graph.nodes[0].fill.as_deref(), Some("#2c3e50"));
        assert_eq!(graph.nodes[8].label, "Operations");

        // The chart draws lines only from the CEO down to the three officers.
        let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.from, e.to)).collect();
        assert_eq!(edges, vec![(0, 1), (0, 2), (0, 3)]);
        assert_eq!(graph.nodes[1].label, "Bob Chen\nChief Technology Officer");
        assert_eq!(graph.nodes[3].label, "Alex Johnson\nChief Operating Officer");
    }

    #[test]
    fn recovers_the_shipped_flowchart() {
        let Some(data) = fixture("flowchart.svg") else {
            eprintln!("test_documents not populated, skipping");
            return;
        };
        let graph = recover(&data).expect("flowchart is a diagram");

        assert_eq!(graph.name.as_deref(), Some("Software Development Lifecycle"));
        let labels: Vec<&str> = graph.nodes.iter().map(|n| n.label.as_str()).collect();
        assert_eq!(labels, vec!["Requirements", "Design", "Implementation", "Testing"]);

        let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.from, e.to)).collect();
        assert_eq!(edges, vec![(0, 1), (1, 2), (2, 3)]);

        // The four side annotations and the footer sit outside every box and
        // must not be mistaken for labels.
        assert!(
            graph.nodes.iter().all(|n| !n.label.contains("Gather user needs")),
            "annotation leaked into a node label"
        );
    }

    /// A bar chart is closed shapes plus straight lines, which is the shape of
    /// a diagram without being one. Nothing but the connector rule separates
    /// them, so it is worth asserting on a real file.
    #[test]
    fn the_shipped_bar_chart_is_not_a_diagram() {
        let Some(data) = fixture("data_dashboard.svg") else {
            eprintln!("test_documents not populated, skipping");
            return;
        };
        assert!(recover(&data).is_none());
    }
}
