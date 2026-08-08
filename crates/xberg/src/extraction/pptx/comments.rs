//! PPTX slide comment extraction.
//!
//! Parses `ppt/comments/comment{N}.xml` and `ppt/commentAuthors.xml` to
//! produce `DocumentRevision { kind: Comment }` entries for each `<p:cm>`
//! element found in the presentation.

use std::collections::HashMap;
use std::io::{Read, Seek};

use crate::core::diagnostics::push_warning;
use crate::extraction::ooxml_constants::PRESENTATIONML_NAMESPACE;
use crate::types::ProcessingWarning;
use crate::types::revisions::{DiffLine, DocumentRevision, RevisionAnchor, RevisionDelta, RevisionKind};

use super::container::PptxContainer;

/// Extract all slide comments as `DocumentRevision` entries.
///
/// Loads `ppt/commentAuthors.xml` for the `authorId → name` map, then
/// iterates `ppt/comments/comment{N}.xml` (one file per slide, 1-indexed)
/// in slide order.
///
/// Returns `None` when no comment XML parts exist in the archive, and
/// `Some(vec![])` when files are present but contain no `<p:cm>` elements.
/// A comment file or the author list that exists but fails to parse is
/// surfaced as a `ProcessingWarning` naming the part, rather than only
/// logging and silently discarding those comments (#238).
pub(super) fn extract_comments<R: Read + Seek>(
    container: &mut PptxContainer<R>,
    slide_paths: &[String],
    warnings: &mut Vec<ProcessingWarning>,
) -> Option<Vec<DocumentRevision>> {
    let author_map = load_author_map(container, warnings);
    let mut all_comments: Vec<DocumentRevision> = Vec::new();
    let mut any_comment_file_found = false;

    for (slide_idx, _) in slide_paths.iter().enumerate() {
        let comment_path = format!("ppt/comments/comment{}.xml", slide_idx + 1);
        let xml_bytes = match container.read_file(&comment_path) {
            Ok(b) => {
                any_comment_file_found = true;
                b
            }
            Err(_) => continue,
        };

        let slide_index = slide_idx;

        match parse_comment_xml(&xml_bytes, slide_index, &author_map) {
            Ok(revisions) => all_comments.extend(revisions),
            Err(e) => {
                tracing::warn!(
                    path = %comment_path,
                    error = %e,
                    "failed to parse PPTX comment file; skipping slide comments"
                );
                push_warning(
                    warnings,
                    "pptx",
                    format!(
                        "Could not parse comments for slide {} ('{}'): {}; comments were not extracted",
                        slide_idx + 1,
                        comment_path,
                        e
                    ),
                );
            }
        }
    }

    if any_comment_file_found {
        Some(all_comments)
    } else {
        None
    }
}

/// Load `ppt/commentAuthors.xml` and return an `authorId → name` map.
///
/// Returns an empty map when the file is absent. A file that exists but
/// fails to parse is surfaced as a `ProcessingWarning`, since comments will
/// then be missing author attribution (#238).
fn load_author_map<R: Read + Seek>(
    container: &mut PptxContainer<R>,
    warnings: &mut Vec<ProcessingWarning>,
) -> HashMap<u32, String> {
    let xml_bytes = match container.read_file("ppt/commentAuthors.xml") {
        Ok(b) => b,
        Err(_) => return HashMap::new(),
    };
    match parse_comment_authors_xml(&xml_bytes) {
        Ok(map) => map,
        Err(e) => {
            tracing::warn!(error = %e, "failed to parse ppt/commentAuthors.xml");
            push_warning(
                warnings,
                "pptx",
                format!(
                    "Could not parse ppt/commentAuthors.xml: {}; comment authors were not resolved",
                    e
                ),
            );
            HashMap::new()
        }
    }
}

/// Parse `ppt/commentAuthors.xml` and return `authorId → name`.
fn parse_comment_authors_xml(xml_bytes: &[u8]) -> crate::error::Result<HashMap<u32, String>> {
    let xml_str = crate::text::utf8_validation::from_utf8(xml_bytes)
        .map_err(|e| crate::error::XbergError::parsing(format!("invalid UTF-8 in commentAuthors.xml: {e}")))?;

    let doc = roxmltree::Document::parse(xml_str)
        .map_err(|e| crate::error::XbergError::parsing(format!("failed to parse commentAuthors.xml: {e}")))?;

    let mut map = HashMap::new();
    for node in doc.descendants() {
        if node.has_tag_name((PRESENTATIONML_NAMESPACE, "cmAuthor")) {
            let id = node
                .attribute("id")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(u32::MAX);
            if let Some(name) = node.attribute("name").filter(|s| !s.is_empty()) {
                map.insert(id, name.to_string());
            }
        }
    }
    Ok(map)
}

