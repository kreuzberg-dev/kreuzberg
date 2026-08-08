//! Render an `InternalDocument` to GFM-compliant Markdown via comrak.

use comrak::{Arena, Options, format_commonmark};
use std::borrow::Cow;
use std::sync::LazyLock;

use crate::types::annotations::PdfAnnotation;
use crate::types::internal::InternalDocument;

use super::common::{annotation_display_text, annotation_type_label};
use super::comrak_bridge::build_comrak_ast;

/// Single-pass replacement of multiple two-char escape sequences of the form `\X`
/// where X is one of `_`, `[`, `]`, `(`, `)`.
///
/// Returns [`Cow::Borrowed`] when no replacement occurs (zero allocation).
/// Returns [`Cow::Owned`] with one pre-sized allocation when any hit is found.
fn unescape_backslash_sequences<'a>(input: &'a str, targets: &[char]) -> Cow<'a, str> {
    let bytes = input.as_bytes();
    let len = bytes.len();
    let mut i = 0usize;

    let first_hit = loop {
        if i + 1 >= len {
            return Cow::Borrowed(input);
        }
        if bytes[i] == b'\\' {
            let next = bytes[i + 1] as char;
            if targets.contains(&next) {
                break i;
            }
        }
        i += 1;
    };

    let mut out = String::with_capacity(input.len());
    out.push_str(&input[..first_hit]);
    i = first_hit;

    while i < len {
        if i + 1 < len && bytes[i] == b'\\' {
            let next = bytes[i + 1] as char;
            if targets.contains(&next) {
                out.push(next);
                i += 2;
                continue;
            }
        }
        let c = input[i..].chars().next().expect("valid UTF-8");
        out.push(c);
        i += c.len_utf8();
    }

    Cow::Owned(out)
}

/// Single-pass replacement of `&#10;` → space and `&#2;` → removed.
///
/// Returns [`Cow::Borrowed`] when neither entity appears (zero allocation).
fn replace_html_entities(input: &str) -> Cow<'_, str> {
    if !input.contains("&#10;") && !input.contains("&#2;") {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len());
    let mut rest = input;

    while !rest.is_empty() {
        if let Some(pos) = rest.find("&#") {
            out.push_str(&rest[..pos]);
            let after = &rest[pos..];
            if let Some(tail) = after.strip_prefix("&#10;") {
                out.push(' ');
                rest = tail;
            } else if let Some(tail) = after.strip_prefix("&#2;") {
                rest = tail;
            } else {
                out.push_str("&#");
                rest = &after[2..];
            }
        } else {
            out.push_str(rest);
            break;
        }
    }

    Cow::Owned(out)
}

/// Collapse runs of three or more consecutive newlines down to exactly two
/// using a single pass over the string.
///
/// Returns [`Cow::Borrowed`] when no triple-newline sequence is found.
fn collapse_excess_newlines(input: &str) -> Cow<'_, str> {
    if !input.contains("\n\n\n") {
        return Cow::Borrowed(input);
    }

    let mut out = String::with_capacity(input.len());
    let mut newline_run = 0usize;

    for c in input.chars() {
        if c == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                out.push('\n');
            }
        } else {
            newline_run = 0;
            out.push(c);
        }
    }

    Cow::Owned(out)
}

static ARXIV_WATERMARK_REGEX: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?:\s+\S+(?:\s+\S+){0,8})?\s*arXiv:\d{4}\.\d{4,5}(?:v\d+)?(?:\s*\[[\w.-]+\])?\s*(?:\d{1,2}\s+\w+\s+\d{4})?",
    )
    .expect("static regex compiles")
});

