//! HTML document extractor.

use crate::Result;
use crate::core::config::{ExtractionConfig, OutputFormat};
use crate::extractors::SyncExtractor;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::text::utf8_validation;
#[cfg(test)]
use crate::types::Table;
use crate::types::document_structure::TextAnnotation;
use crate::types::extraction::ExtractedImage;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::uri::{ExtractedUri, classify_uri};
use crate::types::{HtmlMetadata, Metadata};
use async_trait::async_trait;
use bytes::Bytes;
use html_to_markdown_rs::InlineImageFormat;
use std::borrow::Cow;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const HTML_WARNING_SOURCE: &str = "html";

#[cfg_attr(alef, alef(skip))]
/// HTML document extractor using html-to-markdown.
pub struct HtmlExtractor;

impl Default for HtmlExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl HtmlExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl HtmlExtractor {
    /// Map html-to-markdown's `DocumentStructure` into xberg's `InternalDocument`.
    ///
    /// Walks the flat node array from html-to-markdown and uses `InternalDocumentBuilder`
    /// to construct the equivalent xberg representation. Skips `RawBlock` nodes
    /// (script/style content) and `MetadataBlock` nodes (handled by metadata extraction).
    ///
    /// `budget` enforces hostile-input limits (iteration count, nesting depth, entity
    /// length, cumulative content size, table cells). Any limit violation is converted
    /// into a `XbergError::Security` via the `?` operator.
    fn map_document_structure(
        doc_structure: &html_to_markdown_rs::types::DocumentStructure,
        inject_placeholders: bool,
        budget: &mut SecurityBudget,
    ) -> crate::Result<InternalDocument> {
        let mut b = InternalDocumentBuilder::new("html");

        let root_indices: Vec<usize> = doc_structure
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| n.parent.is_none())
            .map(|(i, _)| i)
            .collect();

        Self::walk_nodes(doc_structure, &root_indices, &mut b, inject_placeholders, budget)?;

        Ok(b.build())
    }

    /// Recursively walk document nodes and push them into the builder.
    ///
    /// Calls `budget` validators on each node visited to enforce hostile-input limits.
    fn walk_nodes(
        doc: &html_to_markdown_rs::types::DocumentStructure,
        indices: &[usize],
        b: &mut InternalDocumentBuilder,
        inject_placeholders: bool,
        budget: &mut SecurityBudget,
    ) -> crate::Result<()> {
        use html_to_markdown_rs::types::NodeContent as HC;

        for &idx in indices {
            budget.step()?;

            let Some(node) = doc.nodes.get(idx) else {
                continue;
            };

            match &node.content {
                HC::Heading { level, text } => {
                    budget.account_text(text.len())?;
                    let elem_idx = b.push_heading(*level, text, None, None);
                    let annotations = map_annotations(&node.annotations);
                    push_link_uris_from_annotations(&annotations, text, b);
                    if !annotations.is_empty() {
                        b.set_annotations(elem_idx, annotations);
                    }
                }
                HC::Paragraph { text } => {
                    budget.account_text(text.len())?;
                    let annotations = map_annotations(&node.annotations);
                    push_link_uris_from_annotations(&annotations, text, b);
                    b.push_paragraph(text, annotations, None, None);
                }
                HC::List { ordered } => {
                    budget.enter()?;
                    b.push_list(*ordered);
                    let child_indices: Vec<usize> = node.children.iter().map(|&i| i as usize).collect();
                    Self::walk_nodes(doc, &child_indices, b, inject_placeholders, budget)?;
                    b.end_list();
                    budget.leave();
                }
                HC::ListItem { text } => {
                    budget.enter()?;
                    let ordered = node
                        .parent
                        .and_then(|p| doc.nodes.get(p as usize))
                        .map(|parent| matches!(parent.content, HC::List { ordered: true }))
                        .unwrap_or(false);
                    let annotations = map_annotations(&node.annotations);
                    push_link_uris_from_annotations(&annotations, text, b);
                    budget.account_text(text.len())?;
                    b.push_list_item(text, ordered, annotations, None, None);

                    if !node.children.is_empty() {
                        let child_indices: Vec<usize> = node.children.iter().map(|&i| i as usize).collect();
                        Self::walk_nodes(doc, &child_indices, b, inject_placeholders, budget)?;
                    }
                    budget.leave();
                }
                HC::Table { grid } => {
                    let cell_count = grid.cells.len();
                    budget.add_cells(cell_count)?;
                    let cells = crate::extraction::grid_flatten::flatten_positioned_cells(
                        grid.rows as usize,
                        grid.cells
                            .iter()
                            .map(|c| (c.row, c.row_span, c.col_span, c.content.clone())),
                    );
                    b.push_table_from_cells(&cells, None, None);
                }
                HC::Image { description, src, .. } => {
                    let text = description.as_deref().unwrap_or("");
                    if inject_placeholders && (!text.is_empty() || src.is_some()) {
                        let display = if let Some(src) = src {
                            if text.is_empty() {
                                format!("![]({})", src)
                            } else {
                                format!("![{}]({})", text, src)
                            }
                        } else {
                            text.to_string()
                        };
                        budget.account_text(display.len())?;
                        b.push_paragraph(&display, vec![], None, None);
                    }
                    if let Some(img_src) = src.as_ref().filter(|s| !s.is_empty()) {
                        b.push_uri(ExtractedUri::image(img_src.as_str(), description.clone()));
                    }
                }
                HC::Code { text, language } => {
                    budget.account_text(text.len())?;
                    b.push_code(text, language.as_deref(), None, None);
                }
                HC::Quote => {
                    budget.enter()?;
                    b.push_quote_start();
                    let child_indices: Vec<usize> = node.children.iter().map(|&i| i as usize).collect();
                    Self::walk_nodes(doc, &child_indices, b, inject_placeholders, budget)?;
                    b.push_quote_end();
                    budget.leave();
                }
                HC::DefinitionList => {
                    budget.enter()?;
                    let child_indices: Vec<usize> = node.children.iter().map(|&i| i as usize).collect();
                    Self::walk_nodes(doc, &child_indices, b, inject_placeholders, budget)?;
                    budget.leave();
                }
                HC::DefinitionItem { term, definition } => {
                    budget.account_text(term.len())?;
                    budget.account_text(definition.len())?;
                    b.push_definition_term(term, None);
                    b.push_definition_description(definition, None);
                }
                HC::Group { label, .. } => {
                    budget.enter()?;
                    b.push_group_start(label.as_deref(), None);
                    let child_indices: Vec<usize> = node.children.iter().map(|&i| i as usize).collect();
                    Self::walk_nodes(doc, &child_indices, b, inject_placeholders, budget)?;
                    b.push_group_end();
                    budget.leave();
                }
                HC::RawBlock { .. } | HC::MetadataBlock { .. } => {}
            }
        }

        Ok(())
    }
}

