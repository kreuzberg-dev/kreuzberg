//! Shared frontmatter and metadata utilities for markup extractors.
//!
//! This module provides common functionality for extractors that process
//! documents with YAML frontmatter (Markdown, Djot, etc.).
//!
//! This is a core module used by the Djot extractor (always available) and
//! the enhanced Markdown extractor (requires `office` feature).

use crate::types::{Metadata, ProcessingWarning};

use serde_yaml_ng::Value as YamlValue;
use std::borrow::Cow;

/// Frontmatter keys with a dedicated, typed home on [`Metadata`].
///
/// Any other top-level key found in a frontmatter mapping is preserved verbatim
/// into [`Metadata::additional`] instead of being silently dropped (xberg-io/xberg#154).
const KNOWN_FRONTMATTER_KEYS: &[&str] = &[
    "title",
    "author",
    "authors",
    "date",
    "keywords",
    "description",
    "abstract",
    "subject",
    "category",
    "tags",
    "language",
    "version",
];

/// The `diagnostics` module's `source` identifier used for frontmatter warnings.
const FRONTMATTER_WARNING_SOURCE: &str = "frontmatter";

/// Extract YAML frontmatter from document content.
///
/// Frontmatter is expected to be delimited by `---` or `...` at the start of the document.
/// This implementation properly handles edge cases:
/// - `---` appearing within YAML strings or arrays
/// - Both `---` and `...` as end delimiters (YAML spec compliant)
/// - Multiline YAML values containing dashes
///
/// Returns a tuple of (parsed YAML value, remaining content after frontmatter).
///
/// # Examples
///
/// ```rust,ignore
/// let content = "---\ntitle: Test\n---\n\n# Content";
/// let (yaml, remaining) = extract_frontmatter(content);
/// assert!(yaml.is_some());
/// assert!(remaining.contains("# Content"));
/// ```
/// Test-only since #154: every production caller (markdown, mdx, djot) now uses
/// [`extract_frontmatter_with_warning`] and attaches the warning to the document, so
/// discarding it is no longer acceptable outside tests that only assert on the parse.
#[cfg(test)]
pub(crate) fn extract_frontmatter(content: &str) -> (Option<YamlValue>, String) {
    let (yaml, remaining, _warning) = extract_frontmatter_with_warning(content);
    (yaml, remaining)
}

/// Same as [`extract_frontmatter`], but also reports a [`ProcessingWarning`] when
/// `---`/`...` delimiters are present yet the enclosed content fails to parse, or the
/// opening delimiter is never closed. Without this, malformed frontmatter and the
/// simple absence of frontmatter are indistinguishable to the caller — both silently
/// yield `(None, content)` (xberg-io/xberg#154).
pub(crate) fn extract_frontmatter_with_warning(
    content: &str,
) -> (Option<YamlValue>, String, Option<ProcessingWarning>) {
    if content.starts_with("+++") {
        let (yaml, remaining) = extract_toml_frontmatter(content);
        return (yaml, remaining, None);
    }

    if !content.starts_with("---") {
        return (None, content.to_string(), None);
    }

    let rest = &content[3..];

    let mut end_pos = None;
    let mut search_start = 0;

    while let Some(pos) = rest[search_start..].find('\n') {
        let absolute_pos = search_start + pos;
        let after_newline = absolute_pos + 1;

        if after_newline >= rest.len() {
            break;
        }

        let remaining = &rest[after_newline..];
        if remaining.starts_with("---") || remaining.starts_with("...") {
            let delimiter_end = after_newline + 3;
            if delimiter_end >= rest.len() || rest.as_bytes()[delimiter_end] == b'\n' {
                end_pos = Some(absolute_pos);
                break;
            }
        }

        search_start = after_newline;
    }

    if let Some(end) = end_pos {
        let frontmatter_str = &rest[..end];
        let after_delimiter = end + 1;
        let remaining_start = if after_delimiter + 3 < rest.len() {
            let after_delim = after_delimiter + 3;
            if after_delim < rest.len() && rest.as_bytes()[after_delim] == b'\n' {
                after_delim + 1
            } else {
                after_delim
            }
        } else {
            rest.len()
        };

        let remaining = if remaining_start < rest.len() {
            &rest[remaining_start..]
        } else {
            ""
        };

        match serde_yaml_ng::from_str::<YamlValue>(frontmatter_str) {
            Ok(value) => (Some(value), remaining.to_string(), None),
            Err(err) => (
                None,
                content.to_string(),
                Some(crate::core::diagnostics::warning(
                    FRONTMATTER_WARNING_SOURCE,
                    format!("frontmatter delimiters found but the YAML between them failed to parse: {err}"),
                )),
            ),
        }
    } else {
        (
            None,
            content.to_string(),
            Some(crate::core::diagnostics::warning(
                FRONTMATTER_WARNING_SOURCE,
                "frontmatter opening delimiter (`---`) found but no matching closing delimiter; treating the \
                 entire document as plain content"
                    .to_string(),
            )),
        )
    }
}

