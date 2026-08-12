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

/// Maximum accepted element nesting depth (`<g>` inside `<g>` inside ...).
///
/// `usvg` 0.48.1's own tree conversion is mutually recursive with no depth
/// limit of its own: `convert_children` calls `convert_element`, which for a
/// `<g>` calls `convert_group` with a closure that calls `convert_element_impl`,
/// which for `EId::G` calls `convert_children` again (verified in
/// `usvg-0.48.1/src/parser/converter.rs`, lines 607-703; every one of those
/// functions carries `#[inline(never)]`, so each level is a real stack frame,
/// not one flattened by inlining). Roughly 10 MB of nested `<g>` elements is
/// about 1.4 million levels, which overflows the stack and kills the process
/// with SIGSEGV rather than returning an `Err` — that recursion runs before
/// any of our code, so it cannot be caught by a counter inside our own
/// traversal; see `exceeds_max_nesting` below, which rejects the input first.
///
/// Real diagrams (Graphviz `dot -Tsvg`, Mermaid, hand-authored flowcharts)
/// nest well under 20 levels. 64 leaves generous headroom above that while
/// keeping the worst case small enough, in both our own recursive traversals
/// (`collect_geometry`, the text pass's element stack) and in whatever
/// remains of usvg's, that depth can never be the limiting factor again.
const MAX_NESTING_DEPTH: usize = 64;

