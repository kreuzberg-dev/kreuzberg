//! Ground-truth scoring for vector diagram recovery (#579).
//!
//! Each fixture is a real diagram file; each ground truth is the graph that file
//! draws, written by node label so it does not depend on how any recogniser
//! numbers its output. The test extracts through the public pipeline with
//! `output_format="dot"` and scores what comes back.
//!
//! Scoring rather than golden-file comparison is deliberate. A golden file
//! freezes whatever the implementation currently returns and calls it correct;
//! recall and precision against an independently written answer say how good it
//! actually is, and regressions show up as a number moving rather than as a
//! diff nobody reads.
//!
//! Usage:
//!   cargo test -p xberg --features "xml,svg" --test diagram_dot_ground_truth -- --nocapture

#![allow(clippy::print_stdout, clippy::print_stderr)] // ~keep: test binaries print by design
#![cfg(all(feature = "xml", feature = "svg"))]

mod helpers;
use helpers::{extract_uri_document_blocking, get_test_file_path};

use std::collections::BTreeSet;
use xberg::core::config::{ExtractionConfig, OutputFormat};

/// A graph reduced to what is comparable across recognisers: which nodes exist,
/// by label, and which labels are joined.
#[derive(Debug, Default, PartialEq)]
struct Shape {
    nodes: BTreeSet<String>,
    edges: BTreeSet<(String, String)>,
}

/// Parse the subset of DOT both our renderer and the ground truth files use.
///
/// This is not a general DOT parser and does not need to be: both sides are
/// one statement per line, `id [attrs];` or `a -> b [attrs];`.
fn parse_dot(source: &str) -> Shape {
    let mut labels: Vec<(String, String)> = Vec::new();
    let mut edges: Vec<(String, String)> = Vec::new();

    for line in source.lines() {
        let line = line.trim().trim_end_matches(';');
        if line.is_empty() || line.starts_with("digraph") || line.starts_with('}') {
            continue;
        }
        if let Some((left, right)) = line.split_once("->") {
            let from = unquote(left.trim());
            let to = unquote(right.trim().split('[').next().unwrap_or_default().trim());
            edges.push((from, to));
        } else if let Some((id, attrs)) = line.split_once('[') {
            let id = unquote(id.trim());
            // Our renderer emits generated ids and carries the text in `label`;
            // ground truth uses the label as the id. Either way the label wins.
            let label = attribute(attrs, "label").unwrap_or_else(|| id.clone());
            labels.push((id, label));
        }
    }

    let resolve = |id: &str| -> String {
        labels
            .iter()
            .find(|(node_id, _)| node_id == id)
            .map(|(_, label)| label.clone())
            .unwrap_or_else(|| id.to_string())
    };

    Shape {
        nodes: labels.iter().map(|(_, label)| label.clone()).collect(),
        edges: edges.iter().map(|(a, b)| (resolve(a), resolve(b))).collect(),
    }
}

fn unquote(value: &str) -> String {
    value.trim().trim_matches('"').replace("\\n", "\n")
}

fn attribute(attrs: &str, name: &str) -> Option<String> {
    let key = format!("{name}=\"");
    let start = attrs.find(&key)? + key.len();
    let rest = &attrs[start..];
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => break,
            '\\' => match chars.next() {
                Some('n') => out.push('\n'),
                Some(other) => out.push(other),
                None => break,
            },
            _ => out.push(ch),
        }
    }
    Some(out)
}

fn recall(expected: usize, matched: usize) -> f64 {
    if expected == 0 {
        1.0
    } else {
        matched as f64 / expected as f64
    }
}

/// Extract a fixture as DOT, or `None` when the corpus is not populated.
fn extract_dot(relative_path: &str) -> Option<String> {
    let path = get_test_file_path(relative_path);
    if !path.exists() {
        return None;
    }
    let config = ExtractionConfig {
        output_format: OutputFormat::Custom("dot".to_string()),
        ..Default::default()
    };
    let result = extract_uri_document_blocking(&path, None, &config).ok()?;
    Some(result.content)
}

fn ground_truth(stem: &str) -> Option<String> {
    let path = get_test_file_path(&format!("ground_truth/dot/{stem}.dot"));
    std::fs::read_to_string(path).ok()
}

struct Case {
    fixture: &'static str,
    stem: &'static str,
    what: &'static str,
}