/// Map html-to-markdown annotations to xberg annotations.
fn map_annotations(annotations: &[html_to_markdown_rs::types::TextAnnotation]) -> Vec<TextAnnotation> {
    annotations
        .iter()
        .map(|a| {
            use html_to_markdown_rs::types::AnnotationKind as AK;
            let kind = match &a.kind {
                AK::Bold => crate::types::document_structure::AnnotationKind::Bold,
                AK::Italic => crate::types::document_structure::AnnotationKind::Italic,
                AK::Underline => crate::types::document_structure::AnnotationKind::Underline,
                AK::Strikethrough => crate::types::document_structure::AnnotationKind::Strikethrough,
                AK::Code => crate::types::document_structure::AnnotationKind::Code,
                AK::Subscript => crate::types::document_structure::AnnotationKind::Subscript,
                AK::Superscript => crate::types::document_structure::AnnotationKind::Superscript,
                AK::Highlight => crate::types::document_structure::AnnotationKind::Highlight,
                AK::Link { url, title } => crate::types::document_structure::AnnotationKind::Link {
                    url: url.clone(),
                    title: title.clone(),
                },
            };
            TextAnnotation {
                start: a.start,
                end: a.end,
                kind,
            }
        })
        .collect()
}

/// Extract URIs from link annotations and push them into the builder.
fn push_link_uris_from_annotations(annotations: &[TextAnnotation], text: &str, b: &mut InternalDocumentBuilder) {
    for ann in annotations {
        if let crate::types::document_structure::AnnotationKind::Link { url, .. } = &ann.kind {
            if url.is_empty() {
                continue;
            }
            let start = ann.start as usize;
            let end = ann.end as usize;
            let label = if ann.start < ann.end && end <= text.len() {
                if text.is_char_boundary(start) && text.is_char_boundary(end) {
                    let slice = &text[start..end];
                    if slice.is_empty() {
                        None
                    } else {
                        Some(slice.to_string())
                    }
                } else {
                    // A non-ASCII document can have an annotation span whose byte
                    // offsets land mid-codepoint; slicing on that would panic.
                    // Degrade gracefully instead: drop the label but keep the URI.
                    b.add_warning(crate::core::diagnostics::warning(
                        HTML_WARNING_SOURCE,
                        format!(
                            "A link annotation ({start}..{end}) did not align with a character \
                             boundary in the source text; its label text was dropped, though the \
                             link URL was preserved"
                        ),
                    ));
                    None
                }
            } else {
                None
            };
            b.push_uri(ExtractedUri {
                url: url.clone(),
                label,
                page: None,
                kind: classify_uri(url),
            });
        }
    }
}

/// Normalize markdown output from html-to-markdown-rs to comply with GFM lint rules.
///
/// html-to-markdown-rs may produce:
/// - Setext-style headings (`text\n===` or `text\n---`) instead of ATX (`# text`)
/// - Lines with trailing whitespace
/// - ATX headings without a preceding blank line
///
/// This function normalizes all three issues. Only called when `pre_rendered_content`
/// is about to be stored as the Markdown output of an HTML extraction.
/// Matches a `<math>...</math>` subtree (case-insensitive, spanning newlines) anywhere in
/// raw HTML, mirroring the allowlist already used by `extraction::html::structure` for
/// EPUB/email content.
#[cfg(feature = "office")]
static MATH_TAG_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<math\b[^>]*>.*?</math>").unwrap());

/// Matches a `<script type="math/tex">` block, which MathJax v2 pages use to carry
/// the LaTeX source. `math/tex; mode=display` marks display math.
#[cfg(feature = "office")]
static MATH_SCRIPT_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<script[^>]*\btype\s*=\s*["']?math/tex[^"'>]*["']?[^>]*>(.*?)</script>"#).unwrap()
});

/// Matches the `alt` text of an image whose class marks it as a rendered
/// equation, the shape legacy MathJax and MediaWiki pages use.
#[cfg(feature = "office")]
static TEX_IMG_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(r#"(?is)<img\b[^>]*\bclass\s*=\s*["'][^"']*\b(?:tex|mwe-math-fallback-image-\w+|latex)\b[^"']*["'][^>]*>"#)
        .unwrap()
});

/// Matches an `alt` attribute's value.
#[cfg(feature = "office")]
static ALT_ATTR_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r#"(?is)\balt\s*=\s*["']([^"']*)["']"#).unwrap());

/// Matches the `<!-- MathML: ... -->` comment that `html-to-markdown-rs` emits inline for
/// every `<math>` element it converts (see `handle_math` in that crate). The comment
/// serializes the raw MathML XML and leaks straight into `pre_rendered_content`/plain text
/// output; it carries no value once the equation has been recovered as LaTeX below, so it
/// is stripped.
static MATHML_COMMENT_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<!--\s*MathML:.*?-->\s*").unwrap());

