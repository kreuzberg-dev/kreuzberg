//! Graphviz DOT renderer.
//!
//! Emits the diagram graphs recovered during extraction. A document with no
//! recovered diagram renders as the empty string rather than as an empty graph,
//! so a caller can tell "no diagram here" from "a diagram with nothing in it".

use crate::types::diagram::{DiagramEdge, DiagramGraph, DiagramNode};
use crate::types::internal::InternalDocument;

/// Render every recovered diagram as a `digraph` block.
pub(crate) fn render_dot(doc: &InternalDocument) -> String {
    let mut out = String::new();
    for (index, graph) in doc.diagrams.iter().enumerate() {
        if index > 0 {
            out.push('\n');
        }
        render_graph(graph, index, &mut out);
    }
    out
}

fn render_graph(graph: &DiagramGraph, index: usize, out: &mut String) {
    let name = graph.name.as_deref().filter(|n| !n.trim().is_empty());
    match name {
        Some(name) => out.push_str(&format!("digraph {} {{\n", quote(name))),
        None => out.push_str(&format!("digraph G{index} {{\n")),
    }

    for node in &graph.nodes {
        out.push_str(&format!("  {} [{}];\n", node.id, node_attributes(node)));
    }

    if !graph.nodes.is_empty() && !graph.edges.is_empty() {
        out.push('\n');
    }

    for edge in &graph.edges {
        let (Some(from), Some(to)) = (graph.nodes.get(edge.from), graph.nodes.get(edge.to)) else {
            continue;
        };
        let attributes = edge_attributes(edge);
        if attributes.is_empty() {
            out.push_str(&format!("  {} -> {};\n", from.id, to.id));
        } else {
            out.push_str(&format!("  {} -> {} [{}];\n", from.id, to.id, attributes));
        }
    }

    out.push_str("}\n");
}

fn node_attributes(node: &DiagramNode) -> String {
    let mut attributes = vec![format!("label={}", quote(&node.label))];
    attributes.push(format!("shape={}", node.shape.as_dot()));

    // Graphviz only paints `fillcolor` when `filled` is in `style`, and only
    // dashes an outline when `dashed` is, so the two share one attribute.
    let mut style: Vec<&str> = Vec::new();
    if node.fill.is_some() {
        style.push("filled");
    }
    if node.dashed {
        style.push("dashed");
    }
    match style.len() {
        0 => {}
        1 => attributes.push(format!("style={}", style[0])),
        _ => attributes.push(format!("style={}", quote(&style.join(",")))),
    }
    if let Some(fill) = &node.fill {
        attributes.push(format!("fillcolor={}", quote(fill)));
    }
    if let Some(stroke) = &node.stroke {
        attributes.push(format!("color={}", quote(stroke)));
    }
    // Graphviz already draws at width 1, and SVG gives every stroked shape a
    // width whether the source set one or not, so emitting the default would
    // put a redundant attribute on almost every node.
    if let Some(width) = node.stroke_width.filter(|w| *w != 1.0) {
        attributes.push(format!("penwidth={}", number(width)));
    }
    attributes.join(" ")
}

fn edge_attributes(edge: &DiagramEdge) -> String {
    let mut attributes: Vec<String> = Vec::new();
    if let Some(label) = &edge.label {
        attributes.push(format!("label={}", quote(label)));
    }
    if edge.bidirectional {
        attributes.push("dir=both".to_string());
    }
    if edge.dashed {
        attributes.push("style=dashed".to_string());
    }
    if let Some(stroke) = &edge.stroke {
        attributes.push(format!("color={}", quote(stroke)));
    }
    attributes.join(" ")
}

/// A DOT quoted string. Backslashes and quotes are escaped, and a newline
/// becomes the `\n` that Graphviz renders as a line break inside a label.
fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => {}
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

