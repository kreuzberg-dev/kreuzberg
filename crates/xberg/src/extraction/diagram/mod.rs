//! Deterministic diagram recovery from vector sources.
//!
//! A vector diagram already contains its own graph. Boxes are closed outlines,
//! connectors are open strokes, and labels are text drawn on top. Nothing has to
//! be inferred from pixels, so the recovered graph is exact rather than
//! probabilistic, and it carries the styling that a detection model cannot
//! recover at all.
//!
//! [`svg`] turns one source format into the geometry-carrying intermediates
//! below; [`assemble`] turns those into a [`DiagramGraph`]. The split is what
//! lets a second source format (vector PDF, DrawingML) reuse the matching rules
//! without reimplementing them.

pub(crate) mod svg;

use crate::types::diagram::{DiagramEdge, DiagramGraph, DiagramNode, DiagramShape};

/// Axis-aligned rectangle in canvas coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    fn width(&self) -> f32 {
        self.x1 - self.x0
    }

    fn height(&self) -> f32 {
        self.y1 - self.y0
    }

    fn area(&self) -> f32 {
        self.width() * self.height()
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Distance from a point to the rectangle, zero when inside.
    fn distance_to(&self, x: f32, y: f32) -> f32 {
        let dx = (self.x0 - x).max(0.0).max(x - self.x1);
        let dy = (self.y0 - y).max(0.0).max(y - self.y1);
        (dx * dx + dy * dy).sqrt()
    }
}

/// A closed outline, i.e. a node candidate.
#[derive(Debug, Clone)]
pub(crate) struct Outline {
    pub bbox: Rect,
    pub shape: DiagramShape,
    pub fill: Option<String>,
    pub stroke: Option<String>,
    pub stroke_width: Option<f32>,
    pub dashed: bool,
}

/// An open stroke, i.e. an edge candidate. Only the endpoints matter: whatever
/// route the connector takes between them, it joins the shapes they land on.
#[derive(Debug, Clone)]
pub(crate) struct Connector {
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub stroke: Option<String>,
    pub dashed: bool,
}

/// A run of text with the anchor point it is drawn from.
#[derive(Debug, Clone)]
pub(crate) struct Label {
    pub x: f32,
    pub y: f32,
    pub text: String,
}

/// Upper bound on outlines considered. Matching is quadratic in this, and a
/// diagram with more nodes than this is not a diagram.
const MAX_OUTLINES: usize = 2_000;

/// Upper bound on connectors considered, for the same reason.
const MAX_CONNECTORS: usize = 5_000;

/// A shape covering at least this fraction of the canvas is a background
/// panel, not a node. Charts and dashboards routinely draw one.
const BACKGROUND_AREA_RATIO: f32 = 0.9;

/// Shapes below this fraction of the canvas's larger dimension on either side
/// are decoration (arrowheads, bullets, tick marks) rather than nodes.
const MIN_NODE_SIDE_RATIO: f32 = 0.02;

/// How far a connector endpoint may sit from a shape and still be taken to
/// touch it, as a fraction of the canvas's larger dimension. Connectors are
/// usually drawn onto the boundary exactly; this absorbs the gap left by
/// arrowhead markers and by hand-placed endpoints.
const SNAP_RATIO: f32 = 0.02;

/// Absolute floor and ceiling applied to both ratios above, so a very small or
/// very large canvas does not produce a degenerate threshold.
const RATIO_FLOOR: f32 = 4.0;
const MIN_NODE_SIDE_CEILING: f32 = 20.0;
const SNAP_CEILING: f32 = 40.0;