/// Render an `InternalDocument` to GFM Markdown.
///
/// Whether the output backslash-escapes CommonMark-significant leading characters
/// (`-`, `#`, plus the always-unescaped `_[]()*=`) is controlled by
/// `doc.escape_markdown`, which the pipeline sets from
/// [`ExtractionConfig::escape_markdown`](crate::core::config::ExtractionConfig::escape_markdown).
/// Defaults to `true` (escaped), preserving prior behavior.
pub(crate) fn render_markdown(doc: &InternalDocument) -> String {
    tracing::debug!(element_count = doc.elements.len(), "markdown rendering starting");
    let arena = Arena::new();
    let root = build_comrak_ast(doc, &arena);

    if root.first_child().is_none() {
        tracing::debug!("markdown rendering: empty AST, returning empty string");
        return finalize_annotations_appendix(doc.annotations.as_deref());
    }

    let mut options = comrak_options();
    options.render.width = 0;

    let mut output = String::new();
    format_commonmark(root, &options, &mut output).expect("comrak formatting should not fail");

    if output.contains("<!--") {
        let marker_re = doc
            .page_marker_format
            .as_deref()
            .map(crate::core::config::page::marker_line_regex);
        output = output
            .lines()
            .filter(|line| {
                let trimmed = line.trim();
                !trimmed.starts_with("<!--")
                    || !trimmed.ends_with("-->")
                    || marker_re.as_ref().is_some_and(|re| re.is_match(trimmed))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }

    if let Cow::Owned(s) = replace_html_entities(&output) {
        output = s;
    }

    if doc.escape_markdown {
        const UNESCAPE_TARGETS: &[char] = &['_', '[', ']', '(', ')', '*', '='];
        let cow = unescape_backslash_sequences(&output, UNESCAPE_TARGETS);
        if let Cow::Owned(s) = cow {
            output = s;
        }

        if output.contains("\\*") || output.contains("\\#") {
            output = output
                .lines()
                .map(|line| {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("\\* ") || trimmed.starts_with("\\#.") || trimmed.starts_with("\\#\\.") {
                        line.replacen("\\*", "*", 1).replacen("\\#", "#", 1)
                    } else {
                        line.to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
    } else {
        const UNESCAPE_TARGETS: &[char] = &['_', '[', ']', '(', ')', '*', '=', '-', '#'];
        let cow = unescape_backslash_sequences(&output, UNESCAPE_TARGETS);
        if let Cow::Owned(s) = cow {
            output = s;
        }
    }

    if let Cow::Owned(s) = collapse_excess_newlines(&output) {
        output = s;
    }

    output = strip_arxiv_watermark_noise(output);

    if let Some(annotations) = doc.annotations.as_deref() {
        let block = render_annotations_markdown(annotations);
        if !block.is_empty() {
            if !output.trim_end().is_empty() {
                output.push_str("\n\n");
            }
            output.push_str(&block);
        }
    }

    let trimmed_len = output.trim_end().len();
    if trimmed_len == 0 {
        return String::new();
    }
    output.truncate(trimmed_len);
    output.push('\n');
    tracing::debug!(output_length = output.len(), "markdown rendering complete");
    output
}

/// Render the document-level PDF annotations (issue #63) as a Markdown
/// section: a `## Annotations` heading followed by one bullet per annotation.
///
/// Returns an empty string when `annotations` is empty, so callers can append
/// unconditionally without introducing spurious blank sections.
fn render_annotations_markdown(annotations: &[PdfAnnotation]) -> String {
    if annotations.is_empty() {
        return String::new();
    }

    let mut out = String::from("## Annotations\n\n");
    for annotation in annotations {
        out.push_str("- **");
        out.push_str(annotation_type_label(annotation.annotation_type));
        out.push_str("** (page ");
        out.push_str(&annotation.page_number.to_string());
        out.push(')');

        if let Some(author) = annotation.author.as_deref().filter(|s| !s.is_empty()) {
            out.push_str(" by ");
            out.push_str(author);
        }

        if let Some(text) = annotation_display_text(annotation) {
            out.push_str(": ");
            out.push_str(text);
        }

        out.push('\n');
    }
    out
}

/// Build the final annotations-only output when the document body itself
/// renders empty (e.g. a scanned page whose only content is a highlight).
fn finalize_annotations_appendix(annotations: Option<&[PdfAnnotation]>) -> String {
    let mut block = annotations.map(render_annotations_markdown).unwrap_or_default();
    let trimmed_len = block.trim_end().len();
    if trimmed_len == 0 {
        return String::new();
    }
    block.truncate(trimmed_len);
    block.push('\n');
    block
}

/// Strip arXiv watermark noise from rendered markdown.
///
/// LaTeX-generated PDFs often have a rotated sidebar with the arXiv identifier
/// that the PDF extractor concatenates with body text. This strips patterns like:
/// "Title N arXiv:NNNN.NNNNNvN [cat.SC] DD Mon YYYY" from the first pages.
fn strip_arxiv_watermark_noise(mut text: String) -> String {
    let search_limit = text.floor_char_boundary(text.len().min(6000));
    let search_area = &text[..search_limit];

    if let Some(m) = ARXIV_WATERMARK_REGEX.find(search_area) {
        let after = &search_area[m.end()..];
        let before_char = if m.start() > 0 {
            search_area[..m.start()].chars().last()
        } else {
            None
        };

        let is_at_paragraph_boundary = before_char == Some('.') || after.starts_with('\n') || after.starts_with("\n\n");
        if is_at_paragraph_boundary {
            let start = m.start();
            let end = m.end();
            tracing::trace!(
                stripped = %&text[start..end].chars().take(80).collect::<String>(),
                "stripping arXiv watermark from markdown output"
            );
            text.replace_range(start..end, "");
        }
    }

    text
}

/// Shared comrak options with all GFM extensions enabled.
pub(crate) fn comrak_options<'a>() -> Options<'a> {
    let mut options = Options::default();
    options.extension.table = true;
    options.extension.strikethrough = true;
    options.extension.footnotes = true;
    options.extension.description_lists = true;
    options.extension.math_dollars = true;
    options.extension.underline = true;
    options.extension.subscript = true;
    options.extension.superscript = true;
    options.extension.highlight = true;
    options.extension.alerts = true;
    options.render.prefer_fenced = true;
    options
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal_builder::InternalDocumentBuilder;

    /// Issue #1292: by default, prose containing a leading `#06-18`-style token
    /// keeps its backslash escapes so the markdown round-trips safely through a
    /// CommonMark parser. This must remain the behavior when `escape_markdown`
    /// is left at its default (`true`).
    #[test]
    fn render_markdown_default_escapes_leading_hash_and_dash_in_prose() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("#06-18 widget replacement", vec![], None, None);
        let doc = b.build();
        assert!(doc.escape_markdown, "escape_markdown must default to true");

        let rendered = render_markdown(&doc);
        assert!(
            rendered.contains("\\#06-18"),
            "expected default rendering to keep the backslash escape: {rendered}"
        );
    }

    /// Issue #1292: a bare "- clause" paragraph must still render with its
    /// leading dash escaped by default, otherwise a CommonMark parser would
    /// reinterpret it as a list item.
    #[test]
    fn render_markdown_default_escapes_leading_dash_in_prose() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("- clause applies here", vec![], None, None);
        let doc = b.build();

        let rendered = render_markdown(&doc);
        assert!(
            rendered.contains("\\- clause"),
            "expected default rendering to escape a leading dash: {rendered}"
        );
    }

    /// Issue #1292: `escape_markdown = false` strips the `#`/`-` escapes so prose
    /// reads identically to the (always-clean) table cell text.
    #[test]
    fn render_markdown_escape_markdown_false_yields_clean_prose() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("#06-18 widget replacement", vec![], None, None);
        b.push_paragraph("- clause applies here", vec![], None, None);
        let mut doc = b.build();
        doc.escape_markdown = false;

        let rendered = render_markdown(&doc);
        assert!(
            rendered.contains("#06-18 widget replacement"),
            "expected clean, unescaped hash: {rendered}"
        );
        assert!(!rendered.contains("\\#06-18"), "escape must be stripped: {rendered}");
        assert!(
            rendered.contains("- clause applies here"),
            "expected clean, unescaped dash: {rendered}"
        );
        assert!(!rendered.contains("\\- clause"), "escape must be stripped: {rendered}");
    }

    /// Issue #1292: with `escape_markdown = false`, the same literal text rendered
    /// in prose and inside a table cell must be byte-identical, since table cells
    /// are never escaped.
    #[test]
    fn render_markdown_escape_markdown_false_matches_table_cell_rendering() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("#06-18 widget replacement", vec![], None, None);
        b.push_table_from_cells(
            &[
                vec!["Part".to_string(), "Description".to_string()],
                vec!["#06-18".to_string(), "Widget".to_string()],
            ],
            None,
            None,
        );
        let mut doc = b.build();
        doc.escape_markdown = false;

        let rendered = render_markdown(&doc);
        assert!(
            rendered.contains("#06-18 widget replacement"),
            "prose must be clean: {rendered}"
        );
        assert!(
            rendered.contains("| #06-18 "),
            "table cell must remain clean and unescaped: {rendered}"
        );
        assert!(
            !rendered.contains("\\#06-18"),
            "no escaped variant should appear anywhere: {rendered}"
        );
    }

    /// Issue #1297: by default (`table_anchors = false`), rendered markdown
    /// contains no `[TABLE:...]` marker, so existing output stays unchanged.
    #[test]
    fn render_markdown_default_has_no_table_anchor() {
        let mut b = InternalDocumentBuilder::new("test");
        let idx = b.push_table_from_cells(&[vec!["A".to_string(), "B".to_string()]], None, None);
        let mut doc = b.build();
        doc.tables[idx as usize].table_id = Some("table-1".to_string());
        assert!(!doc.table_anchors, "table_anchors must default to false");

        let rendered = render_markdown(&doc);
        assert!(
            !rendered.contains("[TABLE:"),
            "no anchor should appear when table_anchors is disabled: {rendered}"
        );
    }

    /// Issue #1297: with `table_anchors = true`, rendered markdown contains a
    /// `[TABLE:{table_id}]` marker immediately before the table's markdown.
    #[test]
    fn render_markdown_table_anchors_enabled_emits_marker() {
        let mut b = InternalDocumentBuilder::new("test");
        let idx = b.push_table_from_cells(
            &[
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            None,
            None,
        );
        let mut doc = b.build();
        doc.tables[idx as usize].table_id = Some("table-1".to_string());
        doc.table_anchors = true;

        let rendered = render_markdown(&doc);
        assert!(
            rendered.contains("[TABLE:table-1]"),
            "expected the table anchor marker: {rendered}"
        );

        let anchor_pos = rendered.find("[TABLE:table-1]").unwrap();
        let table_pos = rendered.find("| Name").unwrap();
        assert!(
            anchor_pos < table_pos,
            "anchor must precede the table markdown: {rendered}"
        );
    }

    /// Issue #1297: when a table has no `table_id`, no anchor is emitted even
    /// with `table_anchors = true` (nothing to reference).
    #[test]
    fn render_markdown_table_anchors_enabled_without_table_id_emits_no_marker() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_table_from_cells(&[vec!["A".to_string(), "B".to_string()]], None, None);
        let mut doc = b.build();
        doc.table_anchors = true;

        let rendered = render_markdown(&doc);
        assert!(
            !rendered.contains("[TABLE:"),
            "no anchor should appear without a table_id: {rendered}"
        );
    }

    #[test]
    fn unescape_backslash_sequences_empty_input_returns_borrowed() {
        let result = unescape_backslash_sequences("", &['_']);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "");
    }

    #[test]
    fn unescape_backslash_sequences_no_targets_returns_borrowed() {
        let input = "hello world no escapes here";
        let result = unescape_backslash_sequences(input, &['_', '[', ']', '(', ')']);
        assert!(
            matches!(result, Cow::Borrowed(_)),
            "expected Cow::Borrowed when no target sequence present"
        );
        assert_eq!(result, input);
    }

    #[test]
    fn unescape_backslash_sequences_single_hit_returns_owned_correct_content() {
        let result = unescape_backslash_sequences("hello\\_world", &['_']);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "hello_world");
    }

    #[test]
    fn unescape_backslash_sequences_multiple_targets_all_replaced() {
        let input = "\\[link\\](url\\) and \\[another\\]";
        let result = unescape_backslash_sequences(input, &['[', ']', '(', ')']);
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "[link](url) and [another]");
    }

    #[test]
    fn unescape_backslash_sequences_backslash_not_followed_by_target_is_kept() {
        let input = "foo\\nbar";
        let result = unescape_backslash_sequences(input, &['_']);
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result, "foo\\nbar");
    }

    #[test]
    fn unescape_backslash_sequences_roundtrip_vs_chained_replace() {
        let input = "a\\_b\\[c\\]d\\(e\\)f";
        let expected = input
            .replace("\\_", "_")
            .replace("\\[", "[")
            .replace("\\]", "]")
            .replace("\\(", "(")
            .replace("\\)", ")");
        let result = unescape_backslash_sequences(input, &['_', '[', ']', '(', ')']);
        assert_eq!(result.as_ref(), expected.as_str());
    }

    #[test]
    fn replace_html_entities_empty_returns_borrowed() {
        let result = replace_html_entities("");
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn replace_html_entities_no_entities_returns_borrowed() {
        let input = "plain text with no HTML entities";
        let result = replace_html_entities(input);
        assert!(
            matches!(result, Cow::Borrowed(_)),
            "expected Cow::Borrowed when no &#xx; entities present"
        );
        assert_eq!(result, input);
    }

    #[test]
    fn replace_html_entities_newline_entity_becomes_space() {
        let result = replace_html_entities("line1&#10;line2");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "line1 line2");
    }

    #[test]
    fn replace_html_entities_stx_entity_is_removed() {
        let result = replace_html_entities("before&#2;after");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "beforeafter");
    }

    #[test]
    fn replace_html_entities_both_entities_in_one_pass() {
        let input = "a&#10;b&#2;c";
        let result = replace_html_entities(input);
        let expected = input.replace("&#10;", " ").replace("&#2;", "");
        assert_eq!(result.as_ref(), expected.as_str());
    }

    #[test]
    fn replace_html_entities_unknown_entity_is_kept_verbatim() {
        let input = "a&#42;b";
        let result = replace_html_entities(input);
        assert_eq!(result, "a&#42;b");
    }

    #[test]
    fn collapse_excess_newlines_empty_returns_borrowed() {
        let result = collapse_excess_newlines("");
        assert!(matches!(result, Cow::Borrowed(_)));
    }

    #[test]
    fn collapse_excess_newlines_no_triple_newline_returns_borrowed() {
        let input = "line1\n\nline2\n";
        let result = collapse_excess_newlines(input);
        assert!(
            matches!(result, Cow::Borrowed(_)),
            "expected Cow::Borrowed when no \\n\\n\\n present"
        );
        assert_eq!(result, input);
    }

    #[test]
    fn collapse_excess_newlines_triple_newline_collapsed_to_double() {
        let result = collapse_excess_newlines("a\n\n\nb");
        assert!(matches!(result, Cow::Owned(_)));
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn collapse_excess_newlines_many_newlines_collapsed() {
        let result = collapse_excess_newlines("a\n\n\n\n\n\nb");
        assert_eq!(result, "a\n\nb");
    }

    #[test]
    fn collapse_excess_newlines_equivalent_to_while_replace() {
        let input = "p1\n\n\n\np2\n\n\np3\n\n";
        let mut expected = input.to_string();
        while expected.contains("\n\n\n") {
            expected = expected.replace("\n\n\n", "\n\n");
        }
        let result = collapse_excess_newlines(input);
        assert_eq!(result.as_ref(), expected.as_str());
    }
}
