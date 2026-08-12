//! Ground-truth scoring for vector diagram recovery (#579).
//!
//! The corpus drives this test, not a list kept here. `diagrams/manifest.json`
//! says what every fixture is, which graph it draws and where the answer
//! lives, so a fixture added there is scored here without touching this file.
//!
//! Scoring rather than golden-file comparison is deliberate. A golden file
//! freezes whatever the implementation currently returns and calls it correct;
//! recall and precision against an independently written answer say how good it
//! actually is, and regressions show up as a number moving rather than as a
//! diff nobody reads.
//!
//! Ground truth is keyed by node label rather than by generated id, so it does
//! not depend on how any recogniser numbers its output.
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

impl Shape {
    fn extend(&mut self, other: Shape) {
        self.nodes.extend(other.nodes);
        self.edges.extend(other.edges);
    }
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
        if line.is_empty() || line.starts_with("digraph") || line.starts_with("graph") || line.starts_with('}') {
            continue;
        }
        if let Some((left, right)) = line.split_once("->").or_else(|| line.split_once("--")) {
            let from = unquote(left.trim());
            let to = unquote(right.trim().split('[').next().unwrap_or_default().trim());
            edges.push((from, to));
        } else if let Some((id, attrs)) = line.split_once('[') {
            let id = unquote(id.trim());
            // Our renderer emits generated ids and carries the text in `label`;
            // ground truth uses the label as the id. Either way the label wins. ~keep
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
        nodes: labels.iter().map(|(_, label)| normalise(&label.clone())).collect(),
        edges: edges
            .iter()
            .map(|(a, b)| (normalise(&resolve(a)), normalise(&resolve(b))))
            .collect(),
    }
}