/// Every diagram fixture in the corpus, with what each one is here to catch.
const CASES: &[Case] = &[
    Case {
        fixture: "diagrams/graphviz_flow.svg",
        stem: "graphviz_flow",
        what: "arrowheads, edge labels, three node shapes, negative root translate",
    },
    Case {
        fixture: "diagrams/graphviz_states.svg",
        stem: "graphviz_states",
        what: "doublecircle, antiparallel edge pair",
    },
    Case {
        fixture: "diagrams/graphviz_network.svg",
        stem: "graphviz_network",
        what: "undirected edges, no arrowhead anywhere",
    },
    Case {
        fixture: "diagrams/graphviz_bidirectional.svg",
        stem: "graphviz_bidirectional",
        what: "dir=both and dir=back",
    },
    Case {
        fixture: "diagrams/nested_transforms.svg",
        stem: "nested_transforms",
        what: "nested transform groups, viewBox differing from viewport",
    },
    Case {
        fixture: "xml/org_chart.svg",
        stem: "org_chart",
        what: "two-line labels, isolated leaf nodes",
    },
    Case {
        fixture: "xml/flowchart.svg",
        stem: "flowchart",
        what: "annotations outside every shape",
    },
];

/// Recovery must find every node the file draws. Nodes are the easy half: a
/// shape is either there or it is not, so anything below perfect is a defect.
const MIN_NODE_RECALL: f64 = 1.0;

/// Edges are harder. Two nodes joined by a pair of opposing connectors put four
/// arrowheads within a few units of each other, and one of the pair is
/// currently lost, so `graphviz_states` scores 0.75. Raising this floor is the
/// next thing worth doing, and the number is here so that it is visible.
const MIN_EDGE_RECALL: f64 = 0.75;

#[test]
fn recovers_the_diagram_corpus() {
    let mut scored = 0;
    let mut failures: Vec<String> = Vec::new();

    for case in CASES {
        let (Some(dot), Some(truth)) = (extract_dot(case.fixture), ground_truth(case.stem)) else {
            continue;
        };
        scored += 1;

        let got = parse_dot(&dot);
        let want = parse_dot(&truth);

        let nodes_found = want.nodes.intersection(&got.nodes).count();
        let edges_found = want.edges.intersection(&got.edges).count();
        let node_recall = recall(want.nodes.len(), nodes_found);
        let edge_recall = recall(want.edges.len(), edges_found);

        println!(
            "{:<28} nodes {}/{} ({:.0}%)  edges {}/{} ({:.0}%)  [{}]",
            case.stem,
            nodes_found,
            want.nodes.len(),
            node_recall * 100.0,
            edges_found,
            want.edges.len(),
            edge_recall * 100.0,
            case.what,
        );

        if node_recall < MIN_NODE_RECALL {
            let missing: Vec<&String> = want.nodes.difference(&got.nodes).collect();
            failures.push(format!("{}: missing nodes {:?}", case.stem, missing));
        }
        if edge_recall < MIN_EDGE_RECALL {
            let missing: Vec<&(String, String)> = want.edges.difference(&got.edges).collect();
            failures.push(format!("{}: missing edges {:?}", case.stem, missing));
        }
        // Precision matters as much as recall: inventing nodes is how an
        // arrowhead or a double border shows up as structure that is not there.
        let invented: Vec<&String> = got.nodes.difference(&want.nodes).collect();
        if !invented.is_empty() {
            failures.push(format!("{}: invented nodes {:?}", case.stem, invented));
        }
    }

    if scored == 0 {
        eprintln!("test_documents not populated, skipping diagram ground truth");
        return;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A vector drawing that is not a diagram must produce no graph at all.
/// Reporting an edgeless node list for a bar chart is a false positive, and
/// these two files are the corpus's record of that.
#[test]
fn drawings_that_are_not_diagrams_recover_nothing() {
    let mut checked = 0;
    for (fixture, stem) in [
        ("xml/data_dashboard.svg", "data_dashboard"),
        ("xml/simple_svg.svg", "simple_svg"),
    ] {
        let (Some(dot), Some(truth)) = (extract_dot(fixture), ground_truth(stem)) else {
            continue;
        };
        checked += 1;
        assert!(truth.trim().is_empty(), "{stem}: ground truth should be empty");
        assert!(
            dot.trim().is_empty(),
            "{stem}: recovered a graph from a non-diagram:\n{dot}"
        );
    }
    if checked == 0 {
        eprintln!("test_documents not populated, skipping negative cases");
    }
}
