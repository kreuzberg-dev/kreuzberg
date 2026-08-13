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

use std::collections::HashMap;

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

    fn centre_x(&self) -> f32 {
        (self.x0 + self.x1) / 2.0
    }

    fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x0 && x <= self.x1 && y >= self.y0 && y <= self.y1
    }

    /// Whether this rectangle fully contains `other`.
    fn encloses(&self, other: &Rect) -> bool {
        self.x0 <= other.x0 && self.y0 <= other.y0 && self.x1 >= other.x1 && self.y1 >= other.y1
    }

    /// Area shared with `other`.
    fn overlap_area(&self, other: &Rect) -> f32 {
        let width = (self.x1.min(other.x1) - self.x0.max(other.x0)).max(0.0);
        let height = (self.y1.min(other.y1) - self.y0.max(other.y0)).max(0.0);
        width * height
    }

    /// How far inside the rectangle a point sits, measured to the nearest
    /// edge. Zero on the border and outside.
    fn depth_of(&self, x: f32, y: f32) -> f32 {
        if !self.contains(x, y) {
            return 0.0;
        }
        (x - self.x0).min(self.x1 - x).min(y - self.y0).min(self.y1 - y)
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

/// Upper bound on labels considered. Owner resolution is `labels x outlines`,
/// and a source can hand this a text run for every glyph on the page rather
/// than one per caption; a diagram does not caption more shapes than this.
const MAX_LABELS: usize = 20_000;

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

/// Shapes an outline must enclose before it is read as a container rather than
/// a node. Graphviz clusters, BPMN pools, PlantUML swimlanes and grouping
/// panels all draw a box around their members, and the box is not a node.
///
/// Two, because one enclosed shape is a double border, which
/// [`collapse_concentric`] has already dealt with by this point.
const CONTAINER_MIN_MEMBERS: usize = 2;

/// Text runs an outline must hold before its layout is worth reading as a
/// grid. Two is a wrapped caption; a table has a header row and a body.
const GRID_MIN_CELLS: usize = 3;

/// How far apart two text anchors must be, as a fraction of the shape they sit
/// in, to count as different rows or columns.
const GRID_CLUSTER_RATIO: f32 = 0.1;

/// How much of the smaller of two shapes must be shared before they count as
/// overlapping rather than merely adjacent. Boxes drawn edge to edge, and the
/// rounding slack around them, must not trip it.
const OVERLAP_RATIO: f32 = 0.1;

/// An unlabelled shape below this fraction of the median labelled node's area
/// is decoration rather than a node.
const UNLABELLED_DECORATION_RATIO: f32 = 0.25;

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

/// How far below a shape a caption may sit, in units of the snap tolerance,
/// and still be that shape's name. Shorter than the edge-label reach, because
/// a caption is set tight under the thing it names while an edge label is
/// pushed clear of the line it belongs to.
const CAPTION_REACH: f32 = 3.0;

/// Absolute ceiling on that reach. The gap under a caption is set in text, so
/// it scales with the type size and not with the drawing: on a poster-sized
/// canvas three snap tolerances is over an inch, and everything within an inch
/// below a shape would become its name.
const CAPTION_CEILING: f32 = 48.0;

/// Minimum number of parallel, same-style strokes before a group is read as
/// chart gridlines rather than a coincidence. A chain of linked shapes
/// routinely has two edges running in the same direction; three sharing a
/// colour, a dash style, a spacing *and* a span besides is what a plotting
/// library draws for its scale, and nothing else a diagram draws does all
/// four at once.
const GRIDLINE_MIN_COUNT: usize = 3;

/// How far a stroke's own two ends may differ, on the axis it is meant to run
/// square to, and still count as axis-aligned. A gridline is drawn dead
/// straight; this only has to absorb floating point noise carried through a
/// transform, not a genuinely diagonal or curved connector.
const GRIDLINE_AXIS_EPSILON: f32 = 0.75;

/// How far the gap between two adjacent members of a candidate gridline
/// family may deviate from the family's own average gap, as a fraction of
/// that average, and still count as "regular". A plotting library spaces
/// gridlines by a fixed data interval, so real ones are near-exact; this only
/// has to absorb rounding in the source file.
const GRIDLINE_SPACING_TOLERANCE_RATIO: f32 = 0.15;

/// Minimum number of shapes an axis-aligned stroke must lie flush against,
/// each for more than a single point of contact, before it is read as a
/// chart's own axis or frame line rather than a connector. See
/// [`find_frame_lines`]. Shares [`GRIDLINE_MIN_COUNT`]'s value and rationale:
/// three is the smallest count a coincidence cannot plausibly explain, and
/// nothing a layout engine draws lines a connector up against the shared
/// border of that many shapes it does not join.
const FRAME_MIN_FLUSH_EDGES: usize = GRIDLINE_MIN_COUNT;

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

    // Capped for the same reason as outlines and connectors: a source can hand
    // this a text run per glyph rather than per caption, and owner resolution
    // below is `labels x outlines`.
    let mut labels: Vec<Label> = labels.into_iter().take(MAX_LABELS).collect();
    labels.sort_by(|a, b| a.y.total_cmp(&b.y).then(a.x.total_cmp(&b.x)));

    // Text belongs to the innermost shape that contains its anchor: a label
    // inside a box that is itself inside a panel names the box.
    let mut owners: Vec<Option<usize>> = labels
        .iter()
        .map(|label| {
            kept.iter()
                .enumerate()
                .filter(|(_, o)| o.bbox.contains(label.x, label.y))
                .min_by(|(_, a), (_, b)| a.bbox.area().total_cmp(&b.bbox.area()))
                .map(|(i, _)| i)
        })
        .collect();

    // Chart chrome, not connectors: a plotting library's gridlines are a
    // family of parallel, same-style strokes at a regular interval, each
    // crossing the same span of the plot; its axis and outer frame are drawn
    // as a single stroke lying flush with the shared edge of the shapes it
    // scales, with no peer of its own style to be caught by the family test.
    // Both are dropped here, before arrowhead detection and every distance
    // test below, so neither can be mistaken for a connector.
    let connectors: Vec<Connector> = connectors.into_iter().take(MAX_CONNECTORS).collect();
    let is_gridline = find_gridlines(&connectors, snap);
    let is_frame = find_frame_lines(&kept, &connectors, snap);
    let connectors: Vec<Connector> = connectors
        .into_iter()
        .zip(is_gridline.into_iter().zip(is_frame))
        .filter_map(|(connector, (gridline, frame_line))| (!gridline && !frame_line).then_some(connector))
        .collect();
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

    // A grid is an interior layout, so it is judged on the text drawn inside
    // the shape and never on a caption adopted from outside it below: three
    // lines of one wrapped caption are rows, and their anchors differ enough in
    // x to read as columns too, which would condemn the shape as a table.
    let mut contained: Vec<Vec<&Label>> = vec![Vec::new(); kept.len()];
    for (label, owner) in labels.iter().zip(&owners) {
        if let Some(index) = owner {
            contained[*index].push(label);
        }
    }
    let grids: Vec<bool> = kept
        .iter()
        .zip(&contained)
        .map(|(outline, texts)| holds_a_grid(&outline.bbox, texts))
        .collect();

    // Which connector endpoints land on each shape, with the arrowhead reach
    // the edge builder uses. Computed once: the container test asks it of every
    // enclosed shape, and recomputing it per enclosure-member pair would make
    // classification quadratic in outlines and linear in connectors at once.
    let endpoints: Vec<((f32, f32), f32)> = connectors
        .iter()
        .flat_map(|c| {
            [
                (c.start, snap + touching_arrowhead(&reach, c.start, snap).unwrap_or(0.0)),
                (c.end, snap + touching_arrowhead(&reach, c.end, snap).unwrap_or(0.0)),
            ]
        })
        .collect();
    let landings: Vec<Vec<(f32, f32)>> = kept
        .iter()
        .map(|outline| {
            endpoints
                .iter()
                .filter(|(point, tolerance)| outline.bbox.distance_to(point.0, point.1) <= *tolerance)
                .map(|(point, _)| *point)
                .collect()
        })
        .collect();

    // An icon node has no outline to put its caption inside, so the caption
    // sits underneath the glyph. Adopting it here, after arrowheads are known,
    // keeps a head from acquiring a name it never had.
    adopt_captions(&kept, &labels, &arrowheads, &connectors, &mut owners, snap);

    // The labels each candidate owns, which is what separates a node from the
    // two things that look most like one: a table and a piece of decoration.
    let mut owned: Vec<Vec<&Label>> = vec![Vec::new(); kept.len()];
    for (label, owner) in labels.iter().zip(&owners) {
        if let Some(index) = owner {
            owned[*index].push(label);
        }
    }

    // Containers can only be told apart from real nodes once labels and
    // arrowheads are both known: a container is an enclosure nobody labelled,
    // and its enclosed members exclude arrowheads, which sit inside a node's
    // own border without making it one.
    let is_container = find_containers(&kept, &arrowheads, &owned, &landings, snap);

    // Named nodes are nodes whatever their geometry does, so both tests below
    // only ever condemn a shape nobody labelled.
    //
    // Overlap: a run of anonymous shapes lying on top of each other is one
    // drawing rather than several nodes. The wedges of a pie all meet at its
    // centre, and the parts of an icon glyph all sit inside its footprint,
    // where a layout engine keeps real nodes apart.
    //
    // Enclosure is excluded, because it is the one way two shapes can overlap
    // completely and still be two things: a container holds its members, and a
    // member sitting wholly inside its cluster overlaps it by the member's
    // whole area. Counting that would condemn every unlabelled node in a
    // cluster, which is the opposite of what grouping means. Partial overlap,
    // which is what a pie or a glyph draws, still condemns.
    let overlapping: Vec<bool> = kept
        .iter()
        .enumerate()
        .map(|(i, outline)| {
            kept.iter().enumerate().any(|(j, other)| {
                i != j
                    && !other.bbox.encloses(&outline.bbox)
                    && !outline.bbox.encloses(&other.bbox)
                    && outline.bbox.overlap_area(&other.bbox)
                        > outline.bbox.area().min(other.bbox.area()) * OVERLAP_RATIO
            })
        })
        .collect();

    // Size: an anonymous shape much smaller than the shapes that are labelled
    // is decoration, such as a PlantUML terminal dot or a leader bullet. Size
    // carries this rather than the missing label alone, because a shape the
    // same size as its labelled neighbours is a node whether or not anyone
    // named it.
    let decoration_cutoff = median_area(
        kept.iter()
            .enumerate()
            .filter(|(i, _)| !arrowheads[*i] && !grids[*i] && !owned[*i].is_empty())
            .map(|(_, o)| o.bbox.area()),
    )
    .map(|median| median * UNLABELLED_DECORATION_RATIO);

    // Arrowheads are geometry belonging to the connector that ends in them, not
    // shapes in their own right, so they never become nodes.
    let node_indices: Vec<usize> = (0..kept.len())
        .filter(|i| {
            let anonymous_decoration = owned[*i].is_empty()
                && (overlapping[*i] || decoration_cutoff.is_some_and(|cutoff| kept[*i].bbox.area() < cutoff));
            !arrowheads[*i] && !grids[*i] && !is_container[*i] && !anonymous_decoration
        })
        .collect();
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
        // A connector returning to the shape it left is a self-loop, but only
        // if it actually went somewhere: a real one arcs clear of the node so
        // it can be seen, while a stroke lying entirely within a shape is
        // interior detail, a divider or a glyph, and joins nothing. ~keep
        if from == to
            && node_outlines[from]
                .bbox
                .contains(connector.midpoint.0, connector.midpoint.1)
        {
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
/// Median of a set of areas, or `None` when the set is empty.
fn median_area(areas: impl Iterator<Item = f32>) -> Option<f32> {
    let mut sorted: Vec<f32> = areas.filter(|a| a.is_finite()).collect();
    sorted.sort_by(f32::total_cmp);
    sorted.get(sorted.len() / 2).copied()
}

/// Whether an outline's text is laid out as a grid, which makes it a table
/// rather than a node.
///
/// The discriminator is rows *and* columns together, because each on its own is
/// something a real node does. A node's caption wraps onto several lines, which
/// is many rows in one column. A Graphviz record divides one node into fields
/// side by side, which is one row in many columns. Only a table is both, and a
/// ruled table sitting on the same page as a diagram is otherwise a perfect
/// node: one closed rectangle with text inside it.
fn holds_a_grid(bbox: &Rect, texts: &[&Label]) -> bool {
    if texts.len() < GRID_MIN_CELLS {
        return false;
    }
    let rows = cluster_count(texts.iter().map(|t| t.y), bbox.height() * GRID_CLUSTER_RATIO);
    let columns = cluster_count(texts.iter().map(|t| t.x), bbox.width() * GRID_CLUSTER_RATIO);
    rows > 1 && columns > 1
}

/// How many distinct values a set of coordinates falls into, given how far
/// apart two of them must be to count as distinct.
fn cluster_count(values: impl Iterator<Item = f32>, tolerance: f32) -> usize {
    let mut sorted: Vec<f32> = values.filter(|v| v.is_finite()).collect();
    sorted.sort_by(f32::total_cmp);
    let mut clusters = 0;
    let mut current = f32::NEG_INFINITY;
    for value in sorted {
        if clusters == 0 || value - current > tolerance.max(f32::EPSILON) {
            clusters += 1;
        }
        current = value;
    }
    clusters
}

/// Mark the outlines that group other shapes rather than being shapes
/// themselves.
///
/// A Graphviz cluster, a BPMN pool and a PlantUML swimlane are all a box drawn
/// around their members with no label of their own attached to the box: a
/// caption on the rim, if there is one, sits closer to the members than to
/// anything else and is theirs, not the container's. Left in, a container both
/// invents a node and steals the edges: a connector running between two boxes
/// inside a cluster reaches the cluster's border first, so it lands there
/// instead of on the box it was drawn to.
///
/// A node's own incoming arrowheads are drawn just inside its border and
/// satisfy enclosure exactly as a real member would, so they are excluded from
/// the count: two arrowheads landing inside an unlabelled node are not two
/// members grouped by it.
///
/// A label on the enclosure is not on its own enough to make it a node. Every
/// renderer that draws clusters captions them — Graphviz puts the cluster's
/// name inside the border above its members, PlantUML and BPMN put the lane or
/// pool name in a header band — and that caption belongs to no member, so the
/// enclosure owns it. What separates such a caption from a UML class box's own
/// name, or a BPMN task's, is what the enclosure holds: a class box draws
/// compartments and markers, which are interior detail nothing connects to,
/// while a cluster groups nodes that the diagram's own connectors land on.
/// So a labelled enclosure is a container only when the shapes it encloses are
/// themselves joined to the graph.
///
/// "Joined" has to mean joined *within* the enclosure, or a class box would
/// fail the test for the wrong reason: its compartments run the full width, so
/// an association arriving at the class's own border is at zero distance from
/// whichever compartment reaches that height. Requiring the endpoint to sit
/// clear of the enclosure's border by the snap tolerance separates the two —
/// a cluster's members are joined well inside it, while everything touching a
/// class box happens on its rim.
///
/// An unlabelled enclosure keeps the older, unconditional rule. Not because
/// the argument above stops applying, but because it has nothing else it could
/// be: a shape that groups two others and never says what it is has no claim
/// to be a node, whereas a labelled one does.
///
/// Enclosure is otherwise the whole test. Nesting is allowed to any depth,
/// since each container is judged against what it directly contains.
fn find_containers(
    outlines: &[Outline],
    arrowheads: &[bool],
    owned: &[Vec<&Label>],
    landings: &[Vec<(f32, f32)>],
    snap: f32,
) -> Vec<bool> {
    outlines
        .iter()
        .enumerate()
        .map(|(i, outer)| {
            let members = outlines
                .iter()
                .enumerate()
                .filter(|(j, inner)| *j != i && !arrowheads[*j] && outer.bbox.encloses(&inner.bbox));
            if owned[i].is_empty() {
                return members.count() >= CONTAINER_MIN_MEMBERS;
            }
            members
                .filter(|(j, _)| {
                    landings
                        .get(*j)
                        .is_some_and(|points| points.iter().any(|(x, y)| outer.bbox.depth_of(*x, *y) > snap))
                })
                .take(CONTAINER_MIN_MEMBERS)
                .count()
                >= CONTAINER_MIN_MEMBERS
        })
        .collect()
}

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

    // Precomputed once so the label test below is a lookup, not a scan of
    // every label for every outline: `label_owners.contains(&Some(index))`
    // repeated inside this loop is `outlines x labels`. ~keep
    let mut labelled = vec![false; outlines.len()];
    for owner in label_owners.iter().flatten() {
        labelled[*owner] = true;
    }

    for (index, outline) in outlines.iter().enumerate() {
        if outline.bbox.area() > largest * ARROWHEAD_AREA_RATIO {
            continue;
        }
        if outline.bbox.width() > max_side || outline.bbox.height() > max_side {
            continue;
        }
        if labelled[index] {
            continue;
        }
        let touches = connectors.iter().any(|c| {
            outline.bbox.distance_to(c.start.0, c.start.1) <= snap || outline.bbox.distance_to(c.end.0, c.end.1) <= snap
        });
        marked[index] = touches;
    }
    marked
}

/// Mark the connectors that are chart gridlines rather than diagram
/// connectors.
///
/// A gridline is not one open stroke that happens to land on two shapes; it
/// is one member of a family. A chart draws its gridlines as a set of
/// parallel strokes, all the same direction, all the same colour and dash
/// style, and spaced apart by the same interval, because that is what a
/// scale is: equal steps. Read in isolation, one gridline is indistinguishable
/// from a connector — that is exactly the GH#1420 defect, where the two ends
/// of one such stroke land on a chart's first and last bar and read as an
/// edge between them. What tells the family apart from a diagram is the
/// other members: nothing a layout engine draws puts three-or-more
/// same-coloured, same-dashed, axis-aligned strokes at a regular spacing
/// that all cover the same span of the canvas.
///
/// A straightforward "does this stroke cross a shape's interior" rule was
/// tried and rejected (see GH#1420): it costs real edges in `graphviz_large`
/// and `mermaid_flow`, and breaks the two tests that deliberately let a
/// connector pass through, or start deep inside, a third shape. The
/// regularity of the *family* is the part a real connector never
/// coincidentally reproduces, so it is what this checks instead of anything
/// about where a single stroke sits relative to a shape.
///
/// A vertical (or horizontal) chain of linked shapes is the closest a real
/// diagram comes to looking like this: consecutive edges in the same
/// direction, evenly spaced because the nodes are. `is_gridline_family`
/// separates the two cases on span: a chain's edges each run only the short,
/// non-overlapping distance between one pair of nodes, while gridlines all
/// cross the same plot and so all cover the same ground. A chain drawn
/// perfectly straight also shares its run-axis position exactly across every
/// edge (they are collinear), which the spacing check rejects outright,
/// since gridlines are offset from each other on purpose.
fn find_gridlines(connectors: &[Connector], snap: f32) -> Vec<bool> {
    let mut marked = vec![false; connectors.len()];

    // Horizontal and vertical are judged separately: a family only ever runs
    // one direction, and a diagonal or curved stroke belongs to neither.
    for horizontal in [true, false] {
        let mut groups: HashMap<(String, bool), Vec<usize>> = HashMap::new();
        for (index, connector) in connectors.iter().enumerate() {
            let axis_aligned = if horizontal {
                (connector.start.1 - connector.end.1).abs() <= GRIDLINE_AXIS_EPSILON
            } else {
                (connector.start.0 - connector.end.0).abs() <= GRIDLINE_AXIS_EPSILON
            };
            if !axis_aligned {
                continue;
            }
            let key = (connector.stroke.clone().unwrap_or_default(), connector.dashed);
            groups.entry(key).or_default().push(index);
        }

        for indices in groups.values() {
            if indices.len() >= GRIDLINE_MIN_COUNT && is_gridline_family(connectors, indices, horizontal, snap) {
                for &index in indices {
                    marked[index] = true;
                }
            }
        }
    }

    marked
}

/// Whether a same-direction, same-style group of connectors is a gridline
/// family. See [`find_gridlines`] for why span and spacing together are the
/// test.
fn is_gridline_family(connectors: &[Connector], indices: &[usize], horizontal: bool, snap: f32) -> bool {
    let mut axis_positions = Vec::with_capacity(indices.len());
    let mut run_starts = Vec::with_capacity(indices.len());
    let mut run_ends = Vec::with_capacity(indices.len());
    for &index in indices {
        let connector = &connectors[index];
        let (axis, run_a, run_b) = if horizontal {
            (
                (connector.start.1 + connector.end.1) / 2.0,
                connector.start.0,
                connector.end.0,
            )
        } else {
            (
                (connector.start.0 + connector.end.0) / 2.0,
                connector.start.1,
                connector.end.1,
            )
        };
        axis_positions.push(axis);
        run_starts.push(run_a.min(run_b));
        run_ends.push(run_a.max(run_b));
    }

    // A real connector stops at the shapes it joins, so a chain's successive
    // edges cover different, non-overlapping ground. Gridlines all cross the
    // same plot, so every member covers the same ground the family's own
    // snap tolerance considers "the same place".
    if spread(run_starts.iter().copied()) > snap || spread(run_ends.iter().copied()) > snap {
        return false;
    }

    // Offset from each other, and by a consistent amount. A connector chain
    // running straight down (or across) a page shares this axis position
    // exactly, since its edges are collinear, so a gap at or below the snap
    // tolerance here means "chain", not "gridlines": gridlines are spread
    // apart on purpose, by however much the scale's interval is.
    let mut sorted = axis_positions;
    sorted.sort_by(f32::total_cmp);
    let gaps: Vec<f32> = sorted.windows(2).map(|pair| pair[1] - pair[0]).collect();
    if gaps.iter().any(|gap| *gap <= snap) {
        return false;
    }
    let average = gaps.iter().sum::<f32>() / gaps.len() as f32;
    gaps.iter()
        .all(|gap| (gap - average).abs() <= average * GRIDLINE_SPACING_TOLERANCE_RATIO)
}

/// Mark the connectors that are a chart's own axis or frame line rather than a
/// diagram connector.
///
/// A gridline never travels alone; [`find_gridlines`] catches those by their
/// peers sharing style, span and spacing. A plot's axis and outer frame do
/// not need a peer to be chart furniture: a plotting library draws the
/// x-axis as one stroke running the width of the plot, flush with every
/// bar's baseline, and the y-axis as one stroke flush with the plot's left
/// edge, and each is typically the only stroke drawn in its own colour and
/// orientation, so the family test never sees three of them to group.
///
/// What gives such a stroke away instead is what it lies against: it is
/// collinear with the same boundary edge of several shapes at once, and for
/// a real stretch of each one's own span, not a single point where two
/// unrelated things happen to meet. A real connector's endpoints touch at
/// most the two shapes it joins, each at a point; nothing a layout engine
/// draws lines a connector up flush against the shared edge of three or more
/// shapes it does not connect. This is deliberately narrower than testing
/// whether a stroke crosses a shape's interior — that was tried for GH#1420
/// and rejected, since it costs real edges that legitimately pass near or
/// through a third shape on their way between the two they join. Requiring
/// the stroke to run *along a shared boundary*, not merely *near* several
/// shapes, is what a real connector never does by coincidence.
fn find_frame_lines(outlines: &[Outline], connectors: &[Connector], snap: f32) -> Vec<bool> {
    connectors
        .iter()
        .map(|connector| is_frame_line(outlines, connector, snap))
        .collect()
}

/// Whether a single axis-aligned connector lies flush with the same boundary
/// edge of at least [`FRAME_MIN_FLUSH_EDGES`] shapes. See [`find_frame_lines`].
fn is_frame_line(outlines: &[Outline], connector: &Connector, snap: f32) -> bool {
    let horizontal = (connector.start.1 - connector.end.1).abs() <= GRIDLINE_AXIS_EPSILON;
    let vertical = (connector.start.0 - connector.end.0).abs() <= GRIDLINE_AXIS_EPSILON;
    if !horizontal && !vertical {
        return false;
    }

    // A connector that is (near enough) both a point and axis-aligned in both
    // senses is judged as horizontal; which axis a near-zero-length stroke is
    // read against does not change the answer.
    let (axis, run_min, run_max) = if horizontal {
        (
            (connector.start.1 + connector.end.1) / 2.0,
            connector.start.0.min(connector.end.0),
            connector.start.0.max(connector.end.0),
        )
    } else {
        (
            (connector.start.0 + connector.end.0) / 2.0,
            connector.start.1.min(connector.end.1),
            connector.start.1.max(connector.end.1),
        )
    };

    let flush_edges = outlines
        .iter()
        .filter(|outline| {
            let (near, far, span_min, span_max) = if horizontal {
                (outline.bbox.y0, outline.bbox.y1, outline.bbox.x0, outline.bbox.x1)
            } else {
                (outline.bbox.x0, outline.bbox.x1, outline.bbox.y0, outline.bbox.y1)
            };
            let on_boundary = (near - axis).abs() <= snap || (far - axis).abs() <= snap;
            let overlap = (span_max.min(run_max) - span_min.max(run_min)).max(0.0);
            on_boundary && overlap > snap
        })
        .count();

    flush_edges >= FRAME_MIN_FLUSH_EDGES
}

/// Distance between the smallest and largest value in a set of coordinates.
/// Zero for an empty set, so an empty group never looks like a match by
/// default.
fn spread(values: impl Iterator<Item = f32>) -> f32 {
    let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
    for value in values {
        min = min.min(value);
        max = max.max(value);
    }
    if min.is_finite() && max.is_finite() {
        max - min
    } else {
        0.0
    }
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

/// Give an unlabelled shape the caption drawn beneath it.
///
/// The AWS and Azure architecture house style draws a node as an icon glyph
/// with its name underneath and no outline at all, so the name lies outside
/// every closed region in the drawing and containment finds it no owner.
///
/// Three things stop this from eating text that names something else. It only
/// considers a label containment left unowned, so nothing is taken from the
/// shape it sits inside. It only offers that label to a shape holding no text
/// of its own, so an annotation under a captioned box — an org chart's
/// headcount under a department — stays free, because the department already
/// says what it is. And the anchor must fall within the shape's own width and
/// just below its lower edge, which is where a caption goes and where an
/// unrelated line of prose does not.
fn adopt_captions(
    outlines: &[Outline],
    labels: &[Label],
    arrowheads: &[bool],
    connectors: &[Connector],
    owners: &mut [Option<usize>],
    snap: f32,
) {
    // Frozen before any adoption, so that two lines of one wrapped caption both
    // reach the same shape and the result does not depend on label order.
    let mut labelled = vec![false; outlines.len()];
    for owner in owners.iter().flatten() {
        labelled[*owner] = true;
    }

    let reach = (snap * CAPTION_REACH).min(CAPTION_CEILING);
    for (label, owner) in labels.iter().zip(owners.iter_mut()) {
        if owner.is_some() {
            continue;
        }
        let Some((index, gap)) = outlines
            .iter()
            .enumerate()
            .filter(|(i, outline)| {
                !labelled[*i]
                    && !arrowheads[*i]
                    && label.x >= outline.bbox.x0
                    && label.x <= outline.bbox.x1
                    && label.y > outline.bbox.y1
                    && label.y - outline.bbox.y1 <= reach
            })
            // Nearest above the caption, and on a tie the shape it is centred
            // under. A banner or rule spanning a row of icons sits below all of
            // them and would otherwise take every caption in the row.
            .min_by(|(_, a), (_, b)| {
                (label.y - a.bbox.y1).total_cmp(&(label.y - b.bbox.y1)).then(
                    (label.x - a.bbox.centre_x())
                        .abs()
                        .total_cmp(&(label.x - b.bbox.centre_x()).abs()),
                )
            })
            .map(|(i, outline)| (i, label.y - outline.bbox.y1))
        else {
            continue;
        };

        // Text under one node and over the next is either this node's caption
        // or that connector's label, and the same geometry describes both. The
        // nearer claim wins; without this the caption test runs first and takes
        // the label even when the connector is an order of magnitude closer.
        if connectors.iter().any(|connector| {
            let dx = label.x - connector.midpoint.0;
            let dy = label.y - connector.midpoint.1;
            dx.hypot(dy) < gap
        }) {
            continue;
        }

        *owner = Some(index);
    }
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

    fn styled_connector(start: (f32, f32), end: (f32, f32), stroke: &str) -> Connector {
        Connector {
            stroke: Some(stroke.to_string()),
            ..connector(start, end)
        }
    }

    fn label(x: f32, y: f32, text: &str) -> Label {
        Label {
            x,
            y,
            text: text.to_string(),
        }
    }

    /// Landing points for a drawing whose connectors reach none of its shapes.
    fn nowhere(outlines: usize) -> Vec<Vec<(f32, f32)>> {
        vec![Vec::new(); outlines]
    }

    /// A connector leaving a node and returning to it is a self-loop, and the
    /// arc has to bulge clear of the node or nobody could see it.
    #[test]
    fn a_self_loop_arcing_clear_of_its_node_is_an_edge() {
        let mut loop_back = connector((30.0, 200.0), (70.0, 200.0));
        loop_back.midpoint = (50.0, 280.0);

        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![outline(0.0, 0.0, 100.0, 50.0), outline(0.0, 200.0, 100.0, 250.0)],
            vec![connector((50.0, 50.0), (50.0, 200.0)), loop_back],
            Vec::new(),
        )
        .expect("graph");

        let self_loop = graph.edges.iter().find(|e| e.from == e.to);
        assert!(self_loop.is_some(), "a self-loop is an edge: {:?}", graph.edges);
        assert_eq!(self_loop.expect("checked").from, 1);
    }

    /// A ruled table on the same page as a diagram is one closed rectangle with
    /// text in it, which is a node in every respect except that its text is
    /// laid out in rows *and* columns.
    #[test]
    fn a_box_holding_a_grid_of_text_is_a_table_not_a_node() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 200.0, 100.0, 250.0),
                outline(200.0, 0.0, 380.0, 100.0),
            ],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            vec![
                label(240.0, 20.0, "Stage"),
                label(320.0, 20.0, "Owner"),
                label(240.0, 70.0, "Build"),
                label(320.0, 70.0, "Ada"),
            ],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2, "the table is not a node: {:?}", graph.nodes);
    }

    /// A caption wrapped onto several lines is many rows in one column, which
    /// is not a grid however many lines it runs to.
    #[test]
    fn a_wrapped_caption_is_not_a_grid() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![outline(0.0, 0.0, 100.0, 50.0), outline(0.0, 200.0, 100.0, 250.0)],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            vec![
                label(50.0, 210.0, "Release"),
                label(50.0, 225.0, "engineer"),
                label(50.0, 240.0, "on call"),
            ],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.nodes[1].label, "Release\nengineer\non call");
    }

    /// Pie wedges all meet at the centre, so their boxes overlap. Nothing a
    /// layout engine draws does that, and none of them is named.
    #[test]
    fn overlapping_anonymous_shapes_are_one_drawing_not_nodes() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 200.0, 100.0, 250.0),
                outline(200.0, 100.0, 300.0, 200.0),
                outline(210.0, 110.0, 310.0, 210.0),
            ],
            vec![connector((50.0, 50.0), (50.0, 200.0))],
            vec![label(50.0, 25.0, "from"), label(50.0, 225.0, "to")],
        )
        .expect("graph");

        assert_eq!(
            graph.nodes.len(),
            2,
            "overlapping wedges are not nodes: {:?}",
            graph.nodes
        );
    }

    /// Overlap only condemns a shape nobody named. Two labelled nodes placed
    /// close enough to overlap are still two nodes.
    #[test]
    fn overlapping_labelled_shapes_are_still_nodes() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![outline(0.0, 0.0, 100.0, 100.0), outline(60.0, 60.0, 160.0, 160.0)],
            vec![connector((50.0, 50.0), (110.0, 110.0))],
            vec![label(20.0, 20.0, "left"), label(140.0, 140.0, "right")],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2);
    }

    /// A terminal dot beside labelled activities is decoration, but a shape the
    /// size of its labelled neighbours is a node even unnamed.
    #[test]
    fn a_tiny_unlabelled_shape_beside_labelled_ones_is_decoration() {
        let graph = assemble(
            None,
            (400.0, 400.0),
            vec![
                outline(0.0, 0.0, 100.0, 50.0),
                outline(0.0, 200.0, 100.0, 250.0),
                outline(40.0, 300.0, 52.0, 312.0),
            ],
            vec![
                connector((50.0, 50.0), (50.0, 200.0)),
                connector((50.0, 250.0), (46.0, 300.0)),
            ],
            vec![label(50.0, 25.0, "from"), label(50.0, 225.0, "to")],
        )
        .expect("graph");

        assert_eq!(graph.nodes.len(), 2, "the dot is decoration: {:?}", graph.nodes);
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
    fn an_enclosing_panel_is_a_container_not_a_node() {
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

        // The panel groups the two boxes, so it is a container: reporting it
        // would both invent a node and give the connector a third thing to
        // land on. It is still not a double border, which is what would
        // happen if it were merged into the shape it encloses.
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
    }

    // The tests below call `find_containers` directly rather than through
    // `assemble`, so that each states one classification rule and fails for one
    // reason. `assemble` reaches the same verdicts, and
    // `an_enclosing_panel_is_a_container_not_a_node` covers it end to end. ~keep

    /// Two of a node's own incoming arrowheads, drawn just inside its border,
    /// satisfy `Rect::encloses` exactly as two real members would. They must
    /// not count: excluding them leaves zero members, short of the container
    /// threshold.
    #[test]
    fn a_container_test_excludes_arrowheads_from_its_member_count() {
        let outlines = vec![
            outline(0.0, 0.0, 100.0, 100.0),
            outline(5.0, 5.0, 15.0, 15.0),
            outline(80.0, 80.0, 90.0, 90.0),
        ];
        let arrowheads = vec![false, true, true];
        let owned: Vec<Vec<&Label>> = vec![Vec::new(), Vec::new(), Vec::new()];

        assert_eq!(
            find_containers(&outlines, &arrowheads, &owned, &nowhere(3), 4.0),
            vec![false, false, false],
            "two arrowheads inside a node's border are not two members grouped by it"
        );
    }

    /// A UML class box or a BPMN task carries its own label alongside the
    /// compartments or markers it draws inside itself. Nothing connects to
    /// those, so the enclosure is a node.
    #[test]
    fn a_labelled_enclosure_of_unconnected_detail_is_a_node() {
        let outlines = vec![
            outline(0.0, 0.0, 100.0, 100.0),
            outline(10.0, 10.0, 40.0, 40.0),
            outline(60.0, 60.0, 90.0, 90.0),
        ];
        let arrowheads = vec![false, false, false];
        let caption = label(50.0, 50.0, "ClassName");
        let owned: Vec<Vec<&Label>> = vec![vec![&caption], Vec::new(), Vec::new()];

        assert_eq!(
            find_containers(&outlines, &arrowheads, &owned, &nowhere(3), 4.0),
            vec![false, false, false],
            "compartments nothing connects to are interior detail, not grouped members"
        );
    }

    /// A Graphviz cluster is captioned inside its own border, so it owns a
    /// label exactly as a class box does. What separates them is that its
    /// members carry the diagram's connectors.
    #[test]
    fn a_labelled_enclosure_grouping_connected_shapes_is_a_container() {
        let outlines = vec![
            outline(0.0, 0.0, 100.0, 100.0),
            outline(10.0, 10.0, 40.0, 40.0),
            outline(60.0, 60.0, 90.0, 90.0),
        ];
        let arrowheads = vec![false, false, false];
        let caption = label(50.0, 5.0, "Ingest");
        let owned: Vec<Vec<&Label>> = vec![vec![&caption], Vec::new(), Vec::new()];
        // One connector joining the two members, landing on each well inside
        // the enclosure rather than on its rim.
        let landings = vec![Vec::new(), vec![(25.0, 40.0)], vec![(75.0, 60.0)]];

        assert_eq!(
            find_containers(&outlines, &arrowheads, &owned, &landings, 4.0),
            vec![true, false, false],
            "a captioned box grouping two connected shapes is a cluster, not a node"
        );
    }

    /// A class box's compartments run its full width, so an association
    /// arriving at the class's own border is at zero distance from whichever
    /// compartment reaches that height. Landing on the rim is not being
    /// grouped, or every UML class in the world becomes a container.
    #[test]
    fn a_labelled_enclosure_whose_members_are_touched_only_at_its_rim_is_a_node() {
        let outlines = vec![
            outline(0.0, 0.0, 100.0, 100.0),
            outline(0.0, 10.0, 100.0, 40.0),
            outline(0.0, 60.0, 100.0, 90.0),
        ];
        let arrowheads = vec![false, false, false];
        let caption = label(50.0, 5.0, "Order");
        let owned: Vec<Vec<&Label>> = vec![vec![&caption], Vec::new(), Vec::new()];
        // Two associations arriving on the class's left border, each landing on
        // a different full-width compartment.
        let landings = vec![Vec::new(), vec![(0.0, 20.0)], vec![(0.0, 70.0)]];

        assert_eq!(
            find_containers(&outlines, &arrowheads, &owned, &landings, 4.0),
            vec![false, false, false],
            "compartments touched on the enclosure's rim are interior detail"
        );
    }

    /// The baseline the two tests above are contrasted with: an unlabelled
    /// enclosure of two ordinary (non-arrowhead) shapes is still a container.
    #[test]
    fn a_container_test_still_condemns_an_unlabelled_enclosure_of_two_real_shapes() {
        let outlines = vec![
            outline(0.0, 0.0, 100.0, 100.0),
            outline(10.0, 10.0, 40.0, 40.0),
            outline(60.0, 60.0, 90.0, 90.0),
        ];
        let arrowheads = vec![false, false, false];
        let owned: Vec<Vec<&Label>> = vec![Vec::new(), Vec::new(), Vec::new()];

        assert_eq!(
            find_containers(&outlines, &arrowheads, &owned, &nowhere(3), 4.0),
            vec![true, false, false]
        );
    }

    #[test]
    fn a_stroke_inside_one_shape_is_not_a_self_loop() {
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

    /// GH#1420: a bar chart's gridlines, drawn across the whole plot, land
    /// exactly on the first and last bar and read as a connector between
    /// them unless the family they belong to is recognised as chrome. Four
    /// bars, no genuine connector between any of them, and three gridlines
    /// sharing one colour, evenly spaced and each spanning the same width —
    /// nothing here should survive as a graph at all.
    #[test]
    fn bar_chart_gridlines_crossing_the_bars_are_not_a_graph() {
        let bars = vec![
            outline(120.0, 180.0, 220.0, 380.0),
            outline(250.0, 240.0, 350.0, 380.0),
            outline(380.0, 130.0, 480.0, 380.0),
            outline(510.0, 210.0, 610.0, 380.0),
        ];
        let gridlines = vec![
            styled_connector((120.0, 300.0), (620.0, 300.0), "#cccccc"),
            styled_connector((120.0, 220.0), (620.0, 220.0), "#cccccc"),
            styled_connector((120.0, 140.0), (620.0, 140.0), "#cccccc"),
        ];

        assert!(
            assemble(None, (700.0, 450.0), bars, gridlines, Vec::new()).is_none(),
            "chart gridlines that cross the bars must not read as a graph"
        );
    }

    /// The gridline family is removed on sight, not just when it is the only
    /// thing present: a real connector elsewhere in the drawing, in a
    /// different colour and direction, survives the same pass untouched.
    #[test]
    fn gridlines_are_dropped_but_a_genuine_edge_beside_them_survives() {
        let bars = vec![
            outline(120.0, 180.0, 220.0, 380.0),
            outline(250.0, 240.0, 350.0, 380.0),
            outline(380.0, 130.0, 480.0, 380.0),
            outline(510.0, 210.0, 610.0, 380.0),
        ];
        let extra_nodes = vec![outline(630.0, 0.0, 700.0, 50.0), outline(630.0, 60.0, 700.0, 110.0)];
        let gridlines = vec![
            styled_connector((120.0, 300.0), (620.0, 300.0), "#cccccc"),
            styled_connector((120.0, 220.0), (620.0, 220.0), "#cccccc"),
            styled_connector((120.0, 140.0), (620.0, 140.0), "#cccccc"),
        ];
        let real_edge = styled_connector((665.0, 50.0), (665.0, 60.0), "#333333");

        let mut connectors = gridlines;
        connectors.push(real_edge);
        let mut outlines = bars;
        outlines.extend(extra_nodes);

        let graph = assemble(None, (700.0, 450.0), outlines, connectors, Vec::new()).expect("the real edge survives");

        assert_eq!(
            graph.edges.len(),
            1,
            "only the non-gridline connector is an edge: {:?}",
            graph.edges
        );
        assert_eq!(graph.edges[0].stroke.as_deref(), Some("#333333"));
    }

    /// A straight top-to-bottom chain of linked shapes is the case a naive
    /// "regularly spaced, same-direction, same-style strokes" rule would
    /// wrongly condemn: consecutive edges in a vertical flowchart are
    /// axis-aligned, share a colour and are evenly spaced because the nodes
    /// are. What tells them apart from gridlines is span (each edge here
    /// covers only the short run between its own two nodes) and collinearity
    /// (they share their run-axis position exactly, which gridlines never
    /// do). All three edges must survive with their exact endpoints.
    #[test]
    fn a_straight_vertical_chain_is_not_mistaken_for_gridlines() {
        let nodes = vec![
            outline(0.0, 0.0, 100.0, 50.0),
            outline(0.0, 100.0, 100.0, 150.0),
            outline(0.0, 200.0, 100.0, 250.0),
            outline(0.0, 300.0, 100.0, 350.0),
        ];
        let links = vec![
            styled_connector((50.0, 50.0), (50.0, 100.0), "#000000"),
            styled_connector((50.0, 150.0), (50.0, 200.0), "#000000"),
            styled_connector((50.0, 250.0), (50.0, 300.0), "#000000"),
        ];

        let graph = assemble(None, (400.0, 400.0), nodes, links, Vec::new()).expect("a real chain is a graph");

        let edges: Vec<(usize, usize)> = graph.edges.iter().map(|e| (e.from, e.to)).collect();
        assert_eq!(
            edges,
            vec![(0, 1), (1, 2), (2, 3)],
            "a real vertical chain loses no edges"
        );
    }
}