/// Collapse the whitespace differences that mean nothing to a graph.
///
/// A renderer breaks a label wherever its layout needs to, so the same node
/// arrives as `Switch A` from one producer and `Switch\nA` from another. The
/// graph is the same either way, and the ground truth should not have to
/// predict which line breaks a layout engine chose.
fn normalise(label: &str) -> String {
    label.split_whitespace().collect::<Vec<_>>().join(" ")
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

/// Extract a corpus-relative fixture as DOT, or `None` when it is not present.
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

/// One fixture as the corpus manifest describes it.
struct Fixture {
    path: String,
    class: String,
    geometry_variant: Option<String>,
    ground_truths: Vec<String>,
    exercises: String,
}

fn manifest() -> Option<Vec<Fixture>> {
    let raw = std::fs::read_to_string(get_test_file_path("diagrams/manifest.json")).ok()?;
    let parsed: serde_json::Value = serde_json::from_str(&raw).ok()?;

    Some(
        parsed
            .get("fixtures")?
            .as_array()?
            .iter()
            .map(|entry| Fixture {
                path: string_at(entry, "path"),
                class: string_at(entry, "class"),
                geometry_variant: entry
                    .get("geometry_only_variant")
                    .and_then(|v| v.as_str())
                    .map(str::to_string),
                ground_truths: entry
                    .get("graphs")
                    .and_then(|g| g.as_array())
                    .map(|graphs| {
                        graphs
                            .iter()
                            .filter_map(|g| g.get("ground_truth").and_then(|v| v.as_str()))
                            .map(str::to_string)
                            .collect()
                    })
                    .unwrap_or_default(),
                exercises: entry
                    .get("exercises")
                    .and_then(|e| e.as_array())
                    .map(|items| items.iter().filter_map(|i| i.as_str()).collect::<Vec<_>>().join(", "))
                    .unwrap_or_default(),
            })
            .collect(),
    )
}

fn string_at(entry: &serde_json::Value, key: &str) -> String {
    entry.get(key).and_then(|v| v.as_str()).unwrap_or_default().to_string()
}

fn expected_shape(ground_truths: &[String]) -> Option<Shape> {
    let mut want = Shape::default();
    for relative in ground_truths {
        let text = std::fs::read_to_string(get_test_file_path(relative)).ok()?;
        want.extend(parse_dot(&text));
    }
    Some(want)
}

/// Recovery must find every node the file draws. Nodes are the easy half: a
/// shape is either there or it is not.
const MIN_NODE_RECALL: f64 = 1.0;

/// Edges too, on the producers the corpus covers.
const MIN_EDGE_RECALL: f64 = 1.0;

/// The one fixture not yet fully recovered, pinned at what it scores today so
/// the gap is visible and a regression still trips the test.
///
/// `graphviz_large` loses 4 of 141 edges, all long-range shortcuts that pass
/// close to intervening nodes on the way.
const KNOWN_GAPS: &[(&str, f64, f64)] = &[("graphviz_large", 1.0, 0.97)];

fn floors(fixture: &Fixture) -> (f64, f64) {
    // Class A files state their graph outright, so recovery from them is
    // lossless and anything short of exact is a defect, not a threshold.
    if fixture.class == "A" {
        return (1.0, 1.0);
    }
    let stem = fixture
        .path
        .rsplit('/')
        .next()
        .and_then(|name| name.split('.').next())
        .unwrap_or_default();
    KNOWN_GAPS
        .iter()
        .find(|(known, _, _)| *known == stem)
        .map(|(_, nodes, edges)| (*nodes, *edges))
        .unwrap_or((MIN_NODE_RECALL, MIN_EDGE_RECALL))
}

struct Score {
    node_recall: f64,
    edge_recall: f64,
    missing_nodes: Vec<String>,
    missing_edges: Vec<(String, String)>,
    invented_nodes: Vec<String>,
}

fn score(dot: &str, want: &Shape) -> Score {
    let got = parse_dot(dot);
    Score {
        node_recall: recall(want.nodes.len(), want.nodes.intersection(&got.nodes).count()),
        edge_recall: recall(want.edges.len(), want.edges.intersection(&got.edges).count()),
        missing_nodes: want.nodes.difference(&got.nodes).cloned().collect(),
        missing_edges: want.edges.difference(&got.edges).cloned().collect(),
        // Precision matters as much as recall: inventing nodes is how an
        // arrowhead, a cluster border or a double border shows up as
        // structure that is not there.
        invented_nodes: got.nodes.difference(&want.nodes).cloned().collect(),
    }
}

#[test]
fn recovers_the_diagram_corpus() {
    let Some(fixtures) = manifest() else {
        eprintln!("test_documents not populated, skipping diagram ground truth");
        return;
    };

    let mut scored = 0;
    let mut failures: Vec<String> = Vec::new();

    for fixture in &fixtures {
        // Negatives are covered by their own test.
        if fixture.ground_truths.is_empty() {
            continue;
        }
        let Some(want) = expected_shape(&fixture.ground_truths) else {
            continue;
        };

        // A fixture that states its own graph is scored twice: as emitted, and
        // with the producer's metadata stripped. Reading the metadata is
        // correct behaviour, but only the stripped copy measures geometry. ~keep
        let variants = [Some(fixture.path.clone()), fixture.geometry_variant.clone()];
        for variant in variants.into_iter().flatten() {
            let Some(dot) = extract_dot(&variant) else {
                continue;
            };
            scored += 1;

            let result = score(&dot, &want);
            let name = variant.rsplit('/').next().unwrap_or(&variant);
            println!(
                "{:<40} nodes {:>3}/{:<3} ({:>3.0}%)  edges {:>3}/{:<3} ({:>3.0}%)  [{}]",
                name,
                (result.node_recall * want.nodes.len() as f64).round() as usize,
                want.nodes.len(),
                result.node_recall * 100.0,
                (result.edge_recall * want.edges.len() as f64).round() as usize,
                want.edges.len(),
                result.edge_recall * 100.0,
                fixture.exercises,
            );

            let (node_floor, edge_floor) = floors(fixture);

            if result.node_recall < node_floor {
                failures.push(format!("{name}: missing nodes {:?}", result.missing_nodes));
            }
            if result.edge_recall < edge_floor {
                failures.push(format!("{name}: missing edges {:?}", result.missing_edges));
            }
            // Precision is not pinned for a fixture with a known recall gap:
            // the shapes it fails to name are the same ones it reports
            // unlabelled, so both halves move together when it is fixed.
            if !result.invented_nodes.is_empty() && node_floor >= MIN_NODE_RECALL {
                failures.push(format!("{name}: invented nodes {:?}", result.invented_nodes));
            }
        }
    }

    if scored == 0 {
        eprintln!("test_documents not populated, skipping diagram ground truth");
        return;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// A vector drawing that is not a diagram must produce no graph at all.
///
/// Bar charts, ruled tables, forms and illustrations all yield closed outlines,
/// and reporting an edgeless node list for one is a false positive. The corpus
/// marks these by declaring no graphs.
#[test]
fn drawings_that_are_not_diagrams_recover_nothing() {
    let Some(fixtures) = manifest() else {
        eprintln!("test_documents not populated, skipping negative cases");
        return;
    };

    let mut checked = 0;
    let mut failures: Vec<String> = Vec::new();

    for fixture in fixtures.iter().filter(|f| f.ground_truths.is_empty()) {
        let Some(dot) = extract_dot(&fixture.path) else {
            continue;
        };
        checked += 1;
        if !dot.trim().is_empty() {
            failures.push(format!(
                "{}: recovered a graph from a non-diagram:\n{dot}",
                fixture.path
            ));
        }
    }

    if checked == 0 {
        eprintln!("test_documents not populated, skipping negative cases");
        return;
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
