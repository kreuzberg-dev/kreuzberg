//! Diagram graph recovered from a vector source.
//!
//! Vector diagrams (SVG today, vector PDF and DrawingML later) already contain
//! the node/edge structure that raster diagram understanding has to infer with a
//! detection model. This module is the format-independent shape that recovery
//! produces and that the `dot` renderer consumes.
//!
//! Only structural and styling facts live here. Geometry stays inside the
//! recovery step: it is what decides which shape a label belongs to and which
//! nodes a connector joins, and it has no meaning once those decisions are made.

use serde::{Deserialize, Serialize};

/// Node outline, mapped onto the Graphviz `shape` attribute.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagramShape {
    /// Axis-aligned rectangle, including rounded rectangles.
    Box,
    /// Circle or ellipse.
    Ellipse,
    /// Quadrilateral standing on a vertex.
    Diamond,
    /// Any other closed outline.
    Polygon,
}

impl DiagramShape {
    /// Graphviz `shape` attribute value.
    pub fn as_dot(self) -> &'static str {
        match self {
            Self::Box => "box",
            Self::Ellipse => "ellipse",
            Self::Diamond => "diamond",
            Self::Polygon => "polygon",
        }
    }
}

/// A diagram node: one closed outline plus the text drawn inside it.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramNode {
    /// Stable identifier, `n0`, `n1`, … assigned in reading order.
    pub id: String,

    /// Text found inside the outline, joined with `\n` in reading order.
    /// Empty when the shape carries no text.
    pub label: String,

    /// Outline shape.
    pub shape: DiagramShape,

    /// Fill colour as `#rrggbb`, `None` when the shape is unfilled or filled
    /// with a gradient or pattern rather than a flat colour.
    pub fill: Option<String>,

    /// Stroke colour as `#rrggbb`, under the same restriction as `fill`.
    pub stroke: Option<String>,

    /// Stroke width in source units.
    pub stroke_width: Option<f32>,

    /// Whether the outline is dashed.
    pub dashed: bool,
}

/// A diagram edge: one open connector joining two nodes.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DiagramEdge {
    /// Index into [`DiagramGraph::nodes`] of the connector's start.
    pub from: usize,

    /// Index into [`DiagramGraph::nodes`] of the connector's end.
    pub to: usize,

    /// Whether the connector is drawn with an arrowhead at both ends.
    ///
    /// A single arrowhead sets `from` and `to`, so it needs no flag. When the
    /// source draws no arrowhead at all, the connector's own point order stands
    /// in for direction and this stays `false`.
    pub bidirectional: bool,

    /// Text drawn on the connector, `None` when it carries none.
    pub label: Option<String>,

    /// Stroke colour as `#rrggbb`.
    pub stroke: Option<String>,

    /// Whether the connector is dashed.
    pub dashed: bool,
}

/// A graph recovered from one vector diagram.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DiagramGraph {
    /// Diagram name, taken from the source's own title where it has one.
    pub name: Option<String>,

    /// Nodes in reading order (top to bottom, then left to right).
    pub nodes: Vec<DiagramNode>,

    /// Edges, ordered by `(from, to)`.
    pub edges: Vec<DiagramEdge>,
}