/// Recover MathML equations as LaTeX `Formula` elements (issue #129).
///
/// `html-to-markdown-rs`'s `DocumentStructure` has no dedicated node kind for `<math>`
/// content: a `<math>` outside a paragraph is dropped entirely by its structure walker, and
/// one nested inside a paragraph is flattened into concatenated token text (`mn`/`mo`/`mi`
/// text with no operators). The library's markdown/plain-text conversion pass additionally
/// leaks a raw `<!-- MathML: ... -->` comment into the rendered output. Neither problem can
/// be fixed inside the extractor's mapped `DocumentStructure`, since the information is
/// already lost by the time it gets there — so this re-scans the original HTML directly,
/// using the same MathML-to-LaTeX converter the EPUB/email HTML structure builder already
/// calls (`crate::extraction::mathml::convert_mathml_str_to_latex`), and appends one
/// `ElementKind::Formula` element per recovered equation.
fn recover_mathml_formulas(html: &str, doc: &mut InternalDocument) {
    doc.pre_rendered_content = doc
        .pre_rendered_content
        .take()
        .map(|content| MATHML_COMMENT_RE.replace_all(&content, "").into_owned());

    #[cfg(feature = "office")]
    {
        let mut budget = crate::extractors::security::SecurityBudget::from_limits(
            &crate::extractors::security::SecurityLimits::default(),
        );
        let push_formula = |latex: String, doc: &mut InternalDocument| {
            let trimmed = latex.trim();
            if trimmed.is_empty() {
                return;
            }
            doc.push_element(crate::types::internal::InternalElement::text(
                crate::types::internal::ElementKind::Formula,
                trimmed.to_string(),
                0,
            ));
        };

        // A `math/tex` script and a `mwe-math-fallback-image` are what a page
        // carries *instead of* MathML: MediaWiki emits the image beside the
        // `math` element it falls back from, and MathJax v2 emits the script for
        // pages that ship no MathML at all. Reading them on a page that already
        // has MathML would report every equation twice. A page repeats a short
        // formula legitimately, so the test is which carriers the page uses, not
        // whether two formulas share their text.
        let has_mathml = MATH_TAG_RE.is_match(html);

        for m in MATH_TAG_RE.find_iter(html) {
            if let Ok(latex) = crate::extraction::mathml::convert_mathml_str_to_latex(m.as_str(), &mut budget) {
                push_formula(latex, doc);
            }
        }

        // A `math/tex` script holds the LaTeX source itself, so it needs no
        // conversion; only its delimiters and entities come off.
        for caps in MATH_SCRIPT_RE.captures_iter(html).filter(|_| !has_mathml) {
            let raw = quick_xml::escape::unescape(caps.get(1).map_or("", |m| m.as_str()))
                .unwrap_or_else(|_| caps.get(1).map_or("", |m| m.as_str()).into());
            let latex = crate::extraction::mathml::strip_style_wrapper(
                crate::extraction::derive::strip_math_delimiters(raw.trim()),
            );
            push_formula(latex.to_string(), doc);
        }

        // A rendered-equation image carries its source in `alt`, which is the
        // only copy of the math on pages that ship no MathML.
        for m in TEX_IMG_RE.find_iter(html).filter(|_| !has_mathml) {
            let Some(alt) = ALT_ATTR_RE.captures(m.as_str()).and_then(|c| c.get(1)) else {
                continue;
            };
            let raw = quick_xml::escape::unescape(alt.as_str()).unwrap_or_else(|_| alt.as_str().into());
            let latex = crate::extraction::mathml::strip_style_wrapper(
                crate::extraction::derive::strip_math_delimiters(raw.trim()),
            );
            push_formula(latex.to_string(), doc);
        }
    }

    // MathML-to-LaTeX conversion needs `roxmltree`, gated behind the `office` feature (see
    // `crate::extraction::mathml`). Without it, equations are dropped rather than mangled.
    #[cfg(not(feature = "office"))]
    let _ = html;
}

/// Matches a `<caption>...</caption>` element's inner content (case-insensitive, spanning
/// newlines), wherever it appears in the raw HTML.
static TABLE_CAPTION_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<caption\b[^>]*>(.*?)</caption>").unwrap());

/// Matches any HTML tag, used to strip inline markup out of a captured `<caption>` body.
static ANY_TAG_RE: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| regex::Regex::new(r"(?is)<[^>]+>").unwrap());

/// Recover `<table><caption>` text as a paragraph immediately preceding its table (issue
/// #146).
///
/// `html-to-markdown-rs`'s `TableGrid` (the structure the extractor's `HC::Table { grid }`
/// arm consumes) has no caption field at all, so a table's caption text never reaches
/// `map_document_structure` through the normal walk — it is dropped before the extractor
/// ever sees it. This re-scans the original HTML for `<caption>` elements (in document
/// order) and inserts one immediately before its corresponding `Table` element, so the
/// caption text at least reaches the element stream rather than vanishing outright.
///
/// This is a best-effort, order-based correlation (Nth caption -> Nth table): it has no way
/// to know a `<table>` had no `<caption>` at all when a *later* table does, since that
/// association is exactly the information the upstream library discards. For the common
/// case (each table has at most one caption, in document order) this is correct.
fn recover_table_captions(html: &str, doc: &mut InternalDocument) {
    let captions: Vec<String> = TABLE_CAPTION_RE
        .captures_iter(html)
        .filter_map(|c| c.get(1).map(|m| m.as_str()))
        .map(|inner| ANY_TAG_RE.replace_all(inner, "").trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if captions.is_empty() {
        return;
    }

    let table_positions: Vec<usize> = doc
        .elements
        .iter()
        .enumerate()
        .filter(|(_, e)| matches!(e.kind, crate::types::internal::ElementKind::Table { .. }))
        .map(|(i, _)| i)
        .collect();

    for (pos, caption) in table_positions
        .iter()
        .zip(captions.iter())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
    {
        doc.elements.insert(
            *pos,
            crate::types::internal::InternalElement::text(crate::types::internal::ElementKind::Paragraph, caption, 0),
        );
    }
}

pub(crate) fn normalize_html_markdown(raw: String) -> String {
    let lines: Vec<&str> = raw.lines().collect();
    let mut pass1: Vec<String> = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let line_trimmed = line.trim_end();

        if i + 1 < lines.len() {
            let next = lines[i + 1].trim();
            let is_setext_h1 = !next.is_empty() && next.chars().all(|c| c == '=');
            let is_setext_h2 = !next.is_empty()
                && next.chars().all(|c| c == '-')
                && !line_trimmed.trim().is_empty()
                && !line_trimmed.trim().starts_with('|');

            if is_setext_h1 {
                let heading_text = line_trimmed.trim();
                pass1.push(format!("# {heading_text}"));
                i += 2;
                continue;
            }
            if is_setext_h2 {
                let heading_text = line_trimmed.trim();
                pass1.push(format!("## {heading_text}"));
                i += 2;
                continue;
            }
        }

        pass1.push(line_trimmed.to_string());
        i += 1;
    }

    let mut result = String::with_capacity(raw.len());
    for (idx, line) in pass1.iter().enumerate() {
        let is_atx_heading = line.starts_with('#');
        if is_atx_heading && idx > 0 {
            let prev = &pass1[idx - 1];
            if !prev.is_empty() {
                result.push('\n');
            }
        }
        result.push_str(line);
        result.push('\n');
    }

    let trimmed_len = result.trim_end().len();
    if trimmed_len == 0 {
        return String::new();
    }
    result.truncate(trimmed_len);
    result.push('\n');
    result
}

