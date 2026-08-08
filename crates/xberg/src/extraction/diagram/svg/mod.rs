//! Diagram recovery from SVG.
//!
//! Shapes come from `usvg`, which resolves `use`, styles, units and the whole
//! transform chain, so every outline and connector arrives in canvas
//! coordinates already. Text does not: `usvg` is built here without its `text`
//! feature, which needs a font database and drops text elements during
//! conversion. Labels therefore come from a second, small pass over the source
//! XML that reproduces only the part of `usvg` we lost, namely the transform
//! chain down to each `<text>` anchor.

mod geometry;
mod text;

use crate::types::diagram::DiagramGraph;

use geometry::collect_geometry;
use text::TextPass;

/// Maximum input byte length accepted. Matches the cap `core::image_encode`
/// applies before handing an SVG to `usvg`, and for the same reason: usvg
/// expands the source into an in-memory tree synchronously, so a small source
/// with many `<use>` references can cost far more than its byte count suggests.
const MAX_INPUT_BYTES: usize = 10 * 1024 * 1024;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::diagram::DiagramShape;

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
