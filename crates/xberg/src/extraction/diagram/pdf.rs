//! Diagram recovery from vector PDF.
//!
//! A PDF written by a diagram tool carries the same drawing the SVG export
//! does, in a different vocabulary: `re` and `c` instead of `<rect>` and
//! `<path>`, a content stream instead of a tree. Both reduce to outlines,
//! connectors and labels, so this module only translates, and
//! [`super::assemble`] does the matching for both.
//!
//! Two things differ from SVG and both matter.
//!
//! Closedness cannot be read from an explicit close operator alone. PDF's
//! painting operators close the path themselves, so `m l l f` draws a filled
//! triangle without ever emitting `h`, and an arrowhead written that way would
//! otherwise be taken for a connector. Fill is therefore evidence of a closed
//! path here, which is exactly the opposite of the SVG case, where every path
//! inherits a black fill by default and fill says nothing at all.
//!
//! A PDF path is also a *set* of subpaths under one paint operator, where an
//! SVG element is one shape, so each subpath is measured on its own. A single
//! `PathContent` here routinely carries a node, an arrowhead and a connector.
//!
//! Coordinates are bottom-up. PDF puts the origin at the bottom-left corner of
//! the media box, while [`super::assemble`] orders nodes top to bottom the way
//! a reader sees them, so every y is flipped on the way in and everything
//! downstream stays in one coordinate space.

use pdf_oxide::elements::{PathContent, PathOperation};

use crate::pdf::oxide::OxideDocument;
use crate::types::diagram::DiagramGraph;

use super::polyline::{Polyline, classify, halfway_along};
use super::{Connector, Label, Outline};

/// Pages inspected for diagrams. Recovery costs one content-stream path parse
/// per page, which is cheap beside text extraction but not free, and a
/// document long enough to exceed this is a report rather than a drawing.
const MAX_PAGES: usize = 200;

/// Outlines a page needs before its text is worth extracting.
///
/// The path pass is cheap; the text pass is not, and on a document of prose it
/// would run a second full text extraction over every page for nothing. Two
/// outlines and one connector is the least that could possibly resolve to an
/// edge, so anything below it cannot produce a graph however it is labelled.
const MIN_OUTLINES: usize = 2;

/// Recover one graph per page that draws one.
///
/// Pages that fail to parse are skipped rather than propagated: a diagram is
/// an extra, and no PDF should fail to extract because one page's content
/// stream is malformed.
pub(crate) fn recover(doc: &mut OxideDocument) -> Vec<DiagramGraph> {
    let Ok(page_count) = doc.doc.page_count() else {
        return Vec::new();
    };

    let mut graphs = Vec::new();
    for page_index in 0..page_count.min(MAX_PAGES) {
        if let Some(graph) = recover_page(doc, page_index) {
            graphs.push(graph);
        }
    }
    graphs
}

fn recover_page(doc: &mut OxideDocument, page_index: usize) -> Option<DiagramGraph> {
    let (x0, y0, x1, y1) = doc.doc.get_page_media_box(page_index).ok()?;
    let canvas = ((x1 - x0).abs(), (y1 - y0).abs());
    let origin = (x0.min(x1), y0.min(y1));
    let top = y0.max(y1);

    let paths = crate::pdf::oxide::guard_oxide_panic(
        || doc.doc.extract_paths(page_index).map_err(|error| error.to_string()),
        |message| message,
    )
    .ok()?;

    let mut outlines = Vec::new();
    let mut connectors = Vec::new();
    for path in &paths {
        collect_path(path, origin, top, &mut outlines, &mut connectors);
    }

    if outlines.len() < MIN_OUTLINES || connectors.is_empty() {
        return None;
    }

    // Read directly rather than through `oxide::text`, whose helper is gated on
    // layout detection and reorders sparse columns in place. Labels want the
    // spans where they were drawn, not in reading order.
    let page_text = crate::pdf::oxide::guard_oxide_panic(
        || {
            doc.doc
                .extract_page_text_with_options(page_index, pdf_oxide::ReadingOrder::ColumnAware)
                .map_err(|error| error.to_string())
        },
        |message| message,
    )
    .ok()?;

    let labels = page_text
        .spans
        .iter()
        .filter(|span| !span.text.trim().is_empty())
        .map(|span| Label {
            // The centre of the span, not its corner: a label is matched
            // against the shape that contains it, and the corner of a wide
            // run of text can easily fall outside the box it names.
            x: span.bbox.x + span.bbox.width / 2.0 - origin.0,
            y: top - (span.bbox.y + span.bbox.height / 2.0),
            text: span.text.trim().to_string(),
        })
        .collect();

    super::assemble(None, canvas, outlines, connectors, labels)
}