/// Build a graph from recovered geometry, or `None` when the geometry is not a
/// graph.
///
/// A vector drawing is not necessarily a diagram. Bar charts, logos and
/// illustrations all yield closed outlines, and reporting those as an
/// edgeless node list would be noise dressed up as structure. Recovery
/// therefore requires at least one connector that resolves to a pair of
/// distinct shapes.
///
/// The result is deterministic. Nodes are ordered top to bottom then left to
/// right, edges by their endpoints, and duplicates of both are collapsed.
pub(crate) fn assemble(
    name: Option<String>,
    canvas: (f32, f32),
    outlines: Vec<Outline>,
    connectors: Vec<Connector>,
    labels: Vec<Label>,
) -> Option<DiagramGraph> {
    let (canvas_w, canvas_h) = canvas;
    if !canvas_w.is_finite() || !canvas_h.is_finite() || canvas_w <= 0.0 || canvas_h <= 0.0 {
        return None;
    }
    let canvas_max = canvas_w.max(canvas_h);
    let canvas_area = canvas_w * canvas_h;
    let min_side = (canvas_max * MIN_NODE_SIDE_RATIO).clamp(RATIO_FLOOR, MIN_NODE_SIDE_CEILING);
    let snap = (canvas_max * SNAP_RATIO).clamp(RATIO_FLOOR, SNAP_CEILING);

    let mut kept: Vec<Outline> = outlines
        .into_iter()
        .take(MAX_OUTLINES)
        .filter(|o| {
            o.bbox.width() >= min_side
                && o.bbox.height() >= min_side
                && o.bbox.area() < canvas_area * BACKGROUND_AREA_RATIO
        })
        .collect();

    // Reading order, with the remaining fields breaking ties so that two shapes
    // sharing a corner still sort deterministically.
    kept.sort_by(|a, b| {
        a.bbox
            .y0
            .total_cmp(&b.bbox.y0)
            .then(a.bbox.x0.total_cmp(&b.bbox.x0))
            .then(a.bbox.y1.total_cmp(&b.bbox.y1))
            .then(a.bbox.x1.total_cmp(&b.bbox.x1))
    });
    // A shape drawn twice (a fill path under a stroke path, a shadow copy) is
    // one node.
    kept.dedup_by(|a, b| a.bbox == b.bbox);

    if kept.is_empty() {
        return None;
    }

    let mut nodes: Vec<DiagramNode> = kept
        .iter()
        .enumerate()
        .map(|(i, o)| DiagramNode {
            id: format!("n{i}"),
            label: String::new(),
            shape: o.shape,
            fill: o.fill.clone(),
            stroke: o.stroke.clone(),
            stroke_width: o.stroke_width,
            dashed: o.dashed,
        })
        .collect();

    let mut labels = labels;
    labels.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));

    // Text belongs to the innermost shape that contains its anchor: a label
    // inside a box that is itself inside a panel names the box.
    let mut free_labels: Vec<&Label> = Vec::new();
    for label in &labels {
        let owner = kept
            .iter()
            .enumerate()
            .filter(|(_, o)| o.bbox.contains(label.x, label.y))
            .min_by(|(_, a), (_, b)| a.bbox.area().total_cmp(&b.bbox.area()))
            .map(|(i, _)| i);
        match owner {
            Some(i) => {
                let node_label = &mut nodes[i].label;
                if !node_label.is_empty() {
                    node_label.push('\n');
                }
                node_label.push_str(&label.text);
            }
            None => free_labels.push(label),
        }
    }

    let mut edges: Vec<DiagramEdge> = Vec::new();
    for connector in connectors.into_iter().take(MAX_CONNECTORS) {
        let (Some(from), Some(to)) = (
            snap_to_outline(&kept, connector.start, snap),
            snap_to_outline(&kept, connector.end, snap),
        ) else {
            continue;
        };
        if from == to {
            continue;
        }
        let midpoint = (
            (connector.start.0 + connector.end.0) / 2.0,
            (connector.start.1 + connector.end.1) / 2.0,
        );
        edges.push(DiagramEdge {
            from,
            to,
            label: nearest_free_label(&free_labels, midpoint, snap * 2.0),
            stroke: connector.stroke,
            dashed: connector.dashed,
        });
    }

    if edges.is_empty() {
        return None;
    }

    edges.sort_by(|a, b| a.from.cmp(&b.from).then(a.to.cmp(&b.to)));
    edges.dedup_by(|a, b| a.from == b.from && a.to == b.to && a.label == b.label);

    // Shapes no connector reached are kept. An org chart that draws leaf
    // departments without lines down to them still has those departments in
    // it, and dropping them would silently lose content the source states.
    Some(DiagramGraph { name, nodes, edges })
}

/// Index of the shape a connector endpoint lands on: the one containing it, or
/// failing that the nearest one within `tolerance`.
fn snap_to_outline(outlines: &[Outline], point: (f32, f32), tolerance: f32) -> Option<usize> {
    let (x, y) = point;
    if !x.is_finite() || !y.is_finite() {
        return None;
    }
    outlines
        .iter()
        .enumerate()
        .map(|(i, o)| (i, o.bbox.distance_to(x, y), o.bbox.area()))
        .filter(|(_, distance, _)| *distance <= tolerance)
        // Nearest wins; on a tie the smaller shape does, so an endpoint inside
        // both a box and its enclosing panel attaches to the box.
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.total_cmp(&b.2)))
        .map(|(i, _, _)| i)
}