/// Drop the trailing `.0` so whole widths read as `penwidth=2`, not `2`
/// rendered inconsistently across platforms.
fn number(value: f32) -> String {
    if value.fract() == 0.0 && value.is_finite() {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::diagram::DiagramShape;

    fn node(id: &str, label: &str) -> DiagramNode {
        DiagramNode {
            id: id.to_string(),
            label: label.to_string(),
            shape: DiagramShape::Box,
            fill: None,
            stroke: None,
            stroke_width: None,
            dashed: false,
        }
    }

    fn doc_with(graph: DiagramGraph) -> InternalDocument {
        let mut doc = InternalDocument::new("svg");
        doc.diagrams.push(graph);
        doc
    }

    #[test]
    fn renders_nodes_and_edges() {
        let doc = doc_with(DiagramGraph {
            name: Some("Chart".to_string()),
            nodes: vec![node("n0", "Start"), node("n1", "End")],
            edges: vec![DiagramEdge {
                from: 0,
                to: 1,
                bidirectional: false,
                label: None,
                stroke: None,
                dashed: false,
            }],
        });

        assert_eq!(
            render_dot(&doc),
            "digraph \"Chart\" {\n  \
               n0 [label=\"Start\" shape=box];\n  \
               n1 [label=\"End\" shape=box];\n\n  \
               n0 -> n1;\n}\n"
        );
    }

    #[test]
    fn styling_becomes_graphviz_attributes() {
        let mut filled = node("n0", "Start");
        filled.fill = Some("#2c3e50".to_string());
        filled.stroke = Some("#1a252f".to_string());
        filled.stroke_width = Some(2.0);
        filled.dashed = true;

        let doc = doc_with(DiagramGraph {
            name: None,
            nodes: vec![filled, node("n1", "End")],
            edges: vec![DiagramEdge {
                from: 0,
                to: 1,
                bidirectional: false,
                label: Some("yes".to_string()),
                stroke: Some("#333333".to_string()),
                dashed: true,
            }],
        });

        let dot = render_dot(&doc);
        assert!(dot.starts_with("digraph G0 {\n"), "{dot}");
        assert!(
            dot.contains(
                "n0 [label=\"Start\" shape=box style=\"filled,dashed\" \
                 fillcolor=\"#2c3e50\" color=\"#1a252f\" penwidth=2];"
            ),
            "{dot}"
        );
        assert!(
            dot.contains("n0 -> n1 [label=\"yes\" style=dashed color=\"#333333\"];"),
            "{dot}"
        );
    }

    #[test]
    fn a_single_style_is_unquoted_and_a_default_penwidth_is_omitted() {
        let mut plain = node("n0", "Start");
        plain.fill = Some("#ffffff".to_string());
        plain.stroke_width = Some(1.0);

        let doc = doc_with(DiagramGraph {
            name: None,
            nodes: vec![plain, node("n1", "End")],
            edges: vec![DiagramEdge {
                from: 0,
                to: 1,
                bidirectional: false,
                label: None,
                stroke: None,
                dashed: false,
            }],
        });

        let dot = render_dot(&doc);
        assert!(
            dot.contains("n0 [label=\"Start\" shape=box style=filled fillcolor=\"#ffffff\"];"),
            "{dot}"
        );
    }

    #[test]
    fn multi_line_labels_and_quotes_are_escaped() {
        let doc = doc_with(DiagramGraph {
            name: Some("a \"quoted\" name".to_string()),
            nodes: vec![node("n0", "Jane Smith\nChief \"Exec\"\\Officer"), node("n1", "")],
            edges: vec![DiagramEdge {
                from: 0,
                to: 1,
                bidirectional: false,
                label: None,
                stroke: None,
                dashed: false,
            }],
        });

        let dot = render_dot(&doc);
        assert!(dot.starts_with("digraph \"a \\\"quoted\\\" name\" {"), "{dot}");
        assert!(
            dot.contains("n0 [label=\"Jane Smith\\nChief \\\"Exec\\\"\\\\Officer\" shape=box];"),
            "{dot}"
        );
        assert!(dot.contains("n1 [label=\"\" shape=box];"), "{dot}");
    }

    #[test]
    fn a_document_without_diagrams_renders_nothing() {
        assert_eq!(render_dot(&InternalDocument::new("pdf")), "");
    }

    #[test]
    fn every_graph_is_emitted() {
        let mut doc = InternalDocument::new("svg");
        for _ in 0..2 {
            doc.diagrams.push(DiagramGraph {
                name: None,
                nodes: vec![node("n0", "a"), node("n1", "b")],
                edges: vec![DiagramEdge {
                    from: 0,
                    to: 1,
                    bidirectional: false,
                    label: None,
                    stroke: None,
                    dashed: false,
                }],
            });
        }

        let dot = render_dot(&doc);
        assert!(dot.contains("digraph G0 {"), "{dot}");
        assert!(dot.contains("digraph G1 {"), "{dot}");
    }

    /// The renderer is reachable without an `OutputFormat` variant: an unknown
    /// format string parses to `Custom`, which `derive_extraction_result` routes
    /// through the renderer registry. This is what makes `output_format="dot"`
    /// work from every language binding.
    #[test]
    #[cfg(all(feature = "xml", feature = "svg"))]
    fn dot_is_reachable_via_custom_output_format() {
        use crate::core::config::{ExtractionConfig, OutputFormat};
        use crate::extractors::SyncExtractor;
        use std::str::FromStr;

        // `FromStr` is the API-handler path; serde is the path the language
        // bindings take (e.g. Python's `OutputFormat("dot")`). ~keep
        let format = OutputFormat::from_str("dot").unwrap();
        assert_eq!(format, OutputFormat::Custom("dot".to_string()));
        assert_eq!(
            serde_json::from_str::<OutputFormat>("\"dot\"").unwrap(),
            OutputFormat::Custom("dot".to_string())
        );

        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="400" viewBox="0 0 400 400">
          <title>Pipeline</title>
          <rect x="100" y="20" width="120" height="60" fill="#2c3e50"/>
          <text x="160" y="55" text-anchor="middle">Start</text>
          <rect x="100" y="200" width="120" height="60" fill="#27ae60"/>
          <text x="160" y="235" text-anchor="middle">End</text>
          <line x1="160" y1="80" x2="160" y2="200" stroke="#333"/>
        </svg>"##;

        // Recovery is gated on the request actually resolving to `dot` output
        // (#1410 review fixes: skip recovery entirely unless the request resolves to
        // the `dot` renderer), so the config passed to the extractor has to carry the
        // format, exactly as the API/CLI/binding entry points do before calling into
        // the extractor. ~keep
        let config = ExtractionConfig {
            output_format: format.clone(),
            ..Default::default()
        };
        let extracted = crate::extractors::xml::XmlExtractor::new()
            .extract_sync(svg, "image/svg+xml", &config)
            .expect("extraction");
        let result = crate::extraction::derive::derive_extraction_result(extracted, false, format);

        assert_eq!(
            result.formatted_content.as_deref(),
            Some(
                "digraph \"Pipeline\" {\n  \
                   n0 [label=\"Start\" shape=box style=filled fillcolor=\"#2c3e50\"];\n  \
                   n1 [label=\"End\" shape=box style=filled fillcolor=\"#27ae60\"];\n\n  \
                   n0 -> n1 [color=\"#333333\"];\n}\n"
            )
        );
    }

    #[test]
    fn an_edge_pointing_past_the_node_list_is_skipped() {
        let doc = doc_with(DiagramGraph {
            name: None,
            nodes: vec![node("n0", "only")],
            edges: vec![DiagramEdge {
                from: 0,
                to: 7,
                bidirectional: false,
                label: None,
                stroke: None,
                dashed: false,
            }],
        });

        assert_eq!(
            render_dot(&doc),
            "digraph G0 {\n  n0 [label=\"only\" shape=box];\n\n}\n"
        );
    }
}