/// Extract TOML frontmatter (`+++...+++`), as used by Hugo and Zola.
///
/// TOML frontmatter is delimited by a `+++` line at the very start of the document and a
/// matching `+++` line on its own. The parsed TOML table is converted into a
/// [`YamlValue`] so downstream code (`extract_metadata_from_yaml`) needs no TOML-specific
/// path.
///
/// Returns `(None, content)` unchanged if the closing delimiter is missing or the TOML
/// fails to parse, mirroring the YAML frontmatter fallback behavior.
fn extract_toml_frontmatter(content: &str) -> (Option<YamlValue>, String) {
    let rest = &content[3..];

    let Some(newline_after_open) = rest.find('\n') else {
        return (None, content.to_string());
    };

    // Anything after `+++` on the opening line (other than whitespace) means this isn't a
    // bare delimiter, so treat it as ordinary content rather than frontmatter.
    if !rest[..newline_after_open].trim().is_empty() {
        return (None, content.to_string());
    }

    let body_start = newline_after_open + 1;
    let body = &rest[body_start..];

    let Some(close_pos) = body.lines().position(|line| line.trim_end() == "+++") else {
        return (None, content.to_string());
    };

    let toml_str: String = body.lines().take(close_pos).collect::<Vec<_>>().join("\n");

    // Recompute the byte offset of the remaining content after the closing delimiter line,
    // since `lines()` does not preserve line-ending byte widths.
    let mut offset = 0usize;
    for line in body.lines().take(close_pos + 1) {
        offset += line.len() + 1;
    }
    let remaining = if offset < body.len() { &body[offset..] } else { "" };

    match toml::from_str::<toml::Value>(&toml_str) {
        Ok(value) => (Some(toml_value_to_yaml(&value)), remaining.to_string()),
        Err(_) => (None, content.to_string()),
    }
}

/// Convert a parsed TOML value into the equivalent [`YamlValue`] so TOML and YAML
/// frontmatter can share a single metadata-extraction path.
fn toml_value_to_yaml(value: &toml::Value) -> YamlValue {
    match value {
        toml::Value::String(s) => YamlValue::String(s.clone()),
        toml::Value::Integer(i) => YamlValue::Number((*i).into()),
        toml::Value::Float(f) => YamlValue::Number(serde_yaml_ng::Number::from(*f)),
        toml::Value::Boolean(b) => YamlValue::Bool(*b),
        toml::Value::Datetime(dt) => YamlValue::String(dt.to_string()),
        toml::Value::Array(arr) => YamlValue::Sequence(arr.iter().map(toml_value_to_yaml).collect()),
        toml::Value::Table(table) => {
            let mut map = serde_yaml_ng::Mapping::new();
            for (k, v) in table {
                map.insert(YamlValue::String(k.clone()), toml_value_to_yaml(v));
            }
            YamlValue::Mapping(map)
        }
    }
}