/// Sort one path's subpaths into outlines and connectors, converting to canvas
/// coordinates on the way.
///
/// Each subpath is classified on its own. A PDF path object is a *set* of
/// subpaths painted by one operator, so a single `PathContent` routinely holds
/// a node outline, an arrowhead and a connector at once, and measuring them as
/// one run of points produces a shape that was never drawn.
fn collect_path(
    path: &PathContent,
    origin: (f32, f32),
    top: f32,
    outlines: &mut Vec<Outline>,
    connectors: &mut Vec<Connector>,
) {
    let stroke = path.stroke_color.as_ref().map(hex);
    let fill = path.fill_color.as_ref().map(hex);
    let dashed = path.dash_pattern.as_ref().is_some_and(|(dashes, _)| !dashes.is_empty());

    // Whether the path was filled, which is not the same question as whether a
    // fill colour is known. pdf_oxide starts the stroke colour at black and
    // never clears it, and reports `stroke_color: None` only for a path it
    // finalized without stroking, so an absent stroke colour *is* the fill
    // flag. Reading `fill_color` instead would miss every path painted in the
    // default colour, which for a `f` with no preceding `rg` is all of them.
    let filled = path.fill_color.is_some() || path.stroke_color.is_none();

    for (line, explicitly_closed) in split_subpaths(&path.operations, origin, top) {
        if line.len() < 2 {
            continue;
        }

        // A fill closes every subpath implicitly (PDF 32000-1 §8.5.3.3), which
        // is how a filled arrowhead reaches here with no close operator of its
        // own. In SVG the same test would be worthless: there `fill` defaults
        // to black, so every path has one.
        if explicitly_closed || filled || line.ends_where_it_started() {
            let Some(bbox) = line.bounds() else {
                continue;
            };
            outlines.push(Outline {
                shape: classify(line.points(), &bbox),
                bbox,
                fill: fill.clone(),
                stroke: stroke.clone(),
                // Only meaningful on a stroked path: `stroke_width` keeps its
                // constructed default on a path that was filled and not stroked.
                stroke_width: (stroke.is_some() && path.stroke_width > 0.0).then_some(path.stroke_width),
                dashed,
            });
        } else {
            let points = line.into_points();
            connectors.push(Connector {
                start: points[0],
                end: points[points.len() - 1],
                midpoint: halfway_along(&points),
                stroke: stroke.clone(),
                dashed,
            });
        }
    }
}

/// Break an operation stream into subpaths, flattening curves and flipping to
/// canvas coordinates. The flag on each is whether it closed itself.
fn split_subpaths(operations: &[PathOperation], origin: (f32, f32), top: f32) -> Vec<(Polyline, bool)> {
    let to_canvas = |x: f32, y: f32| (x - origin.0, top - y);

    let mut subpaths: Vec<(Polyline, bool)> = Vec::new();
    let mut line = Polyline::default();
    let mut cursor = (0.0f32, 0.0f32);
    let mut subpath_start = (0.0f32, 0.0f32);

    fn flush(line: &mut Polyline, subpaths: &mut Vec<(Polyline, bool)>, closed: bool) {
        let finished = std::mem::take(line);
        if finished.len() >= 2 {
            subpaths.push((finished, closed));
        }
    }

    for operation in operations {
        match *operation {
            PathOperation::MoveTo(x, y) => {
                flush(&mut line, &mut subpaths, false);
                cursor = to_canvas(x, y);
                subpath_start = cursor;
                line.push(cursor);
            }
            PathOperation::LineTo(x, y) => {
                cursor = to_canvas(x, y);
                line.push(cursor);
            }
            PathOperation::CurveTo(c1x, c1y, c2x, c2y, x, y) => {
                let end = to_canvas(x, y);
                line.push_cubic(cursor, to_canvas(c1x, c1y), to_canvas(c2x, c2y), end);
                cursor = end;
            }
            PathOperation::Rectangle(x, y, width, height) => {
                // `re` is a complete closed subpath in itself (§8.5.2.1), so it
                // interrupts whatever was being built rather than continuing
                // it. Width and height arrive already transformed by the CTM
                // and may be negative.
                flush(&mut line, &mut subpaths, false);
                let corners = [
                    to_canvas(x, y),
                    to_canvas(x + width, y),
                    to_canvas(x + width, y + height),
                    to_canvas(x, y + height),
                ];
                let mut rect = Polyline::default();
                for corner in corners {
                    rect.push(corner);
                }
                rect.push(corners[0]);
                subpaths.push((rect, true));
                cursor = corners[0];
                subpath_start = cursor;
            }
            PathOperation::ClosePath => {
                cursor = subpath_start;
                line.push(cursor);
                flush(&mut line, &mut subpaths, true);
                // A segment following `h` starts a fresh subpath from the
                // point the closed one began at.
                line.push(cursor);
            }
        }
    }
    flush(&mut line, &mut subpaths, false);

    subpaths
}

