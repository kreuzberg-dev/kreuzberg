//! Full-decode tests against the WordPerfect corpus in the `test_documents`
//! submodule, covering WP 4.2 through Corel WP6 and the Macintosh variants.
//!
//! Each document is checked against the committed `ground_truth/wordperfect`
//! renderings: `.txt` against [`extract_text`] and `.md` against
//! [`extract_markdown`]. Comparison normalizes runs of whitespace, since line
//! wrapping is a presentation detail of the source document rather than part
//! of the extracted content.
//!
//! Every test skips when the submodule is not checked out (or its LFS objects
//! are not pulled), matching the other corpus-backed tests in this workspace.

use std::collections::BTreeMap;
use std::path::PathBuf;

/// Documents whose extraction matches the ground truth exactly, in both modes.
const EXACT: &[&str] = &["wp42", "wp50", "wp51", "wp6", "wp_mac1", "wp_mac3"];

/// Source extension per document stem; WP 4.2/5.x samples use `.wp`.
fn source_name(stem: &str) -> String {
    match stem {
        "wp42" | "wp50" | "wp51" => format!("{stem}.wp"),
        _ => format!("{stem}.wpd"),
    }
}

fn test_documents() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../test_documents")
}

/// Reads a corpus file, or returns `None` when the submodule is absent or the
/// file is still an unfetched LFS pointer.
fn corpus(rel: &str) -> Option<Vec<u8>> {
    let bytes = std::fs::read(test_documents().join(rel)).ok()?;
    if bytes.starts_with(b"version https://git-lfs") {
        return None;
    }
    Some(bytes)
}

fn corpus_str(rel: &str) -> Option<String> {
    corpus(rel).and_then(|b| String::from_utf8(b).ok())
}

/// Collapses all whitespace runs to single spaces so that comparisons ignore
/// source line wrapping.
fn normalize(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn word_counts(s: &str) -> BTreeMap<&str, usize> {
    let mut counts = BTreeMap::new();
    for w in s.split_whitespace() {
        *counts.entry(w).or_insert(0) += 1;
    }
    counts
}

#[test]
fn text_matches_ground_truth() {
    let mut checked = 0;
    for stem in EXACT {
        let (Some(bytes), Some(expected)) = (
            corpus(&format!("wordperfect/{}", source_name(stem))),
            corpus_str(&format!("ground_truth/wordperfect/{stem}.txt")),
        ) else {
            continue;
        };
        assert!(xberg_libwpd::is_supported(&bytes), "{stem} should be recognized");
        let got = xberg_libwpd::extract_text(&bytes).unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert_eq!(
            normalize(&got),
            normalize(&expected),
            "{stem}: text differs from ground truth"
        );
        checked += 1;
    }
    eprintln!("compared {checked}/{} documents", EXACT.len());
}

#[test]
fn markdown_matches_ground_truth() {
    let mut checked = 0;
    for stem in EXACT {
        let (Some(bytes), Some(expected)) = (
            corpus(&format!("wordperfect/{}", source_name(stem))),
            corpus_str(&format!("ground_truth/wordperfect/{stem}.md")),
        ) else {
            continue;
        };
        let got = xberg_libwpd::extract_markdown(&bytes).unwrap_or_else(|e| panic!("{stem}: {e}"));
        assert_eq!(
            normalize(&got),
            normalize(&expected),
            "{stem}: markdown differs from ground truth"
        );
        checked += 1;
    }
    eprintln!("compared {checked}/{} documents", EXACT.len());
}

/// `corel_wp6.wpd` is a 1.1 MB real-world WP6 report: nested emphasis,
/// superscripts, a footnote, tables and embedded images.
///
/// It is asserted by content rather than byte-for-byte because two rendering
/// choices differ deliberately from the ground truth: tables stay
/// tab/newline-separated instead of becoming HTML `<table>` markup (see
/// `shim.cpp`'s `render`), and the note body is collected at the end of the
/// document rather than at the end of its page. Every ground-truth word must
/// still be reproduced.
#[test]
fn corel_wp6_reproduces_all_ground_truth_text() {
    let (Some(bytes), Some(expected)) = (
        corpus("wordperfect/corel_wp6.wpd"),
        corpus_str("ground_truth/wordperfect/corel_wp6.txt"),
    ) else {
        return;
    };

    let got = xberg_libwpd::extract_text(&bytes).expect("extract_text");
    let got_counts = word_counts(&got);
    let missing: Vec<_> = word_counts(&expected)
        .into_iter()
        .filter(|(w, n)| got_counts.get(w).copied().unwrap_or(0) < *n)
        .map(|(w, _)| w)
        .collect();
    assert!(missing.is_empty(), "words missing from extraction: {missing:?}");

    assert!(got.contains("[1]"), "expected a numbered note anchor");
    assert!(
        got.contains("[1] E wordt op vrijwel"),
        "expected the collected note body"
    );

    let markdown = xberg_libwpd::extract_markdown(&bytes).expect("extract_markdown");
    assert!(
        markdown.contains("[^1]"),
        "markdown should use footnote reference syntax"
    );
    assert!(
        markdown.contains("[^1]: "),
        "markdown should define the collected footnote"
    );
    assert!(markdown.contains("<sup>"), "superscripts should survive into markdown");
}

/// The CVE samples are malformed by construction and the `.wpg` files are
/// WordPerfect *Graphics*, which libwpd does not handle. None may panic, hang
/// or return success.
#[test]
fn malformed_and_unrelated_documents_are_rejected() {
    for name in [
        "cve_2007_1735_1.wpd",
        "cve_2015_1760_1.wpd",
        "cve_2015_1760_2.wpd",
        "graphic_v1.wpg",
        "graphic_v2.wpg",
    ] {
        let Some(bytes) = corpus(&format!("wordperfect/{name}")) else {
            continue;
        };
        assert!(
            !xberg_libwpd::is_supported(&bytes),
            "{name} should not be reported as supported"
        );
        assert!(xberg_libwpd::extract_text(&bytes).is_err(), "{name} should not decode");
        assert!(
            xberg_libwpd::extract_markdown(&bytes).is_err(),
            "{name} should not decode"
        );
    }
}

/// Truncating a real document at many offsets exercises the parser's error
/// paths against input that starts out genuinely well-formed.
#[test]
fn truncated_documents_never_crash() {
    let Some(bytes) = corpus("wordperfect/wp51.wp") else {
        return;
    };
    for cut in (1..bytes.len()).step_by(613) {
        let head = &bytes[..cut];
        let _ = xberg_libwpd::is_supported(head);
        let _ = xberg_libwpd::extract_text(head);
        let _ = xberg_libwpd::extract_markdown(head);
    }
}