/// Text sitting on a connector, used as the edge label.
fn nearest_free_label(labels: &[&Label], midpoint: (f32, f32), tolerance: f32) -> Option<String> {
    labels
        .iter()
        .map(|l| {
            let dx = l.x - midpoint.0;
            let dy = l.y - midpoint.1;
            (l, (dx * dx + dy * dy).sqrt())
        })
        .filter(|(_, distance)| *distance <= tolerance)
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(l, _)| l.text.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline(x0: f32, y0: f32, x1: f32, y1: f32) -> Outline {
        Outline {
            bbox: Rect { x0, y0, x1, y1 },
            shape: DiagramShape::Box,
            fill: None,
            stroke: None,
            stroke_width: None,
            dashed: false,
        }
    }

    fn connector(start: (f32, f32), end: (f32, f32)) -> Connector {
        Connector {
            start,
            end,
            stroke: None,
            dashed: false,
        }
    }

    fn label(x: f32, y: f32, text: &str) -> Label {
        Label {
            x,
            y,
            text: text.to_string(),
        }
    }

    #[test]
    fn two_boxes_and_a_line_make_an_edge() {
        let graph = assemble(
            Some("g".into()),
            (400.0, 400.0),
            vec![outline(0.0, 0.0, 100.0, 50.0), outline(0.0, 200.0, 100.0, 250.0)],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            vec![label(50.0, 25.0, "top"), label(50.0, 225.0, "bottom")],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[0].label, "top");
        assert_eq!(graph.nodes[1].label, "bottom");
        assert_eq!(graph.edges.len(), 1);
        assert_eq!((graph.edges[0].from, graph.edges[0].to), (0, 1));
    }

    #[test]
    fn shapes_without_connectors_are_not_a_graph() {
        assert!(
            assemble(
                None,
                (400.0, 400.0),
                vec![outline(0.0, 0.0, 100.0, 50.0), outline(0.0, 200.0, 100.0, 250.0)],
                Vec::new(),
                Vec::new(),
            )
            .is_none()
        );
    }

    #[test]
    fn background_panel_is_not_a_node() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 400.0, 400.0),
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
    }

    #[test]
    fn shapes_no_connector_reaches_are_kept() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                // Sorts second, and no connector touches it.
                outline(0.0, 100.0, 100.0, 150.0),
                outline(0.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 3);
        assert_eq!((graph.edges[0].from, graph.edges[0].to), (0, 2));
    }

    #[test]
    fn duplicate_outlines_and_edges_collapse() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 200.0, 100.0, 250.0),
            ],
            vec![
                connector((50.0, 50.0), (50.0, 200.0)),
                connector((50.0, 50.0), (50.0, 200.0)),
            ],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
    }

    #[test]
    fn text_outside_every_shape_can_label_an_edge() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![outline(0.0, 0.0, 100.0, 50.0), outline(0.0, 200.0, 100.0, 250.0)],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            vec![label(52.0, 126.0, "yes"), label(390.0, 390.0, "footer")],
        )
        .expect("graph");

        assert_eq!(graph.edges[0].label.as_deref(), Some("yes"));
        assert!(graph.nodes.iter().all(|n| n.label.is_empty()));
    }

    #[test]
    fn label_attaches_to_the_innermost_containing_shape() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                // A panel that is large but still under the background cutoff.
                outline(0.0, 0.0, 200.0, 300.0),
                outline(10.0, 10.0, 100.0, 60.0),
                outline(10.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 60.0), (50.0, 200.0))],
            vec![label(50.0, 30.0, "inner")],
        )
        .expect("graph");

        let inner = graph.nodes.iter().find(|n| n.label == "inner");
        assert!(inner.is_some(), "label went to the panel instead of the box");
    }

    #[test]
    fn self_loops_are_dropped() {
        assert!(
            assemble(
                None,
                (400.0, 400.0),
                vec![outline(0.0, 0.0, 100.0, 50.0), outline(0.0, 200.0, 100.0, 250.0)],
                vec![connector((10.0, 10.0), (90.0, 40.0))],
                Vec::new(),
            )
            .is_none()
        );
    }
}