/// Merge content filter settings into HTML conversion options.
///
/// When `content_filter` is `Some(...)`, adds `"header"` and/or `"footer"` to
/// `strip_tags` so `html-to-markdown-rs` removes those elements during conversion.
/// When `content_filter` is `None`, returns the options unchanged (preserving
/// current default behavior).
pub(crate) fn apply_content_filter_to_html_options(
    options: Option<html_to_markdown_rs::ConversionOptions>,
    content_filter: Option<&crate::core::config::ContentFilterConfig>,
) -> Option<html_to_markdown_rs::ConversionOptions> {
    let Some(filter) = content_filter else {
        return options;
    };

    let mut tags_to_strip: Vec<String> = Vec::new();
    if !filter.include_headers {
        tags_to_strip.push("header".to_string());
    }
    if !filter.include_footers {
        tags_to_strip.push("footer".to_string());
    }

    if tags_to_strip.is_empty() {
        return options;
    }

    let mut opts = options.unwrap_or_default();
    for tag in tags_to_strip {
        if !opts.strip_tags.contains(&tag) {
            opts.strip_tags.push(tag);
        }
    }
    Some(opts)
}

impl Plugin for HtmlExtractor {
    fn name(&self) -> &str {
        "html-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

impl SyncExtractor for HtmlExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let _span = tracing::debug_span!("extract_html", element_count = tracing::field::Empty,).entered();

        // A non-UTF-8 page still extracts, but every undecodable byte has already become
        // U+FFFD by the time anything downstream sees it. Remember that here so the caller
        // is told the text was mangled instead of being handed silent mojibake (#171).
        let (html, decoded_lossily) = match utf8_validation::from_utf8(content) {
            Ok(valid) => (valid.to_string(), false),
            Err(_) => (String::from_utf8_lossy(content).into_owned(), true),
        };

        let html_options =
            apply_content_filter_to_html_options(config.html_options.clone(), config.content_filter.as_ref());

        // `_table_data` is intentionally unused: it comes from the same conversion pass as
        // `doc_structure`, whose `map_document_structure` already creates the table elements and
        // their `doc.tables` entries. Converting it a second time only added duplicate,
        // unreferenced entries to `doc.tables` (and double-counted cells against the security
        // budget) without adding anything to rendered output.
        let (content_text, html_metadata, _table_data, doc_structure) =
            crate::extraction::html::convert_html_to_markdown_with_tables(
                &html,
                html_options,
                Some(config.output_format.clone()),
            )?;

        let mut budget = SecurityBudget::from_config(config);

        let meta_title = html_metadata.as_ref().and_then(|m| m.title.clone());
        let meta_authors = html_metadata
            .as_ref()
            .and_then(|m| m.author.as_ref().map(|a| vec![a.clone()]));
        let meta_language = html_metadata.as_ref().and_then(|m| m.language.clone());
        let meta_subject = html_metadata.as_ref().and_then(|m| m.description.clone());
        let meta_keywords = html_metadata.as_ref().and_then(|m| {
            if m.keywords.is_empty() {
                None
            } else {
                Some(m.keywords.clone())
            }
        });

        let format_metadata = html_metadata.map(|m: HtmlMetadata| crate::types::FormatMetadata::Html(Box::new(m)));

        let (pre_formatted, pre_rendered) = match config.output_format {
            OutputFormat::Markdown => {
                let normalized = normalize_html_markdown(content_text.clone());
                (Some("markdown".to_string()), Some(normalized))
            }
            OutputFormat::Djot => (Some("djot".to_string()), Some(content_text.clone())),
            _ => (None, None),
        };

        let inject_placeholders = config
            .images
            .as_ref()
            .map(|img| img.inject_placeholders)
            .unwrap_or(true);

        let mut doc = if let Some(ref structure) = doc_structure {
            let mapped = Self::map_document_structure(structure, inject_placeholders, &mut budget)?;
            if mapped.elements.is_empty() && !content_text.is_empty() {
                let mut b = InternalDocumentBuilder::new("html");
                b.push_paragraph(&content_text, vec![], None, None);
                b.build()
            } else {
                mapped
            }
        } else if !content_text.is_empty() {
            let mut b = InternalDocumentBuilder::new("html");
            b.push_paragraph(&content_text, vec![], None, None);
            b.build()
        } else {
            InternalDocumentBuilder::new("html").build()
        };

        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut doc.processing_warnings,
                HTML_WARNING_SOURCE,
                "HTML source",
            );
        }

        doc.metadata = Metadata {
            title: meta_title,
            authors: meta_authors,
            language: meta_language,
            subject: meta_subject,
            keywords: meta_keywords,
            output_format: pre_formatted,
            format: format_metadata,
            ..Default::default()
        };
        doc.mime_type = mime_type.to_string();
        doc.pre_rendered_content = pre_rendered;
        recover_mathml_formulas(&html, &mut doc);
        recover_table_captions(&html, &mut doc);

        let should_extract_images = config.needs_image_data();

        // Images extracted here (with binary data and OCR eligibility) each need a matching
        // `ElementKind::Image` element: every renderer resolves images by walking elements, not
        // by reading `doc.images` directly, so a raw `push_image` alone silently drops them.
        // The HTML conversion pass above emits only inline `![alt](src)` markdown and a URI
        // record — it never stores bytes — and correlating those back to the extracted images
        // would require threading image identity through that pass, so the elements are appended
        // here instead of being placed in-flow. ~keep
        if should_extract_images {
            let image_html_options =
                apply_content_filter_to_html_options(config.html_options.clone(), config.content_filter.as_ref());
            let inline_images = crate::extraction::html::extract_html_inline_images(&html, image_html_options)?;

            for (i, img) in inline_images.into_iter().enumerate() {
                let (width, height) = img.dimensions.map_or((None, None), |d| (Some(d.width), Some(d.height)));
                let format: Cow<'static, str> = match img.format {
                    InlineImageFormat::Png => Cow::Borrowed("png"),
                    InlineImageFormat::Jpeg => Cow::Borrowed("jpeg"),
                    InlineImageFormat::Gif => Cow::Borrowed("gif"),
                    InlineImageFormat::Bmp => Cow::Borrowed("bmp"),
                    InlineImageFormat::Webp => Cow::Borrowed("webp"),
                    InlineImageFormat::Svg => Cow::Borrowed("svg"),
                    InlineImageFormat::Other(ref s) => Cow::Owned(s.clone()),
                };

                let (image_kind, kind_confidence) = crate::extraction::image_kind::classify(
                    &img.data,
                    format.as_ref(),
                    width,
                    height,
                    None,
                    None,
                    false,
                );

                let extracted = ExtractedImage {
                    data: Bytes::from(img.data),
                    format,
                    image_index: i as u32,
                    page_number: None,
                    width,
                    height,
                    colorspace: None,
                    bits_per_component: None,
                    is_mask: false,
                    description: img.description,
                    ocr_result: None,
                    bounding_box: None,
                    source_path: None,
                    image_kind: Some(image_kind),
                    kind_confidence: Some(kind_confidence),
                    cluster_id: None,
                    caption: None,
                    qr_codes: None,
                    data_base64: None,
                };
                let description = extracted.description.clone();
                let image_index = doc.push_image(extracted);
                let text = description.unwrap_or_default();
                doc.push_element(crate::types::internal::InternalElement::text(
                    crate::types::internal::ElementKind::Image { image_index },
                    text,
                    0,
                ));
            }
        }

        _span.record("element_count", doc.elements.len());

        Ok(doc)
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for HtmlExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        self.extract_sync(content, mime_type, config)
    }

    #[cfg(feature = "tokio-runtime")]
    #[cfg_attr(feature = "otel", tracing::instrument(
        skip(self, path, config),
        fields(
            extractor.name = self.name(),
        )
    ))]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let bytes = crate::core::io::read_file_async(path).await?;
        self.extract_content(&bytes, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["text/html", "application/xhtml+xml"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extractors::security::SecurityLimits;

    /// Helper to extract tables from HTML using the unified converter.
    fn extract_tables(html: &str) -> Vec<Table> {
        let (_, _, table_data, _): (String, _, Vec<html_to_markdown_rs::types::TableData>, _) =
            crate::extraction::html::convert_html_to_markdown_with_tables(html, None, None).unwrap();
        table_data
            .into_iter()
            .enumerate()
            .map(|(i, t)| {
                let grid = &t.grid;
                let cells = crate::extraction::grid_flatten::flatten_positioned_cells(
                    grid.rows as usize,
                    grid.cells
                        .iter()
                        .map(|c| (c.row, c.row_span, c.col_span, c.content.clone())),
                );
                Table {
                    cells,
                    markdown: t.markdown,
                    page_number: (i + 1) as u32,
                    bounding_box: None,
                    ..Default::default()
                }
            })
            .collect()
    }

    #[test]
    fn test_html_extractor_plugin_interface() {
        let extractor = HtmlExtractor::new();
        assert_eq!(extractor.name(), "html-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_html_extractor_supported_mime_types() {
        let extractor = HtmlExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 2);
        assert!(mime_types.contains(&"text/html"));
        assert!(mime_types.contains(&"application/xhtml+xml"));
    }

    #[test]
    fn test_extract_html_tables_basic() {
        let html = r#"
            <table>
                <tr><th>Header1</th><th>Header2</th></tr>
                <tr><td>Row1Col1</td><td>Row1Col2</td></tr>
                <tr><td>Row2Col1</td><td>Row2Col2</td></tr>
            </table>
        "#;

        let tables = extract_tables(html);
        assert_eq!(tables.len(), 1);

        let table = &tables[0];
        assert_eq!(table.cells.len(), 3);
        assert_eq!(table.cells[0], vec!["Header1", "Header2"]);
        assert_eq!(table.cells[1], vec!["Row1Col1", "Row1Col2"]);
        assert_eq!(table.cells[2], vec!["Row2Col1", "Row2Col2"]);
        assert_eq!(table.page_number, 1);
        assert!(table.markdown.contains("Header1"));
        assert!(table.markdown.contains("Row1Col1"));
    }

    /// A non-ASCII document whose link annotation lands mid-codepoint must not panic
    /// `push_link_uris_from_annotations`. `text` is "café" (5 bytes: c, a, f, then the
    /// 2-byte UTF-8 encoding of 'é'); an annotation ending at byte 4 splits that 'é' and
    /// is not a valid `&str` slice boundary. The link's label must be dropped (not sliced)
    /// while the URI itself is still recorded, and a `ProcessingWarning` must name the drop.
    #[test]
    fn should_not_panic_and_should_warn_when_link_annotation_splits_a_codepoint() {
        use crate::types::document_structure::AnnotationKind;

        let text = "caf\u{00e9}";
        assert_eq!(
            text.len(),
            5,
            "'café' must be 5 UTF-8 bytes for this test to be meaningful"
        );
        assert!(!text.is_char_boundary(4), "byte 4 must split the 2-byte 'é' encoding");

        let annotations = vec![TextAnnotation {
            start: 0,
            end: 4,
            kind: AnnotationKind::Link {
                url: "https://example.com".to_string(),
                title: None,
            },
        }];

        let mut builder = InternalDocumentBuilder::new("html");
        push_link_uris_from_annotations(&annotations, text, &mut builder);
        let doc = builder.build();

        assert_eq!(doc.uris.len(), 1, "the URI must still be recorded");
        assert_eq!(doc.uris[0].url, "https://example.com");
        assert_eq!(
            doc.uris[0].label, None,
            "label must be dropped, not sliced mid-codepoint"
        );

        assert_eq!(doc.processing_warnings.len(), 1);
        assert_eq!(doc.processing_warnings[0].source, HTML_WARNING_SOURCE);
        assert!(
            doc.processing_warnings[0].message.contains("character boundary"),
            "warning message was: {}",
            doc.processing_warnings[0].message
        );
    }

    #[test]
    fn test_extract_html_tables_multiple() {
        let html = r#"
            <table>
                <tr><th>Table1</th></tr>
                <tr><td>Data1</td></tr>
            </table>
            <p>Some text</p>
            <table>
                <tr><th>Table2</th></tr>
                <tr><td>Data2</td></tr>
            </table>
        "#;

        let tables = extract_tables(html);
        assert_eq!(tables.len(), 2);
        assert_eq!(tables[0].page_number, 1);
        assert_eq!(tables[1].page_number, 2);
    }

    #[test]
    fn test_extract_html_tables_no_thead() {
        let html = r#"
            <table>
                <tr><td>Cell1</td><td>Cell2</td></tr>
                <tr><td>Cell3</td><td>Cell4</td></tr>
            </table>
        "#;

        let tables = extract_tables(html);
        assert_eq!(tables.len(), 1);

        let table = &tables[0];
        assert_eq!(table.cells.len(), 2);
        assert_eq!(table.cells[0], vec!["Cell1", "Cell2"]);
        assert_eq!(table.cells[1], vec!["Cell3", "Cell4"]);
    }

    #[test]
    fn test_extract_html_tables_empty() {
        let html = "<p>No tables here</p>";
        let tables = extract_tables(html);
        assert_eq!(tables.len(), 0);
    }

    #[test]
    fn test_extract_html_tables_with_nested_elements() {
        let html = r#"
            <table>
                <tr><th>Header <strong>Bold</strong></th></tr>
                <tr><td>Data with <em>emphasis</em></td></tr>
            </table>
        "#;

        let tables = extract_tables(html);
        assert_eq!(tables.len(), 1);

        let table = &tables[0];
        assert!(table.cells[0][0].contains("Header"));
        assert!(table.cells[0][0].contains("Bold"));
        assert!(table.cells[1][0].contains("Data with"));
        assert!(table.cells[1][0].contains("emphasis"));
    }

    #[test]
    fn test_extract_nested_html_tables() {
        let html = r#"
            <table>
                <tr>
                    <th>Category</th>
                    <th>Details &amp; Nested Data</th>
                </tr>
                <tr>
                    <td><strong>Project Alpha</strong></td>
                    <td>
                    <table>
                        <tr><th>Task ID</th><th>Status</th><th>Priority</th></tr>
                        <tr><td>001</td><td>Completed</td><td>High</td></tr>
                        <tr><td>002</td><td>In Progress</td><td>Medium</td></tr>
                    </table>
                    </td>
                </tr>
                <tr>
                    <td><strong>Project Beta</strong></td>
                    <td>No sub-tasks assigned yet.</td>
                </tr>
            </table>
        "#;

        let tables = extract_tables(html);

        assert!(
            tables.len() >= 2,
            "Expected at least 2 tables (outer + nested), found {}",
            tables.len()
        );

        let nested = tables
            .iter()
            .find(|t| {
                t.cells
                    .first()
                    .is_some_and(|row| row.iter().any(|c| c.contains("Task ID")))
            })
            .expect("Should find nested table with Task ID header");

        assert_eq!(nested.cells[0].len(), 3, "Nested table header should have 3 columns");
        assert!(nested.cells[0][0].contains("Task ID"));
        assert!(nested.cells[0][1].contains("Status"));
        assert!(nested.cells[0][2].contains("Priority"));
        assert_eq!(
            nested.cells.len(),
            3,
            "Nested table should have 3 rows (header + 2 data)"
        );
        assert!(nested.cells[1][0].contains("001"));
        assert!(nested.cells[1][1].contains("Completed"));
        assert!(nested.cells[2][0].contains("002"));
        assert!(nested.cells[2][1].contains("In Progress"));
    }

    #[tokio::test]
    async fn test_html_extractor_with_table() {
        let html = r#"
            <html>
                <body>
                    <h1>Test Page</h1>
                    <table>
                        <tr><th>Name</th><th>Age</th></tr>
                        <tr><td>Alice</td><td>30</td></tr>
                        <tr><td>Bob</td><td>25</td></tr>
                    </table>
                </body>
            </html>
        "#;

        let extractor = HtmlExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(html.as_bytes(), "text/html", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(!result.tables.is_empty(), "Should have at least one table");
        let table = &result.tables[0];
        assert_eq!(table.cells.len(), 3);
        assert_eq!(table.cells[0], vec!["Name", "Age"]);
        assert_eq!(table.cells[1], vec!["Alice", "30"]);
        assert_eq!(table.cells[2], vec!["Bob", "25"]);
    }

    /// Regression test: `extract_sync` used to additionally re-push every table via the raw,
    /// element-less `InternalDocument::push_table`, on top of the correctly-created table
    /// element from `map_document_structure`. That created a duplicate, unreferenced entry in
    /// `doc.tables` for every table (and double-counted cells against the security budget)
    /// without changing rendered output. Assert there is exactly one table, not two.
    #[tokio::test]
    async fn test_html_extractor_table_is_not_duplicated_in_structured_output() {
        let html = r#"
            <html>
                <body>
                    <table>
                        <tr><th>Name</th><th>Age</th></tr>
                        <tr><td>Alice</td><td>30</td></tr>
                    </table>
                </body>
            </html>
        "#;

        let extractor = HtmlExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(html.as_bytes(), "text/html", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(
            result.tables.len(),
            1,
            "table should not be duplicated: {:?}",
            result.tables
        );
    }

    /// Regression test: extracted inline image bytes must be reachable from a matching
    /// `ElementKind::Image` element, otherwise renderers silently drop the image (and any OCR
    /// content attached to it) since they look images up by walking elements, not `doc.images`
    /// directly.
    #[tokio::test]
    async fn test_html_extractor_inline_image_produces_image_element() {
        let png_b64 =
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";
        let html = format!(r#"<html><body><img src="data:image/png;base64,{png_b64}" alt="a photo"></body></html>"#);

        let config = ExtractionConfig {
            images: Some(crate::core::config::ImageExtractionConfig {
                extract_images: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let extractor = HtmlExtractor::new();
        let doc = extractor
            .extract_content(html.as_bytes(), "text/html", &config)
            .await
            .expect("extraction should succeed");

        assert_eq!(doc.images.len(), 1, "expected one extracted image in {:?}", doc.images);
        let image_element_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Image { .. }))
            .count();
        assert_eq!(
            image_element_count, 1,
            "expected one Image element in {:?}",
            doc.elements
        );
    }

    #[tokio::test]
    async fn test_html_extractor_with_djot_output() {
        let html = r#"
        <html>
            <body>
                <h1>Test Page</h1>
                <p>Content with <strong>emphasis</strong>.</p>
            </body>
        </html>
    "#;

        let extractor = HtmlExtractor::new();
        let config = ExtractionConfig {
            output_format: OutputFormat::Djot,
            ..Default::default()
        };

        let result = extractor
            .extract_content(html.as_bytes(), "text/html", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.mime_type, "text/html");
        assert!(
            result.content.contains("Test Page"),
            "Should contain heading text: {}",
            result.content
        );
        assert!(
            result.content.contains("emphasis"),
            "Should contain emphasis text: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_html_extractor_djot_double_conversion_prevention() {
        let html = r#"
        <html>
            <body>
                <h1>Test</h1>
                <p>Content with <strong>bold</strong> text.</p>
            </body>
        </html>
    "#;

        let extractor = HtmlExtractor::new();
        let config = ExtractionConfig {
            output_format: OutputFormat::Djot,
            ..Default::default()
        };

        let result = extractor
            .extract_content(html.as_bytes(), "text/html", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.mime_type, "text/html");
        let original_content = result.content.clone();

        let pipeline_result = crate::core::pipeline::apply_output_format(result.clone(), OutputFormat::Djot);

        assert_eq!(pipeline_result.content, original_content);
        assert_eq!(pipeline_result.mime_type, "text/html");
    }

    #[test]
    fn test_map_document_structure_basic() {
        let html = "<h1>Title</h1><p>Hello world.</p>";
        let (_, _, _, doc_structure) =
            crate::extraction::html::convert_html_to_markdown_with_tables(html, None, None).unwrap();
        let doc_structure = doc_structure.expect("should have document structure");
        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let doc = HtmlExtractor::map_document_structure(&doc_structure, true, &mut budget).unwrap();
        assert!(!doc.elements.is_empty(), "Should have elements");
    }

    #[tokio::test]
    async fn test_extract_sync_plain_text_has_content() {
        let html = r#"<h1>Title</h1><p>Hello world</p>"#;
        let extractor = HtmlExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(html.as_bytes(), "text/html", &config)
            .await
            .unwrap();
        assert!(
            !result.elements.is_empty(),
            "InternalDocument should have elements, got: {:?}",
            result.elements.len()
        );
        let content = result.content();
        assert!(
            content.contains("Title"),
            "Content should contain heading: '{}'",
            content
        );
    }

    #[test]
    fn test_no_css_or_script_leaking() {
        let html = r#"
        <html>
            <head>
                <style>body { color: red; } .hidden { display: none; }</style>
                <script>alert('xss');</script>
                <script type="application/ld+json">{"@type": "Article"}</script>
            </head>
            <body>
                <h1>Clean Content</h1>
                <p>This should be the only content.</p>
            </body>
        </html>
        "#;

        let doc_structure = {
            let (_, _, _, ds) =
                crate::extraction::html::convert_html_to_markdown_with_tables(html, None, None).unwrap();
            ds.expect("should have document structure")
        };
        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let doc = HtmlExtractor::map_document_structure(&doc_structure, true, &mut budget).unwrap();

        for elem in &doc.elements {
            let text = elem.text.as_str();
            assert!(
                !text.contains("color: red"),
                "CSS should not leak into elements: {:?}",
                text
            );
            assert!(
                !text.contains("alert("),
                "Script should not leak into elements: {:?}",
                text
            );
            assert!(
                !text.contains("@type"),
                "JSON-LD should not leak into elements: {:?}",
                text
            );
        }
    }

    #[test]
    fn test_html_inject_placeholders_true() {
        let html = r#"<html><body><img src="test.png" alt="test image"></body></html>"#;
        let (_, _, _, doc_structure) =
            crate::extraction::html::convert_html_to_markdown_with_tables(html, None, None).unwrap();
        let doc_structure = doc_structure.expect("should have document structure");
        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let doc = HtmlExtractor::map_document_structure(&doc_structure, true, &mut budget).unwrap();
        let content = doc.content();
        assert!(
            content.contains("!["),
            "inject_placeholders=true should produce image markdown placeholder, got: '{}'",
            content
        );
    }

    #[test]
    fn test_html_inject_placeholders_false() {
        let html = r#"<html><body><img src="test.png" alt="test image"></body></html>"#;
        let (_, _, _, doc_structure) =
            crate::extraction::html::convert_html_to_markdown_with_tables(html, None, None).unwrap();
        let doc_structure = doc_structure.expect("should have document structure");
        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let doc = HtmlExtractor::map_document_structure(&doc_structure, false, &mut budget).unwrap();
        let content = doc.content();
        assert!(
            !content.contains("!["),
            "inject_placeholders=false should not produce image markdown placeholder, got: '{}'",
            content
        );
        assert!(
            !doc.uris.is_empty(),
            "Image URI should still be collected when inject_placeholders=false"
        );
        assert!(
            doc.uris.iter().any(|u| u.url.contains("test.png")),
            "Should have URI for test.png"
        );
    }

    #[test]
    fn test_normalize_converts_setext_h1_to_atx() {
        let input = "Title\n=====\n\nSome paragraph.\n".to_string();
        let output = normalize_html_markdown(input);
        assert!(output.contains("# Title"), "should convert setext H1, got: {output}");
        assert!(!output.contains("====="), "should remove underline, got: {output}");
    }

    #[test]
    fn test_normalize_converts_setext_h2_to_atx() {
        let input = "Subtitle\n--------\n\nSome paragraph.\n".to_string();
        let output = normalize_html_markdown(input);
        assert!(
            output.contains("## Subtitle"),
            "should convert setext H2, got: {output}"
        );
        assert!(!output.contains("--------"), "should remove underline, got: {output}");
    }

    #[test]
    fn test_normalize_strips_trailing_whitespace() {
        let input = "Line one   \nLine two\t\n".to_string();
        let output = normalize_html_markdown(input);
        for line in output.lines() {
            assert!(
                !line.ends_with(' ') && !line.ends_with('\t'),
                "line has trailing whitespace: {line:?}"
            );
        }
    }

    #[test]
    fn test_normalize_ensures_blank_line_before_atx_heading() {
        let input = "Some text\n# Heading\n".to_string();
        let output = normalize_html_markdown(input);
        let lines: Vec<&str> = output.lines().collect();
        let heading_idx = lines.iter().position(|l| l.starts_with("# Heading")).unwrap();
        assert!(heading_idx > 0, "heading should not be at line 0");
        assert!(
            lines[heading_idx - 1].is_empty(),
            "blank line before ATX heading required, found: {:?}",
            lines[heading_idx - 1]
        );
    }

    #[test]
    fn test_normalize_atx_heading_at_file_start_no_blank_line_needed() {
        let input = "# Top Heading\n\nSome text.\n".to_string();
        let output = normalize_html_markdown(input.clone());
        assert!(
            output.starts_with("# Top Heading"),
            "heading at file start should not have leading blank line, got: {output:?}"
        );
    }

    #[test]
    fn test_normalize_single_trailing_newline() {
        let input = "Content\n\n\n".to_string();
        let output = normalize_html_markdown(input);
        assert!(output.ends_with('\n'), "should end with newline");
        assert!(
            !output.ends_with("\n\n"),
            "should have exactly one trailing newline, got: {output:?}"
        );
    }

    #[test]
    fn test_normalize_does_not_convert_table_separator_as_setext_h2() {
        let input = "| Col1 | Col2 |\n|------|------|\n| A    | B    |\n".to_string();
        let output = normalize_html_markdown(input);
        assert!(
            output.contains("|------|"),
            "table separator should not be treated as setext H2, got: {output}"
        );
    }

    #[test]
    fn test_normalize_empty_input() {
        let input = String::new();
        let output = normalize_html_markdown(input);
        assert!(output.is_empty(), "empty input should produce empty output");
    }

    /// Test that extract_metadata=false is respected (issue #1171).
    /// When the user explicitly sets extract_metadata to false, no metadata
    /// should be returned, even if the HTML contains meta tags.
    #[test]
    fn test_extract_metadata_false_is_respected() {
        let html = r#"<!DOCTYPE html>
<html>
  <head>
    <title>This Is A Title</title>
    <meta name="description" content="This is the description">
    <meta name="author" content="John Doe">
  </head>
  <body>
    <h1>Content Heading</h1>
    <p>Some content here.</p>
  </body>
</html>"#;

        let options = html_to_markdown_rs::ConversionOptions {
            extract_metadata: false,
            ..Default::default()
        };

        let (content, metadata, _, _) =
            crate::extraction::html::convert_html_to_markdown_with_tables(html, Some(options), None).unwrap();

        assert!(content.contains("Content Heading"));

        assert!(
            metadata.is_none() || metadata.as_ref().unwrap().is_empty(),
            "Metadata should be None or empty when extract_metadata=false, but got: {:?}",
            metadata
        );
    }

    /// Test that extract_metadata=true returns metadata (default behavior).
    #[test]
    fn test_extract_metadata_true_returns_metadata() {
        let html = r#"<!DOCTYPE html>
<html>
  <head>
    <title>This Is A Title</title>
    <meta name="description" content="This is the description">
    <meta name="author" content="Jane Doe">
  </head>
  <body>
    <h1>Content Heading</h1>
    <p>Some content here.</p>
  </body>
</html>"#;

        let options = html_to_markdown_rs::ConversionOptions {
            extract_metadata: true,
            ..Default::default()
        };

        let (content, metadata, _, _) =
            crate::extraction::html::convert_html_to_markdown_with_tables(html, Some(options), None).unwrap();

        assert!(content.contains("Content Heading"));

        assert!(metadata.is_some(), "Metadata should be Some when extract_metadata=true");
        let meta = metadata.unwrap();
        assert_eq!(
            meta.title,
            Some("This Is A Title".to_string()),
            "Title should be extracted"
        );
        assert_eq!(
            meta.description,
            Some("This is the description".to_string()),
            "Description should be extracted"
        );
        assert_eq!(meta.author, Some("Jane Doe".to_string()), "Author should be extracted");
    }
    /// MathJax v2 pages carry the LaTeX source in a `math/tex` script, which is
    /// the only copy of the math on a page that ships no MathML.
    #[tokio::test]
    async fn test_math_tex_script_becomes_a_formula() {
        use crate::core::config::ExtractionConfig;
        let html = r#"<html><body><p>Einstein wrote
            <script type="math/tex; mode=display">E = mc^2</script></p></body></html>"#;
        let doc = HtmlExtractor::new()
            .extract_content(html.as_bytes(), "text/html", &ExtractionConfig::default())
            .await
            .expect("extraction failed");

        let latex: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Formula))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(latex, vec!["E = mc^2"]);
    }

    /// A rendered-equation image carries its source in `alt`, and the entities
    /// in it decode.
    #[tokio::test]
    async fn test_tex_image_alt_becomes_a_formula() {
        use crate::core::config::ExtractionConfig;
        let html = r#"<html><body><img class="mwe-math-fallback-image-inline"
            alt="a &lt; b" src="eq.png"/></body></html>"#;
        let doc = HtmlExtractor::new()
            .extract_content(html.as_bytes(), "text/html", &ExtractionConfig::default())
            .await
            .expect("extraction failed");

        let latex: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Formula))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(latex, vec!["a < b"]);
    }

    /// An ordinary image is not an equation.
    #[tokio::test]
    async fn test_plain_image_alt_is_not_a_formula() {
        use crate::core::config::ExtractionConfig;
        let html = r#"<html><body><img class="photo" alt="a cat" src="cat.png"/></body></html>"#;
        let doc = HtmlExtractor::new()
            .extract_content(html.as_bytes(), "text/html", &ExtractionConfig::default())
            .await
            .expect("extraction failed");

        assert!(
            !doc.elements
                .iter()
                .any(|e| matches!(e.kind, crate::types::internal::ElementKind::Formula))
        );
    }

    /// MediaWiki ships one equation twice: the `math` element and a fallback
    /// image whose `alt` holds the same TeX. The second is a representation of
    /// the first, not another formula.
    #[tokio::test]
    async fn test_mathml_and_its_fallback_image_are_one_formula() {
        use crate::core::config::ExtractionConfig;
        let html = r#"<html><body><span class="mwe-math-element">
            <math xmlns="http://www.w3.org/1998/Math/MathML"><semantics><mrow><mi>E</mi></mrow>
            <annotation encoding="application/x-tex">E=mc^{2}</annotation></semantics></math>
            <img class="mwe-math-fallback-image-inline" alt="{\displaystyle E=mc^{2}}" src="eq.png"/>
            </span></body></html>"#;
        let doc = HtmlExtractor::new()
            .extract_content(html.as_bytes(), "text/html", &ExtractionConfig::default())
            .await
            .expect("extraction failed");

        let latex: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Formula))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(latex, vec!["E=mc^{2}"], "the fallback image is the same equation");
    }

    /// A page repeats a short formula legitimately, and each occurrence is its
    /// own equation.
    #[tokio::test]
    async fn test_repeated_formulas_are_kept() {
        use crate::core::config::ExtractionConfig;
        let html = r#"<html><body>
            <p>Let <math xmlns="http://www.w3.org/1998/Math/MathML"><mi>n</mi></math> be even.</p>
            <p>Then <math xmlns="http://www.w3.org/1998/Math/MathML"><mi>n</mi></math> divides it.</p>
            </body></html>"#;
        let doc = HtmlExtractor::new()
            .extract_content(html.as_bytes(), "text/html", &ExtractionConfig::default())
            .await
            .expect("extraction failed");

        let latex: Vec<&str> = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Formula))
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(latex, vec!["n", "n"], "both occurrences are formulas");
    }

}