/// Recover a graph from SVG bytes, or `None` when the source is not a diagram.
pub(crate) fn recover(data: &[u8]) -> Option<DiagramGraph> {
    if data.is_empty() || data.len() > MAX_INPUT_BYTES || exceeds_max_nesting(data) {
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
    // `collect_geometry` reports truncation (rather than returning whatever
    // it managed to collect) so that a tree deeper than the raw byte scan
    // predicted - possible via `<use>`/`<symbol>` expansion - never turns
    // into a partial graph presented as a complete one.
    if collect_geometry(tree.root(), &mut outlines, &mut connectors) {
        return None;
    }

    let source = String::from_utf8_lossy(data);
    let text = TextPass::default().run(&source, canvas);

    super::assemble(text.title, canvas, outlines, connectors, text.labels)
}

/// Reject `data` before it reaches `usvg::Tree::from_data` if its element
/// nesting would exceed [`MAX_NESTING_DEPTH`]. `usvg`'s own conversion is
/// unboundedly recursive (see the comment on `MAX_NESTING_DEPTH`), so this
/// has to run first: once `usvg` starts converting, a deeply nested document
/// may already have overflowed the stack.
///
/// Single pass over the raw bytes, no allocation. Tag boundaries are found by
/// tracking quoted attribute values (so a literal `>` inside `d="..."` does
/// not end a tag early); comments, CDATA sections and declarations are
/// skipped whole rather than descended into, since none of them nest
/// elements. This is not a general XML/SVG parser and does not try to be
/// one: it only has to bound the depth `usvg` will see, and returns as soon
/// as the bound is crossed.
fn exceeds_max_nesting(data: &[u8]) -> bool {
    let mut depth: usize = 0;
    let mut index = 0;

    while let Some(open) = find(data, index, b"<") {
        if data[open..].starts_with(b"<!--") {
            index = find(data, open + 4, b"-->").map_or(data.len(), |end| end + 3);
            continue;
        }
        if data[open..].starts_with(b"<![CDATA[") {
            index = find(data, open + 9, b"]]>").map_or(data.len(), |end| end + 3);
            continue;
        }
        if matches!(data.get(open + 1), Some(b'!') | Some(b'?')) {
            // Doctype/declaration/processing-instruction: skip to the next
            // `>`. Neither opens nor closes an element.
            index = find(data, open + 1, b">").map_or(data.len(), |end| end + 1);
            continue;
        }

        let closing = data.get(open + 1) == Some(&b'/');
        let Some(tag_end) = unquoted_tag_end(data, open + 1) else {
            break;
        };
        let self_closing = tag_end > 0 && data.get(tag_end - 1) == Some(&b'/');

        if closing {
            depth = depth.saturating_sub(1);
        } else if !self_closing {
            depth += 1;
            if depth > MAX_NESTING_DEPTH {
                return true;
            }
        }
        index = tag_end + 1;
    }

    false
}

/// Index of the `>` that ends the tag whose name starts at `start`, treating
/// a `>` inside a single- or double-quoted attribute value as ordinary text
/// rather than the tag terminator.
fn unquoted_tag_end(data: &[u8], start: usize) -> Option<usize> {
    let mut quote: Option<u8> = None;
    let mut index = start;
    while index < data.len() {
        let byte = data[index];
        match quote {
            Some(q) if byte == q => quote = None,
            Some(_) => {}
            None if byte == b'"' || byte == b'\'' => quote = Some(byte),
            None if byte == b'>' => return Some(index),
            None => {}
        }
        index += 1;
    }
    None
}

/// First occurrence of `needle` in `data` at or after `from`, or `None` when
/// it does not appear (including when `from` is already past the end).
fn find(data: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > data.len() {
        return None;
    }
    data[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
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

    /// GH#1420's fixture verbatim: a bar chart whose gridlines are drawn
    /// across the whole plot, so their ends land exactly on the first and
    /// last bar. Before the fix this recovered `n1 -> n2 [color="#cccccc"]`
    /// — a gridline read as an edge.
    const CROSSING_GRIDLINES_BAR_CHART: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="700" height="450" viewBox="0 0 700 450">
      <title>Requests per quarter</title>
      <rect x="0" y="0" width="700" height="450" fill="white"/>
      <text x="60" y="48" font-family="Helvetica, sans-serif" font-size="16" font-weight="bold">Requests per quarter</text>
      <g stroke="#cccccc" stroke-width="1">
        <line x1="120" y1="300" x2="620" y2="300"/>
        <line x1="120" y1="220" x2="620" y2="220"/>
        <line x1="120" y1="140" x2="620" y2="140"/>
      </g>
      <g fill="#4c78a8">
        <rect x="120" y="180" width="100" height="200"/>
        <rect x="250" y="240" width="100" height="140"/>
        <rect x="380" y="130" width="100" height="250"/>
        <rect x="510" y="210" width="100" height="170"/>
      </g>
      <g stroke="#333333" stroke-width="1.5">
        <line x1="120" y1="380" x2="620" y2="380"/>
        <line x1="120" y1="380" x2="120" y2="120"/>
      </g>
      <g font-family="Helvetica, sans-serif" font-size="12" fill="#222222" text-anchor="middle">
        <text x="170" y="404">Q1</text>
        <text x="300" y="404">Q2</text>
        <text x="430" y="404">Q3</text>
        <text x="560" y="404">Q4</text>
      </g>
      <g font-family="Helvetica, sans-serif" font-size="11" fill="#555555" text-anchor="end">
        <text x="112" y="304">100</text>
        <text x="112" y="224">200</text>
        <text x="112" y="144">300</text>
      </g>
    </svg>"##;

    #[test]
    fn a_bar_chart_whose_gridlines_cross_the_bars_is_not_a_diagram() {
        assert!(recover(CROSSING_GRIDLINES_BAR_CHART.as_bytes()).is_none());
    }

    /// GH#1420's own scope note: this is not caption adoption (#1410). Strip
    /// every `<text>` from the fixture above and the drawing is still not a
    /// diagram, merely an unnamed one — the axis, the bars and the gridlines
    /// are unchanged.
    #[test]
    fn the_same_chart_without_any_captions_is_still_not_a_diagram() {
        const NO_CAPTIONS: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="700" height="450" viewBox="0 0 700 450">
          <rect x="0" y="0" width="700" height="450" fill="white"/>
          <g stroke="#cccccc" stroke-width="1">
            <line x1="120" y1="300" x2="620" y2="300"/>
            <line x1="120" y1="220" x2="620" y2="220"/>
            <line x1="120" y1="140" x2="620" y2="140"/>
          </g>
          <g fill="#4c78a8">
            <rect x="120" y="180" width="100" height="200"/>
            <rect x="250" y="240" width="100" height="140"/>
            <rect x="380" y="130" width="100" height="250"/>
            <rect x="510" y="210" width="100" height="170"/>
          </g>
          <g stroke="#333333" stroke-width="1.5">
            <line x1="120" y1="380" x2="620" y2="380"/>
            <line x1="120" y1="380" x2="120" y2="120"/>
          </g>
        </svg>"##;

        assert!(recover(NO_CAPTIONS.as_bytes()).is_none());
    }

    /// The harder control GH#1420 calls out: `data_dashboard.svg`'s own
    /// gridlines only miss the bars by luck, stopping 50 units short of
    /// them. This is the same dashboard with the gridlines extended to the
    /// bars' own edges, which is what most chart libraries actually draw. A
    /// fix that merely tightened the endpoint-distance threshold would still
    /// fail this one.
    #[test]
    fn a_dashboard_whose_gridlines_reach_the_bars_is_still_not_a_diagram() {
        const DASHBOARD_WITH_CROSSING_GRIDLINES: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" width="700" height="450" viewBox="0 0 700 450">
          <title>Quarterly Revenue Dashboard</title>
          <rect width="700" height="450" fill="#f8f9fa"/>
          <text x="350" y="35" text-anchor="middle" font-family="Arial" font-size="18" font-weight="bold" fill="#2c3e50">Quarterly Revenue Report FY2023</text>
          <line x1="120" y1="100" x2="640" y2="100" stroke="#e0e0e0" stroke-width="1"/>
          <line x1="120" y1="170" x2="640" y2="170" stroke="#e0e0e0" stroke-width="1"/>
          <line x1="120" y1="240" x2="640" y2="240" stroke="#e0e0e0" stroke-width="1"/>
          <line x1="120" y1="310" x2="640" y2="310" stroke="#e0e0e0" stroke-width="1"/>
          <line x1="120" y1="380" x2="640" y2="380" stroke="#e0e0e0" stroke-width="1"/>
          <rect x="120" y="170" width="100" height="210" fill="#3498db" rx="4"/>
          <text x="170" y="410" text-anchor="middle" font-family="Arial" font-size="12" fill="#333">Q1</text>
          <rect x="260" y="130" width="100" height="250" fill="#2ecc71" rx="4"/>
          <text x="310" y="410" text-anchor="middle" font-family="Arial" font-size="12" fill="#333">Q2</text>
          <rect x="400" y="200" width="100" height="180" fill="#e74c3c" rx="4"/>
          <text x="450" y="410" text-anchor="middle" font-family="Arial" font-size="12" fill="#333">Q3</text>
          <rect x="540" y="100" width="100" height="280" fill="#f39c12" rx="4"/>
          <text x="590" y="410" text-anchor="middle" font-family="Arial" font-size="12" fill="#333">Q4</text>
        </svg>"##;

        assert!(recover(DASHBOARD_WITH_CROSSING_GRIDLINES.as_bytes()).is_none());
    }

    /// `<g>` opened `depth` times around `leaf`, closed the same number of
    /// times.
    fn nested(depth: usize, leaf: &str) -> String {
        let mut source = String::from(
            r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">"##,
        );
        for _ in 0..depth {
            source.push_str("<g>");
        }
        source.push_str(leaf);
        for _ in 0..depth {
            source.push_str("</g>");
        }
        source.push_str("</svg>");
        source
    }

    /// Real attack magnitude (the defect report's "~10 MB is ~1.4 million
    /// levels"), run directly against `exceeds_max_nesting` and `recover`
    /// rather than through the byte-length cap, to prove the depth scan
    /// itself is iterative and returns promptly instead of recursing.
    #[test]
    fn should_reject_svg_nested_far_past_the_bound_without_recursing() {
        // Two orders of magnitude past the bound: enough to prove the scan
        // does not slow down or recurse as depth grows, without building
        // anywhere near the ~1.4 million levels the defect report's 10 MB
        // estimate implies.
        const ATTACK_SCALE_DEPTH: usize = 100_000;
        let source = nested(ATTACK_SCALE_DEPTH, "");

        assert!(exceeds_max_nesting(source.as_bytes()));
        assert!(recover(source.as_bytes()).is_none());
    }

    #[test]
    fn should_reject_nesting_exactly_one_past_the_bound() {
        // `nested` wraps its groups in an `<svg>`, and the root element is a
        // level of nesting like any other, so `nested(n)` is `n + 1` deep. ~keep
        assert!(!exceeds_max_nesting(nested(MAX_NESTING_DEPTH - 1, "").as_bytes()));
        assert!(exceeds_max_nesting(nested(MAX_NESTING_DEPTH, "").as_bytes()));
    }

    #[test]
    fn should_not_count_a_self_closing_group_toward_depth() {
        let mut source = String::from(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">"##);
        for _ in 0..(MAX_NESTING_DEPTH + 10) {
            source.push_str(r#"<g/>"#);
        }
        source.push_str("</svg>");

        assert!(!exceeds_max_nesting(source.as_bytes()));
    }

    #[test]
    fn should_not_miscount_a_greater_than_sign_inside_a_quoted_attribute() {
        // A literal `>` inside an attribute value must not end the tag early
        // and desynchronise the scan from the real element boundaries.
        let mut source = String::from(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">"##);
        for _ in 0..(MAX_NESTING_DEPTH + 10) {
            source.push_str(r#"<g data-note="a > b">"#);
        }
        for _ in 0..(MAX_NESTING_DEPTH + 10) {
            source.push_str("</g>");
        }
        source.push_str("</svg>");

        assert!(exceeds_max_nesting(source.as_bytes()));
    }

    #[test]
    fn should_ignore_nesting_markup_inside_a_comment() {
        let mut comment = String::from("<!--");
        for _ in 0..(MAX_NESTING_DEPTH * 4) {
            comment.push_str("<g>");
        }
        comment.push_str("-->");
        let source = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10">{comment}</svg>"##);

        assert!(!exceeds_max_nesting(source.as_bytes()));
    }

    /// A shape at every level of a chain nested past the bound: if the depth
    /// guard did not trip, this is shaped exactly like a two-node diagram.
    #[test]
    fn should_return_no_diagram_for_a_shape_flood_nested_past_the_bound() {
        let mut leaf = String::new();
        for i in 0..(MAX_NESTING_DEPTH + 20) {
            leaf.push_str(&format!(r#"<rect x="{i}" y="{i}" width="10" height="10"/>"#));
        }
        leaf.push_str(r##"<line x1="0" y1="0" x2="50" y2="50" stroke="#333"/>"##);

        assert!(recover(nested(MAX_NESTING_DEPTH + 20, &leaf).as_bytes()).is_none());
    }

    /// A label at every level of a chain nested past the bound: proves the
    /// text pass's own depth guard does not leak labels from a subtree the
    /// byte-level scan already rejected.
    #[test]
    fn should_return_no_diagram_for_a_label_flood_nested_past_the_bound() {
        let mut leaf = String::from(r#"<rect x="10" y="10" width="10" height="10"/>"#);
        for i in 0..(MAX_NESTING_DEPTH + 20) {
            leaf.push_str(&format!(r#"<text x="{i}" y="{i}">label {i}</text>"#));
        }
        leaf.push_str(r#"<rect x="200" y="200" width="10" height="10"/>"#);
        leaf.push_str(r##"<line x1="10" y1="10" x2="200" y2="200" stroke="#333"/>"##);

        assert!(recover(nested(MAX_NESTING_DEPTH + 20, &leaf).as_bytes()).is_none());
    }

    /// The bound must not be so tight that an everyday translated wrapper
    /// (see `a_translated_group_still_matches_labels_to_shapes` above) stops
    /// recovering.
    #[test]
    fn should_still_recover_a_diagram_nested_well_within_the_bound() {
        let leaf = concat!(
            r##"<rect x="10" y="10" width="80" height="40" fill="#2c3e50"/>"##,
            r#"<text x="50" y="35" text-anchor="middle">Start</text>"#,
            r##"<rect x="10" y="150" width="80" height="40" fill="#27ae60"/>"##,
            r#"<text x="50" y="175" text-anchor="middle">End</text>"#,
            r##"<line x1="50" y1="50" x2="50" y2="150" stroke="#333"/>"##,
        );

        let graph = recovered(&nested(4, leaf));
        assert_eq!(graph.nodes.len(), 2);
        assert_eq!(graph.edges.len(), 1);
    }
}