/// Extract metadata from YAML frontmatter.
///
/// Extracts the following YAML fields into Xberg metadata:
/// - **Standard fields**: title, author, date, description (as subject)
/// - **Extended fields**: abstract, subject, category, tags, language, version
/// - **Array fields** (keywords, tags): stored as `Vec<String>` in typed fields
///
/// # Arguments
///
/// * `yaml` - The parsed YAML value from frontmatter
///
/// # Returns
///
/// A `Metadata` struct populated with extracted fields
///
/// # Examples
///
/// ```rust,ignore
/// let yaml = serde_yaml_ng::from_str("title: Test\nauthor: John").unwrap();
/// let metadata = extract_metadata_from_yaml(&yaml);
/// assert_eq!(metadata.title.as_deref(), Some("Test"));
/// ```
pub(crate) fn extract_metadata_from_yaml(yaml: &YamlValue) -> Metadata {
    let mut metadata = Metadata::default();

    if let Some(title) = yaml.get("title").and_then(|v| v.as_str())
        && metadata.title.is_none()
    {
        metadata.title = Some(title.to_string());
    }

    // `author` may be a scalar ("Jane Doe") or a sequence (["Jane Doe", "John Roe"]);
    // the plural Hugo/Jekyll `authors` key is the same shape. `Metadata::authors` is the
    // typed home for all three. Decision: if both `author` and `authors` are present,
    // `authors` wins for `Metadata::authors` (it is the more specific, list-oriented key),
    // while `Metadata::created_by` is still populated from a scalar `author` regardless,
    // preserving the pre-existing single-author behavior.
    if let Some(author_value) = yaml.get("author") {
        if let Some(author) = author_value.as_str()
            && metadata.created_by.is_none()
        {
            metadata.created_by = Some(author.to_string());
        }
        if metadata.authors.is_none() {
            metadata.authors = yaml_scalar_or_sequence_to_strings(author_value);
        }
    }

    if let Some(authors_value) = yaml.get("authors")
        && let Some(authors) = yaml_scalar_or_sequence_to_strings(authors_value)
    {
        metadata.authors = Some(authors);
    }

    if let Some(date) = yaml.get("date").and_then(|v| v.as_str()) {
        metadata.created_at = Some(date.to_string());
    }

    if let Some(keywords) = yaml.get("keywords") {
        match keywords {
            YamlValue::String(s) if metadata.keywords.is_none() => {
                metadata.keywords = Some(s.split(',').map(|k| k.trim().to_string()).collect());
            }
            YamlValue::Sequence(seq) => {
                let kw_vec: Vec<String> = seq.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
                if metadata.keywords.is_none() {
                    metadata.keywords = Some(kw_vec);
                }
            }
            _ => {}
        }
    }

    if let Some(description) = yaml.get("description").and_then(|v| v.as_str()) {
        metadata.subject = Some(description.to_string());
    }

    if let Some(abstract_val) = yaml.get("abstract").and_then(|v| v.as_str()) {
        metadata.abstract_text = Some(abstract_val.to_string());
    }

    if let Some(subject) = yaml.get("subject").and_then(|v| v.as_str()) {
        metadata.subject = Some(subject.to_string());
    }

    if let Some(category) = yaml.get("category").and_then(|v| v.as_str()) {
        metadata.category = Some(category.to_string());
    }

    if let Some(tags) = yaml.get("tags") {
        match tags {
            YamlValue::String(s) => {
                metadata.tags = Some(s.split(',').map(|t| t.trim().to_string()).collect());
            }
            YamlValue::Sequence(seq) => {
                let tags_vec: Vec<String> = seq.iter().filter_map(|v| v.as_str()).map(|s| s.to_string()).collect();
                metadata.tags = Some(tags_vec);
            }
            _ => {}
        }
    }

    if let Some(language) = yaml.get("language").and_then(|v| v.as_str())
        && metadata.language.is_none()
    {
        metadata.language = Some(language.to_string());
    }

    if let Some(version) = yaml.get("version").and_then(|v| v.as_str()) {
        metadata.document_version = Some(version.to_string());
    }

    // Preserve every top-level key without a typed field above (Hugo/Jekyll/Obsidian
    // extras like `aliases`, `slug`, `series`, `weight`, `draft`, or arbitrary custom
    // keys) instead of silently dropping them (xberg-io/xberg#154). YAML structure is
    // kept faithfully — a sequence stays a `serde_json::Value::Array`, not a joined string.
    if let YamlValue::Mapping(map) = yaml {
        for (key, value) in map {
            let Some(key_str) = key.as_str() else { continue };
            if KNOWN_FRONTMATTER_KEYS.contains(&key_str) {
                continue;
            }
            if let Ok(json_value) = serde_json::to_value(value) {
                metadata.additional.insert(Cow::Owned(key_str.to_string()), json_value);
            }
        }
    }

    metadata
}