/// Parse one `ppt/comments/comment{N}.xml` file and return its comments as
/// `DocumentRevision` entries anchored to `slide_index` (0-based).
fn parse_comment_xml(
    xml_bytes: &[u8],
    slide_index: usize,
    author_map: &HashMap<u32, String>,
) -> crate::error::Result<Vec<DocumentRevision>> {
    let xml_str = crate::text::utf8_validation::from_utf8(xml_bytes)
        .map_err(|e| crate::error::XbergError::parsing(format!("invalid UTF-8 in comment XML: {e}")))?;

    let doc = roxmltree::Document::parse(xml_str)
        .map_err(|e| crate::error::XbergError::parsing(format!("failed to parse comment XML: {e}")))?;

    let mut revisions = Vec::new();
    let mut seq: usize = 0;

    for cm in doc.descendants() {
        if !cm.has_tag_name((PRESENTATIONML_NAMESPACE, "cm")) {
            continue;
        }

        let revision_id = cm
            .attribute("idx")
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| {
                let id = format!("pptx-comment-slide-{}-{}", slide_index + 1, seq);
                seq += 1;
                id
            });

        let author_id: Option<u32> = cm.attribute("authorId").and_then(|s| s.parse().ok());
        let author = author_id.and_then(|id| author_map.get(&id)).cloned();

        let timestamp = cm.attribute("dt").filter(|s| !s.is_empty()).map(str::to_string);

        let comment_text: String = cm
            .descendants()
            .filter(|n| n.has_tag_name((PRESENTATIONML_NAMESPACE, "text")))
            .filter_map(|n| n.text())
            .collect::<Vec<_>>()
            .join(" ");

        let delta = if comment_text.is_empty() {
            RevisionDelta::default()
        } else {
            RevisionDelta {
                content: vec![DiffLine::Context(comment_text)],
                ..Default::default()
            }
        };

        revisions.push(DocumentRevision {
            revision_id,
            author,
            timestamp,
            kind: RevisionKind::Comment,
            anchor: Some(RevisionAnchor::Slide { index: slide_index }),
            delta,
        });
    }

    Ok(revisions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_author_map(entries: &[(u32, &str)]) -> HashMap<u32, String> {
        entries.iter().map(|(id, name)| (*id, name.to_string())).collect()
    }

    /// Build minimal `ppt/commentAuthors.xml` bytes.
    fn make_authors_xml(authors: &[(u32, &str)]) -> Vec<u8> {
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<p:cmAuthorLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">"#,
        );
        for (id, name) in authors {
            xml.push_str(&format!(
                r#"<p:cmAuthor id="{id}" name="{name}" initials="A" lastIdx="0" clrIdx="0"/>"#
            ));
        }
        xml.push_str("</p:cmAuthorLst>");
        xml.into_bytes()
    }

    /// Build a minimal `ppt/comments/comment{N}.xml` with the given comments.
    ///
    /// Each entry is `(idx, author_id, datetime, text)`.
    fn make_comment_xml(comments: &[(u32, u32, &str, &str)]) -> Vec<u8> {
        let mut xml = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">"#,
        );
        for (idx, author_id, dt, text) in comments {
            xml.push_str(&format!(
                r#"<p:cm authorId="{author_id}" dt="{dt}" idx="{idx}"><p:text>{text}</p:text></p:cm>"#
            ));
        }
        xml.push_str("</p:cmLst>");
        xml.into_bytes()
    }

    #[test]
    fn should_parse_comment_authors_xml() {
        let xml = make_authors_xml(&[(0, "Alice"), (1, "Bob")]);
        let map = parse_comment_authors_xml(&xml).expect("should parse");
        assert_eq!(map.get(&0).map(String::as_str), Some("Alice"));
        assert_eq!(map.get(&1).map(String::as_str), Some("Bob"));
    }

    #[test]
    fn should_parse_single_comment_with_known_author() {
        let author_map = make_author_map(&[(0, "Alice")]);
        let xml = make_comment_xml(&[(1, 0, "2024-03-15T10:30:00Z", "Please revise this slide")]);
        let revisions = parse_comment_xml(&xml, 0, &author_map).expect("should parse");

        assert_eq!(revisions.len(), 1);
        let rev = &revisions[0];
        assert_eq!(rev.revision_id, "1");
        assert_eq!(rev.author.as_deref(), Some("Alice"));
        assert_eq!(rev.timestamp.as_deref(), Some("2024-03-15T10:30:00Z"));
        assert!(matches!(rev.kind, RevisionKind::Comment));
        assert!(
            matches!(&rev.anchor, Some(RevisionAnchor::Slide { index: 0 })),
            "anchor should be Slide {{ index: 0 }} for the first slide"
        );
        assert_eq!(rev.delta.content.len(), 1);
        assert!(matches!(&rev.delta.content[0], DiffLine::Context(t) if t == "Please revise this slide"));
    }

    #[test]
    fn should_resolve_author_none_when_author_id_is_unmapped() {
        let author_map = make_author_map(&[(0, "Alice")]);
        let xml = make_comment_xml(&[(1, 99, "2024-03-15T10:30:00Z", "Orphan comment")]);
        let revisions = parse_comment_xml(&xml, 2, &author_map).expect("should parse");

        assert_eq!(revisions.len(), 1);
        assert!(
            revisions[0].author.is_none(),
            "author should be None when authorId is not in the map"
        );
        assert!(
            matches!(&revisions[0].anchor, Some(RevisionAnchor::Slide { index: 2 })),
            "slide anchor should match the provided slide index"
        );
    }

    #[test]
    fn should_parse_multiple_comments_preserving_document_order() {
        let author_map = make_author_map(&[(0, "Alice"), (1, "Bob")]);
        let xml = make_comment_xml(&[
            (1, 0, "2024-03-15T09:00:00Z", "First comment"),
            (2, 1, "2024-03-15T10:00:00Z", "Second comment"),
            (3, 0, "2024-03-15T11:00:00Z", "Third comment"),
        ]);
        let revisions = parse_comment_xml(&xml, 1, &author_map).expect("should parse");

        assert_eq!(revisions.len(), 3);
        assert_eq!(revisions[0].revision_id, "1");
        assert_eq!(revisions[0].author.as_deref(), Some("Alice"));
        assert_eq!(revisions[1].revision_id, "2");
        assert_eq!(revisions[1].author.as_deref(), Some("Bob"));
        assert_eq!(revisions[2].revision_id, "3");
        for rev in &revisions {
            assert!(matches!(&rev.anchor, Some(RevisionAnchor::Slide { index: 1 })));
        }
    }
}