/// `#rrggbb` from pdf_oxide's 0..=1 channels.
fn hex(color: &pdf_oxide::layout::Color) -> String {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!(
        "#{:02x}{:02x}{:02x}",
        channel(color.r),
        channel(color.g),
        channel(color.b)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::diagram::DiagramShape;

    fn colour(r: f32, g: f32, b: f32) -> pdf_oxide::layout::Color {
        pdf_oxide::layout::Color { r, g, b }
    }

    fn path(operations: Vec<PathOperation>, fill: Option<pdf_oxide::layout::Color>) -> PathContent {
        let mut content = PathContent::new(pdf_oxide::geometry::Rect::new(0.0, 0.0, 0.0, 0.0));
        content.operations = operations;
        content.fill_color = fill;
        content.stroke_color = Some(colour(0.0, 0.0, 0.0));
        content.stroke_width = 1.0;
        content
    }

    fn sorted(path: &PathContent) -> (Vec<Outline>, Vec<Connector>) {
        let (mut outlines, mut connectors) = (Vec::new(), Vec::new());
        collect_path(path, (0.0, 0.0), 100.0, &mut outlines, &mut connectors);
        (outlines, connectors)
    }

    /// PDF's origin is the bottom-left of the media box; everything downstream
    /// reads top-down. A box drawn near the bottom of the page must come out
    /// near the *bottom* of the canvas, not the top.
    #[test]
    fn the_y_axis_is_flipped_into_reading_order() {
        let low = path(vec![PathOperation::Rectangle(10.0, 5.0, 30.0, 20.0)], None);
        let (outlines, _) = sorted(&low);

        let bbox = outlines[0].bbox;
        assert_eq!((bbox.x0, bbox.x1), (10.0, 40.0));
        // y 5..25 from the bottom of a 100-tall page is 75..95 from the top.
        assert_eq!((bbox.y0, bbox.y1), (75.0, 95.0));
    }

    /// `f` closes the path itself, so a filled triangle never emits `h`.
    /// Reading closedness from the close operator alone files every filled
    /// arrowhead as a connector, and arrowheads are what give edges direction.
    #[test]
    fn a_filled_path_is_closed_without_a_close_operator() {
        let arrowhead = path(
            vec![
                PathOperation::MoveTo(0.0, 0.0),
                PathOperation::LineTo(10.0, 0.0),
                PathOperation::LineTo(5.0, 8.0),
            ],
            Some(colour(0.2, 0.4, 0.6)),
        );
        let (outlines, connectors) = sorted(&arrowhead);

        assert_eq!(outlines.len(), 1);
        assert!(connectors.is_empty());
        assert_eq!(outlines[0].fill.as_deref(), Some("#336699"));
    }

    #[test]
    fn an_unfilled_open_stroke_is_a_connector() {
        let edge = path(
            vec![PathOperation::MoveTo(0.0, 90.0), PathOperation::LineTo(0.0, 50.0)],
            None,
        );
        let (outlines, connectors) = sorted(&edge);

        assert!(outlines.is_empty());
        assert_eq!(connectors.len(), 1);
        assert_eq!(connectors[0].start, (0.0, 10.0));
        assert_eq!(connectors[0].end, (0.0, 50.0));
        assert_eq!(connectors[0].midpoint, (0.0, 30.0));
    }

    #[test]
    fn a_rectangle_operator_measures_as_a_box() {
        let node = path(vec![PathOperation::Rectangle(0.0, 0.0, 40.0, 20.0)], None);
        let (outlines, _) = sorted(&node);
        assert_eq!(outlines[0].shape, DiagramShape::Box);
    }

    /// A rectangle whose transformed width or height came out negative still
    /// covers the same area of the page.
    #[test]
    fn a_negative_rectangle_bounds_the_same_area() {
        let flipped = path(vec![PathOperation::Rectangle(40.0, 20.0, -40.0, -20.0)], None);
        let (outlines, _) = sorted(&flipped);

        let bbox = outlines[0].bbox;
        assert_eq!((bbox.x0, bbox.x1), (0.0, 40.0));
        assert_eq!((bbox.y0, bbox.y1), (80.0, 100.0));
    }

    /// A path painted in the default colour reports no colour at all. It is
    /// still a filled shape, and an arrowhead drawn without a preceding `rg`
    /// is exactly that, so absence of colour must not mean absence of paint.
    #[test]
    fn a_default_coloured_fill_is_still_a_closed_shape() {
        let mut arrowhead = path(
            vec![
                PathOperation::MoveTo(0.0, 0.0),
                PathOperation::LineTo(10.0, 0.0),
                PathOperation::LineTo(5.0, 8.0),
            ],
            None,
        );
        arrowhead.stroke_color = None;
        let (outlines, connectors) = sorted(&arrowhead);

        assert_eq!(outlines.len(), 1);
        assert!(outlines[0].fill.is_none());
        assert!(connectors.is_empty());
    }

    #[test]
    fn channels_round_to_hex() {
        assert_eq!(hex(&colour(1.0, 1.0, 1.0)), "#ffffff");
        assert_eq!(hex(&colour(0.0, 0.0, 0.0)), "#000000");
        // Out-of-range channels clamp rather than wrapping.
        assert_eq!(hex(&colour(-0.5, 2.0, 0.5)), "#00ff80");
    }
}
