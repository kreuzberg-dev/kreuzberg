//! Deterministic diagram recovery from vector sources.
//!
//! A vector diagram already contains its own graph. Boxes are closed outlines,
//! connectors are open strokes, and labels are text drawn on top. Nothing has to
//! be inferred from pixels, so the recovered graph is exact rather than
//! probabilistic, and it carries the styling that a detection model cannot
//! recover at all.
//!
//! Each front end turns one source format into the geometry-carrying
//! intermediates below; [`assemble`] turns those into a [`DiagramGraph`]. The
//! split is what lets [`svg`] and [`pdf`] share every matching rule while
//! agreeing on nothing but the intermediates, and what would let a third
//! format (DrawingML, EMF) join them without touching the matching at all.

#[cfg(feature = "pdf")]
pub(crate) mod pdf;
mod polyline;
#[cfg(all(feature = "svg", feature = "xml"))]
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

    /// Whether this rectangle fully contains `other`.
    fn encloses(&self, other: &Rect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
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

/// An open stroke, i.e. an edge candidate.
///
/// The endpoints decide which shapes it joins. The midpoint is carried
/// separately because it is where a renderer puts the edge label, and on a
/// curved connector that is nowhere near the average of the two ends.
#[derive(Debug, Clone)]
pub(crate) struct Connector {
    pub start: (f32, f32),
    pub end: (f32, f32),
    pub midpoint: (f32, f32),
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

/// An outline enclosed by another and covering at least this fraction of its
/// area is the inner ring of a double border, not a node of its own.
const CONCENTRIC_AREA_RATIO: f32 = 0.5;

/// An arrowhead covers at most this fraction of the largest shape's area.
/// Graphviz draws a 7x10 head against nodes an order of magnitude larger.
const ARROWHEAD_AREA_RATIO: f32 = 0.2;

/// An arrowhead is also small in absolute terms: neither side exceeds this
/// fraction of the canvas. The area ratio alone is not enough, because a
/// diagram with one large panel makes every real node look small beside it.
const ARROWHEAD_MAX_SIDE_RATIO: f32 = 0.05;

/// How far from a connector's midpoint, in units of the snap tolerance, text
/// may sit and still be that connector's label. Renderers offset edge labels
/// clear of the line, so this has to reach further than shape snapping does.
const EDGE_LABEL_REACH: f32 = 4.0;

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

    // Two collections, because a shape too small to be a node can still be an
    // arrowhead, and an arrowhead is what tells a connector which way it points.
    // Filtering decoration away before looking for arrowheads would leave only
    // the arrowheads big enough to have been nodes, which on a diagram measured
    // in points is none of them.
    let (mut kept, decoration): (Vec<Outline>, Vec<Outline>) = outlines
        .into_iter()
        .take(MAX_OUTLINES)
        .filter(|o| o.bbox.area() < canvas_area * BACKGROUND_AREA_RATIO)
        .partition(|o| o.bbox.width() >= min_side && o.bbox.height() >= min_side);

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
    // So is a shape drawn with a double border. Graphviz's `doublecircle` and
    // the double-ruled boxes BPMN and ER diagrams use are two concentric
    // outlines around one node, and reporting the ring and the disc separately
    // both invents a node and gives the connectors two things to land on.
    collapse_concentric(&mut kept);

    if kept.is_empty() {
        return None;
    }

    let mut labels = labels;
    labels.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));

    // Text belongs to the innermost shape that contains its anchor: a label
    // inside a box that is itself inside a panel names the box.
    let owners: Vec<Option<usize>> = labels
        .iter()
        .map(|label| {
            kept.iter()
                .enumerate()
                .filter(|(_, o)| o.bbox.contains(label.x, label.y))
                .min_by(|(_, a), (_, b)| a.bbox.area().total_cmp(&b.bbox.area()))
                .map(|(i, _)| i)
        })
        .collect();

    let connectors: Vec<Connector> = connectors.into_iter().take(MAX_CONNECTORS).collect();
    let arrowheads = find_arrowheads(&kept, &connectors, &owners, snap, canvas_max);

    // Every arrowhead a connector may have to reach across: the ones large
    // enough to have been mistaken for nodes, and the decoration-sized ones
    // that were never candidates. Only the geometry is needed here, so the two
    // sources collapse to one list of boxes.
    let reach: Vec<Rect> = arrowheads
        .iter()
        .zip(&kept)
        .filter(|(is_arrowhead, _)| **is_arrowhead)
        .map(|(_, outline)| outline.bbox)
        .chain(
            decoration
                .iter()
                .filter(|o| {
                    connectors.iter().any(|c| {
                        o.bbox.distance_to(c.start.0, c.start.1) <= snap || o.bbox.distance_to(c.end.0, c.end.1) <= snap
                    })
                })
                .map(|o| o.bbox),
        )
        .collect();

    // Arrowheads are geometry belonging to the connector that ends in them, not
    // shapes in their own right, so they never become nodes.
    let node_indices: Vec<usize> = (0..kept.len()).filter(|i| !arrowheads[*i]).collect();
    if node_indices.is_empty() {
        return None;
    }
    let mut to_node = vec![usize::MAX; kept.len()];
    for (new, old) in node_indices.iter().enumerate() {
        to_node[*old] = new;
    }
    let node_outlines: Vec<&Outline> = node_indices.iter().map(|i| &kept[*i]).collect();

    let mut nodes: Vec<DiagramNode> = node_outlines
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

    let mut free_labels: Vec<&Label> = Vec::new();
    for (label, owner) in labels.iter().zip(&owners) {
        match owner.map(|old| to_node[old]).filter(|new| *new != usize::MAX) {
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
    for connector in connectors {
        // An arrowhead sits between the connector's endpoint and the shape it
        // points at, so the endpoint has to reach across it. Widening the
        // tolerance by the arrowhead's own size does that without inventing a
        // coordinate the source never drew.
        let head_at_start = touching_arrowhead(&reach, connector.start, snap);
        let head_at_end = touching_arrowhead(&reach, connector.end, snap);
        let (Some(from), Some(to)) = (
            snap_to_outline(&node_outlines, connector.start, snap + head_at_start.unwrap_or(0.0)),
            snap_to_outline(&node_outlines, connector.end, snap + head_at_end.unwrap_or(0.0)),
        ) else {
            continue;
        };
        if from == to {
            continue;
        }

        // The arrowhead is the direction. Absent one, the path's own point
        // order stands in, which is what the source author drew first.
        let (from, to, bidirectional) = match (head_at_start.is_some(), head_at_end.is_some()) {
            (true, false) => (to, from, false),
            (true, true) => (from, to, true),
            _ => (from, to, false),
        };

        edges.push(DiagramEdge {
            from,
            to,
            bidirectional,
            label: nearest_free_label(&free_labels, connector.midpoint, snap * EDGE_LABEL_REACH),
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

/// Drop outlines that are the inner ring of a double border.
///
/// The test is containment plus similar size. A panel enclosing several boxes
/// also contains them, but it is far larger, so the ratio keeps the two cases
/// apart. The outer outline survives, since that is the shape a connector
/// actually reaches.
fn collapse_concentric(outlines: &mut Vec<Outline>) {
    let mut inner = vec![false; outlines.len()];
    // Styling is split across the two rings. Graphviz fills the inner disc of a
    // `doublecircle` and leaves the outer ring unpainted, so keeping the outer
    // geometry while dropping the inner one loses the node's colour unless the
    // two are merged.
    let mut merged: Vec<(usize, Outline)> = Vec::new();
    for (i, a) in outlines.iter().enumerate() {
        for (j, b) in outlines.iter().enumerate() {
            if i == j || inner[j] {
                continue;
            }
            let area = a.bbox.area();
            if area <= 0.0 || area >= b.bbox.area() {
                continue;
            }
            if b.bbox.encloses(&a.bbox) && area >= b.bbox.area() * CONCENTRIC_AREA_RATIO {
                inner[i] = true;
                let mut outer = b.clone();
                outer.fill = outer.fill.or_else(|| a.fill.clone());
                outer.stroke = outer.stroke.or_else(|| a.stroke.clone());
                outer.stroke_width = outer.stroke_width.or(a.stroke_width);
                outer.dashed |= a.dashed;
                merged.push((j, outer));
                break;
            }
        }
    }
    for (index, outline) in merged {
        outlines[index] = outline;
    }
    let mut keep = inner.iter().map(|i| !i);
    outlines.retain(|_| keep.next().unwrap_or(true));
}

/// Mark the outlines that are arrowheads rather than nodes.
///
/// A renderer that draws arrows draws them as filled closed shapes, so they
/// arrive here indistinguishable from small boxes. Three things separate them,
/// and all three are required: an arrowhead carries no label, it sits on a
/// connector's endpoint, and it is small next to the largest shape on the
/// canvas. A real node can satisfy any one of those; satisfying all three and
/// still being a node is not something a diagram does.
fn find_arrowheads(
    outlines: &[Outline],
    connectors: &[Connector],
    label_owners: &[Option<usize>],
    snap: f32,
    canvas_max: f32,
) -> Vec<bool> {
    let max_side = canvas_max * ARROWHEAD_MAX_SIDE_RATIO;
    let mut marked = vec![false; outlines.len()];
    let Some(largest) = outlines
        .iter()
        .map(|o| o.bbox.area())
        .max_by(|a, b| a.total_cmp(b))
        .filter(|a| *a > 0.0)
    else {
        return marked;
    };

    for (index, outline) in outlines.iter().enumerate() {
        if outline.bbox.area() > largest * ARROWHEAD_AREA_RATIO {
            continue;
        }
        if outline.bbox.width() > max_side || outline.bbox.height() > max_side {
            continue;
        }
        if label_owners.contains(&Some(index)) {
            continue;
        }
        let touches = connectors.iter().any(|c| {
            outline.bbox.distance_to(c.start.0, c.start.1) <= snap || outline.bbox.distance_to(c.end.0, c.end.1) <= snap
        });
        marked[index] = touches;
    }
    marked
}

/// Size of the arrowhead sitting on `point`, if one does, as the reach a
/// connector needs to see past it.
fn touching_arrowhead(arrowheads: &[Rect], point: (f32, f32), snap: f32) -> Option<f32> {
    arrowheads
        .iter()
        .map(|bbox| (bbox.distance_to(point.0, point.1), bbox))
        .filter(|(distance, _)| *distance <= snap)
        // Nearest, not largest. Two nodes joined by a pair of opposing
        // connectors put four arrowheads within a few units of each other, and
        // taking the biggest would let one connector reach across its
        // neighbour's head and land on the wrong shape.
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(_, bbox)| bbox.width().hypot(bbox.height()))
}

/// Index of the shape a connector endpoint lands on: the one containing it, or
/// failing that the nearest one within `tolerance`.
fn snap_to_outline(outlines: &[&Outline], point: (f32, f32), tolerance: f32) -> Option<usize> {
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
            midpoint: ((start.0 + end.0) / 2.0, (start.1 + end.1) / 2.0),
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

    fn arrowhead(cx: f32, cy: f32) -> Outline {
        outline(cx - 4.0, cy - 5.0, cx + 4.0, cy + 5.0)
    }

    /// A renderer draws an arrow as a small filled shape on the end of the
    /// line. It is not a node, and the line has to reach past it.
    #[test]
    fn an_arrowhead_is_not_a_node_and_sets_direction() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                arrowhead(50.0, 194.0),
                outline(0.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 50.0), (50.0, 189.0))],
            vec![label(50.0, 25.0, "top"), label(50.0, 225.0, "bottom")],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2, "arrowhead became a node: {:?}", graph.nodes);
        assert_eq!(graph.nodes[0].label, "top");
        assert_eq!(graph.nodes[1].label, "bottom");
        assert_eq!((graph.edges[0].from, graph.edges[0].to), (0, 1));
        assert!(!graph.edges[0].bidirectional);
    }

    #[test]
    fn an_arrowhead_at_the_start_reverses_the_edge() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                arrowhead(50.0, 56.0),
                outline(0.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 61.0), (50.0, 200.0))],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!((graph.edges[0].from, graph.edges[0].to), (1, 0));
    }

    #[test]
    fn arrowheads_at_both_ends_are_bidirectional() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                arrowhead(50.0, 56.0),
                arrowhead(50.0, 194.0),
                outline(0.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 61.0), (50.0, 189.0))],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
        assert!(graph.edges[0].bidirectional);
    }

    /// A `doublecircle` is one node drawn as two rings.
    #[test]
    fn a_double_border_is_one_node() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 200.0, 100.0, 250.0),
                outline(4.0, 204.0, 96.0, 246.0),
            ],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            vec![label(50.0, 225.0, "done")],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].label, "done");
        assert_eq!((graph.edges[0].from, graph.edges[0].to), (0, 1));
    }

    /// Graphviz fills the inner disc of a `doublecircle` and leaves the outer
    /// ring unpainted, so keeping the outer geometry has to keep the inner
    /// paint or the node loses its colour.
    #[test]
    fn a_double_border_keeps_the_styling_of_both_rings() {
        let mut outer = outline(0.0, 200.0, 100.0, 250.0);
        outer.fill = None;
        outer.stroke = Some("#000000".to_string());
        let mut inner = outline(4.0, 204.0, 96.0, 246.0);
        inner.fill = Some("#fb8072".to_string());
        inner.stroke = None;

        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![outline(0.0, 0.0, 100.0, 50.0), outer, inner],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].fill.as_deref(), Some("#fb8072"));
        assert_eq!(graph.nodes[1].stroke.as_deref(), Some("#000000"));
    }

    /// A panel encloses its boxes too, but it is far larger, so it must not be
    /// collapsed into them.
    #[test]
    fn an_enclosing_panel_is_not_a_double_border() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 200.0, 300.0),
                outline(10.0, 10.0, 100.0, 60.0),
                outline(10.0, 200.0, 100.0, 250.0),
            ],
            vec![connector((50.0, 60.0), (50.0, 200.0))],
            Vec::new(),
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 3);
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