/// Convert a YAML scalar string or a sequence of strings into a `Vec<String>`.
///
/// Used for frontmatter fields that may appear either as a single value
/// (`author: Jane Doe`) or as a list (`author: [Jane Doe, John Roe]`,
/// `authors: [...]`). Returns `None` for an empty sequence or any other shape.
fn yaml_scalar_or_sequence_to_strings(value: &YamlValue) -> Option<Vec<String>> {
    match value {
        YamlValue::String(s) => Some(vec![s.clone()]),
        YamlValue::Sequence(seq) => {
            let values: Vec<String> = seq.iter().filter_map(|v| v.as_str()).map(str::to_string).collect();
            if values.is_empty() { None } else { Some(values) }
        }
        _ => None,
    }
}

/// Extract first heading as title from content.
///
/// Searches for the first level-1 heading (# Title) in the content
/// and returns it as a potential title if no title was found in frontmatter.
///
/// # Arguments
///
/// * `content` - The document content to search
///
/// # Returns
///
/// Some(title) if a heading is found, None otherwise
///
/// # Examples
///
/// ```rust,ignore
/// let content = "# My Document\n\nContent here";
/// assert_eq!(extract_title_from_content(content), Some("My Document".to_string()));
/// ```
pub(crate) fn extract_title_from_content(content: &str) -> Option<String> {
    for line in content.lines() {
        if let Some(heading) = line.strip_prefix("# ") {
            return Some(heading.trim().to_string());
        }
    }
    None
}

/// Convert table cells to markdown format.
///
/// Takes a 2D array of cell values and formats them as a markdown table
/// with header row, separator row, and data rows.
///
/// Delegates to the crate's single table renderer
/// ([`crate::rendering::common::render_table_markdown`]) so markdown/MDX and
/// PDF-structure tables serialise identically to every other source
/// (xberg-io/xberg#220). That renderer escapes `|` and line breaks in cell
/// content, which this copy did not — an unescaped pipe used to split one cell
/// into two columns (xberg-io/xberg#163).
///
/// # Arguments
///
/// * `cells` - A 2D array where cells[0] is the header row
///
/// # Returns
///
/// A string containing the markdown-formatted table
///
/// # Examples
///
/// ```rust,ignore
/// let cells = vec![
///     vec!["Name".to_string(), "Age".to_string()],
///     vec!["Alice".to_string(), "30".to_string()],
/// ];
/// let markdown = cells_to_markdown(&cells);
/// assert!(markdown.contains("| Name | Age |"));
/// ```
pub(crate) fn cells_to_markdown(cells: &[Vec<String>]) -> String {
    crate::rendering::common::render_table_markdown(cells)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_frontmatter_basic() {
        let content = "---\ntitle: Test\n---\n\n# Content";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_some());
        assert!(remaining.contains("# Content"));

        let metadata = extract_metadata_from_yaml(&yaml.unwrap());
        assert_eq!(metadata.title.as_deref(), Some("Test"));
    }

    #[test]
    fn test_frontmatter_with_dashes_in_content() {
        let content = "---\ntitle: Test\ndescription: |\n  This has ---\n  in the middle\n---\n\n# Body";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_some());
        assert!(remaining.contains("# Body"));
    }

    #[test]
    fn test_frontmatter_with_dots_terminator() {
        let content = "---\ntitle: Test\nauthor: John\n...\n\n# Content";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_some());
        assert!(remaining.contains("# Content"));

        let metadata = extract_metadata_from_yaml(&yaml.unwrap());
        assert_eq!(metadata.title.as_deref(), Some("Test"));
    }

    #[test]
    fn test_frontmatter_with_triple_dash_in_string() {
        let content = "---\ntitle: \"Before --- After\"\nauthor: John\n---\n\n# Content";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_some());
        assert!(remaining.contains("# Content"));

        let metadata = extract_metadata_from_yaml(&yaml.unwrap());
        assert_eq!(metadata.title.as_deref(), Some("Before --- After"));
    }

    #[test]
    fn test_frontmatter_multiline_string_with_dashes() {
        let content = "---\ntitle: Test\ndescription: |\n  Line 1\n  ---\n  Line 2\n---\n\n# Body";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_some());
        assert!(remaining.contains("# Body"));

        let metadata = extract_metadata_from_yaml(&yaml.unwrap());
        assert_eq!(metadata.title.as_deref(), Some("Test"));
    }

    #[test]
    fn test_no_frontmatter() {
        let content = "# Title\n\nContent without frontmatter";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_none());
        assert_eq!(remaining, content);
    }

    #[test]
    fn test_incomplete_frontmatter() {
        let content = "---\ntitle: Test\nauthor: John\n\n# Content";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_none());
        assert_eq!(remaining, content);
    }

    #[test]
    fn test_extract_title_from_content() {
        let content = "# My Document\n\nContent here";
        assert_eq!(extract_title_from_content(content), Some("My Document".to_string()));
    }

    #[test]
    fn test_extract_title_from_content_no_heading() {
        let content = "Content without heading";
        assert_eq!(extract_title_from_content(content), None);
    }

    #[test]
    fn test_extract_title_from_content_level_2() {
        let content = "## Subheading\n\nContent";
        assert_eq!(extract_title_from_content(content), None);
    }

    #[test]
    fn test_cells_to_markdown() {
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
            vec!["Bob".to_string(), "25".to_string()],
        ];

        let markdown = cells_to_markdown(&cells);
        assert!(markdown.contains("| Name | Age |"));
        assert!(markdown.contains("| Alice | 30 |"));
        assert!(markdown.contains("| Bob | 25 |"));
        assert!(markdown.contains("| --- | --- |"));
    }

    #[test]
    fn test_cells_to_markdown_empty() {
        let cells: Vec<Vec<String>> = vec![];
        let markdown = cells_to_markdown(&cells);
        assert_eq!(markdown, "");
    }

    #[test]
    fn test_metadata_from_yaml_all_fields() {
        let yaml_str = r#"
title: Test Document
author: John Doe
date: 2024-01-15
keywords:
  - rust
  - testing
description: A test document
abstract: This is an abstract
subject: Test Subject
category: Documentation
tags:
  - tag1
  - tag2
language: en
version: 1.0
"#;

        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert_eq!(metadata.title.as_deref(), Some("Test Document"));
        assert_eq!(metadata.created_by.as_deref(), Some("John Doe"));
        assert_eq!(metadata.created_at, Some("2024-01-15".to_string()));
        assert!(metadata.keywords.is_some());
        assert_eq!(metadata.subject, Some("Test Subject".to_string()));
        assert!(metadata.tags.is_some());
    }

    #[test]
    fn test_metadata_from_yaml_string_arrays() {
        let yaml_str = r#"
keywords: "single, keyword, string"
tags: "tag1, tag2"
"#;

        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert_eq!(
            metadata.keywords.as_deref(),
            Some(["single", "keyword", "string"].map(String::from).as_slice())
        );
    }

    #[test]
    fn test_unknown_frontmatter_key_preserved_in_additional() {
        let yaml_str = "title: Test\nslug: my-post\nweight: 10\ndraft: true\naliases:\n  - /old-url\n";
        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert_eq!(metadata.title.as_deref(), Some("Test"));
        assert_eq!(
            metadata.additional.get(&Cow::Borrowed("slug")).cloned(),
            Some(serde_json::Value::String("my-post".to_string()))
        );
        assert_eq!(
            metadata.additional.get(&Cow::Borrowed("weight")).cloned(),
            Some(serde_json::Value::Number(10.into()))
        );
        assert_eq!(
            metadata.additional.get(&Cow::Borrowed("draft")).cloned(),
            Some(serde_json::Value::Bool(true))
        );
        assert_eq!(
            metadata.additional.get(&Cow::Borrowed("aliases")).cloned(),
            Some(serde_json::Value::Array(vec![serde_json::Value::String(
                "/old-url".to_string()
            )]))
        );
    }

    #[test]
    fn test_known_keys_do_not_leak_into_additional() {
        let yaml_str = "title: Test\nauthor: Jane\ndate: 2024-01-15\nkeywords: [a, b]\ndescription: d\n\
                         abstract: ab\nsubject: s\ncategory: c\ntags: [t1]\nlanguage: en\nversion: \"1.0\"\n";
        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert!(
            metadata.additional.is_empty(),
            "expected no leaked keys, got {:?}",
            metadata.additional
        );
    }

    #[test]
    fn test_author_sequence_produces_two_authors() {
        let yaml_str = "author:\n  - A\n  - B\n";
        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert_eq!(metadata.authors, Some(vec!["A".to_string(), "B".to_string()]));
        // A sequence `author` has no single string to hang off `created_by`.
        assert_eq!(metadata.created_by, None);
    }

    #[test]
    fn test_plural_authors_key_produces_authors() {
        let yaml_str = "authors:\n  - A\n  - B\n";
        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert_eq!(metadata.authors, Some(vec!["A".to_string(), "B".to_string()]));
    }

    #[test]
    fn test_scalar_author_still_populates_created_by_and_authors() {
        let yaml_str = "author: Jane Doe\n";
        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        assert_eq!(metadata.created_by.as_deref(), Some("Jane Doe"));
        assert_eq!(metadata.authors, Some(vec!["Jane Doe".to_string()]));
    }

    #[test]
    fn test_both_author_and_authors_present_plural_wins_for_authors_field() {
        let yaml_str = "author: Jane Doe\nauthors:\n  - Jane Doe\n  - John Roe\n";
        let yaml: YamlValue = serde_yaml_ng::from_str(yaml_str).unwrap();
        let metadata = extract_metadata_from_yaml(&yaml);

        // Precedence decision (xberg-io/xberg#154): plural `authors` wins for the
        // `Metadata::authors` list, while `created_by` still reflects the scalar `author`.
        assert_eq!(
            metadata.authors,
            Some(vec!["Jane Doe".to_string(), "John Roe".to_string()])
        );
        assert_eq!(metadata.created_by.as_deref(), Some("Jane Doe"));
    }

    #[test]
    fn test_malformed_yaml_frontmatter_emits_warning() {
        // Unterminated flow mapping - delimiters are present but the YAML is unparseable.
        let content = "---\ntitle: [unterminated\n---\n\nBody";
        let (yaml, remaining, warning) = extract_frontmatter_with_warning(content);

        assert!(yaml.is_none());
        assert_eq!(remaining, content);
        let warning = warning.expect("expected a ProcessingWarning for malformed frontmatter");
        assert_eq!(warning.source.as_ref(), "frontmatter");
        assert_eq!(
            warning.message.as_ref(),
            "frontmatter delimiters found but the YAML between them failed to parse: did not find expected ',' \
             or ']' at line 3 column 1, while parsing a flow sequence at line 2 column 8"
        );
    }

    #[test]
    fn test_unclosed_frontmatter_delimiter_emits_warning() {
        let content = "---\ntitle: Test\nauthor: John\n\n# Content";
        let (yaml, remaining, warning) = extract_frontmatter_with_warning(content);

        assert!(yaml.is_none());
        assert_eq!(remaining, content);
        let warning = warning.expect("expected a ProcessingWarning for an unclosed delimiter");
        assert_eq!(warning.source.as_ref(), "frontmatter");
        assert_eq!(
            warning.message.as_ref(),
            "frontmatter opening delimiter (`---`) found but no matching closing delimiter; treating the entire \
             document as plain content"
        );
    }

    #[test]
    fn test_extract_frontmatter_drops_warning_for_backward_compatibility() {
        // The 2-tuple `extract_frontmatter` is the existing public(crate) entry point used by
        // the markdown/mdx/djot extractors; it must keep behaving exactly as before.
        let content = "---\ntitle: [unterminated\n---\n\nBody";
        let (yaml, remaining) = extract_frontmatter(content);

        assert!(yaml.is_none());
        assert_eq!(remaining, content);
    }
}
