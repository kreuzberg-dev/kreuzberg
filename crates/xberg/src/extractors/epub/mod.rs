//! Native EPUB extractor using permissive-licensed dependencies.
//!
//! This extractor provides native Rust-based EPUB extraction without GPL-licensed
//! dependencies, extracting:
//! - Metadata from OPF (Open Packaging Format) using Dublin Core standards
//! - Content from XHTML files in spine order
//! - Proper handling of EPUB2 and EPUB3 formats
//!
//! Uses only permissive-licensed crates:
//! - `zip` (MIT/Apache) - for reading EPUB container
//! - `roxmltree` (MIT) - for parsing XML
//! - `html-to-markdown-rs` (MIT) - for converting XHTML to Markdown/Djot when requested

mod content;
mod metadata;
mod parsing;

use crate::Result;
use crate::core::config::{ExtractionConfig, OutputFormat};
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::Metadata;
use crate::types::ProcessingWarning;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::{EpubMetadata, FormatMetadata};
use crate::types::uri::{ExtractedUri, UriKind, classify_uri};
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
use std::io::{Cursor, Read};
use zip::ZipArchive;

use crate::extractors::security::{SecurityBudget, ZipBombValidator};
use content::{extract_text_from_xhtml, extract_text_from_xhtml_budgeted};
use metadata::{build_additional_metadata, parse_opf};
use parsing::{MAX_EPUB_MEMBER_SIZE, parse_container_xml, read_file_from_zip, resolve_path};

const MARKUP_SWITCH_NAMESPACES: &[&str] = &[content::XHTML_NAMESPACE, content::MATHML_NAMESPACE];
const PLAIN_SWITCH_NAMESPACES: &[&str] = &[content::XHTML_NAMESPACE];
#[cfg_attr(alef, alef(skip))]
/// EPUB format extractor using permissive-licensed dependencies.
///
/// Extracts content and metadata from EPUB files (both EPUB2 and EPUB3)
/// using native Rust parsing without GPL-licensed dependencies.
pub struct EpubExtractor;

impl EpubExtractor {
    fn spine_references_asset(spine_documents: &[content::EpubSpineDocument], asset_path: &str) -> bool {
        spine_documents.iter().any(|document| {
            let Ok(parsed) = roxmltree::Document::parse(&document.xhtml) else {
                return false;
            };
            let document_dir = document
                .file_path
                .rsplit_once('/')
                .map(|(directory, _)| directory)
                .unwrap_or("");

            parsed.descendants().filter(|node| node.is_element()).any(|node| {
                node.tag_name().name().eq_ignore_ascii_case("img")
                    && node.attribute("src").is_some_and(|source| {
                        resolve_path(document_dir, source).is_ok_and(|resolved| resolved.path == asset_path)
                    })
            })
        })
    }

    /// Create a new EPUB extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for EpubExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "office")]
#[allow(dead_code)]
struct RenderedSpineDocument {
    content_fragment: String,
    content_fully_converted: bool,
    document: Option<crate::types::document_structure::DocumentStructure>,
    warnings: Vec<ProcessingWarning>,
}

#[cfg(feature = "office")]
#[allow(dead_code)]
fn trim_trailing_newlines(s: &str) -> &str {
    s.trim_end_matches(['\n', '\r'])
}

#[cfg(feature = "office")]
#[allow(dead_code)]
impl EpubExtractor {
    fn build_fallback_document_structure(
        xhtml: &str,
        index: usize,
    ) -> crate::types::document_structure::DocumentStructure {
        use crate::types::builder::DocumentStructureBuilder;

        let mut builder = DocumentStructureBuilder::new().source_format("epub");
        let chapter_title = extract_title_from_xhtml(xhtml).unwrap_or_else(|| format!("Chapter {}", index + 1));
        builder.push_heading(1, &chapter_title, None, None);

        let text = extract_text_from_xhtml(xhtml);
        for paragraph in text.split("\n\n") {
            let trimmed = paragraph.trim();
            if !trimmed.is_empty() {
                builder.push_paragraph(trimmed, vec![], None, None);
            }
        }

        builder.build()
    }

    /// Render a spine document once.
    fn render_spine_document(
        document: &content::EpubSpineDocument,
        xhtml: &str,
        index: usize,
        config: &ExtractionConfig,
    ) -> RenderedSpineDocument {
        let wants_markup = matches!(
            config.output_format,
            OutputFormat::Markdown | OutputFormat::Djot | OutputFormat::DocTags
        );
        let mut warnings = Vec::new();

        let (content_fragment, content_fully_converted) = if wants_markup {
            let html_options = super::html::apply_content_filter_to_html_options(
                config.html_options.clone(),
                config.content_filter.as_ref(),
            );
            match crate::extraction::html::convert_html_to_markdown_with_metadata(
                xhtml,
                html_options,
                Some(config.output_format.clone()),
            ) {
                Ok((converted, _)) => (trim_trailing_newlines(&converted).to_string(), true),
                Err(err) => {
                    warnings.push(ProcessingWarning {
                        source: std::borrow::Cow::Borrowed("epub"),
                        message: std::borrow::Cow::Owned(format!(
                            "XHTML conversion failed for spine item '{}'; falling back to plain text: {}",
                            document.file_path, err
                        )),
                    });
                    (extract_text_from_xhtml(xhtml).trim_end().to_string(), false)
                }
            }
        } else {
            (extract_text_from_xhtml(xhtml).trim_end().to_string(), true)
        };

        let document = if config.include_document_structure {
            let chapter_structure = crate::extraction::html::structure::build_document_structure(xhtml);

            if chapter_structure.nodes.is_empty() {
                warnings.push(ProcessingWarning {
                    source: std::borrow::Cow::Borrowed("epub"),
                    message: std::borrow::Cow::Owned(format!(
                        "Document structure extraction produced no nodes for spine item '{}'; falling back to plain-text structure",
                        document.file_path
                    )),
                });
                Some(Self::build_fallback_document_structure(xhtml, index))
            } else {
                Some(chapter_structure)
            }
        } else {
            None
        };

        RenderedSpineDocument {
            content_fragment,
            content_fully_converted,
            document,
            warnings,
        }
    }

    fn build_document_structure(
        rendered_documents: &[RenderedSpineDocument],
    ) -> Option<crate::types::document_structure::DocumentStructure> {
        use crate::types::builder::DocumentStructureBuilder;

        let mut builder = DocumentStructureBuilder::new().source_format("epub");
        let mut has_nodes = false;

        for rendered in rendered_documents {
            let Some(chapter_structure) = &rendered.document else {
                continue;
            };

            for node in &chapter_structure.nodes {
                has_nodes = true;
                builder.push_raw(
                    node.content.clone(),
                    None,
                    None,
                    node.content_layer,
                    node.annotations.clone(),
                );
            }
        }

        if has_nodes { Some(builder.build()) } else { None }
    }

    /// Build an `InternalDocument` from the EPUB spine.
    ///
    /// Walks each chapter's XHTML content using the HTML structure walker,
    /// emitting elements via `InternalDocumentBuilder`. Captures TOC-to-chapter
    /// relationships where possible by linking chapter headings.
    ///
    /// If `cover_image_path` is provided, the cover image is extracted from the
    /// archive and emitted as the first Image element.
    fn build_internal_document(
        archive: &mut ZipArchive<Cursor<Vec<u8>>>,
        spine_documents: &[content::EpubSpineDocument],
        cover_image_path: Option<&str>,
        budget: &mut SecurityBudget,
        config: &ExtractionConfig,
    ) -> Option<InternalDocument> {
        use crate::types::internal::{ElementKind, InternalElement};

        let mut builder = InternalDocumentBuilder::new("epub");

        let wants_markup = matches!(
            config.output_format,
            OutputFormat::Markdown | OutputFormat::Djot | OutputFormat::DocTags
        );
        let mut pre_rendered_fragments = Vec::new();
        let mut all_converted_successfully = wants_markup;
        let mut warnings: Vec<ProcessingWarning> = Vec::new();

        if let Some(cover_path) = cover_image_path
            && !Self::spine_references_asset(spine_documents, cover_path)
        {
            let mut buf = Vec::new();
            if let Ok(entry) = archive.by_name(cover_path) {
                let _ = entry.take(MAX_EPUB_MEMBER_SIZE).read_to_end(&mut buf);
            }
            if !buf.is_empty() {
                let fmt = cover_path
                    .rsplit('.')
                    .next()
                    .map(|ext| match ext.to_lowercase().as_str() {
                        "jpg" | "jpeg" => "jpeg",
                        "png" => "png",
                        "gif" => "gif",
                        "webp" => "webp",
                        "svg" => "svg",
                        "bmp" => "bmp",
                        _ => "png",
                    })
                    .unwrap_or("png");

                let (image_kind, kind_confidence) =
                    crate::extraction::image_kind::classify(&buf, fmt, None, None, None, None, false);

                let image = crate::types::ExtractedImage {
                    data: bytes::Bytes::from(buf),
                    format: Cow::Owned(fmt.to_string()),
                    image_index: 0,
                    page_number: Some(0),
                    width: None,
                    height: None,
                    colorspace: None,
                    bits_per_component: None,
                    is_mask: false,
                    description: Some("Cover".to_string()),
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
                builder.push_image(Some("Cover"), image, None, None);
            }
        }

        for (index, spine_doc) in spine_documents.iter().enumerate() {
            if budget.step().is_err() {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed(EPUB_WARNING_SOURCE),
                    message: Cow::Owned(format!(
                        "Iteration limit reached; {} of {} spine items were not extracted (first skipped: '{}')",
                        spine_documents.len() - index,
                        spine_documents.len(),
                        spine_doc.file_path
                    )),
                });
                break;
            }

            let file_path = &spine_doc.file_path;
            let supported_namespaces = if wants_markup {
                MARKUP_SWITCH_NAMESPACES
            } else {
                PLAIN_SWITCH_NAMESPACES
            };
            let resolved_xhtml = content::resolve_epub_switch_elements(&spine_doc.xhtml, supported_namespaces);
            let sanitized = resolved_xhtml.as_str();

            if wants_markup {
                let rendered = Self::render_spine_document(spine_doc, sanitized, index, config);
                warnings.extend(rendered.warnings);
                if rendered.content_fully_converted {
                    pre_rendered_fragments.push(rendered.content_fragment);
                } else {
                    all_converted_successfully = false;
                }
            }

            let _ = budget.account_text(sanitized.len());

            if extract_text_from_xhtml_budgeted(sanitized, budget).is_empty() && !content::has_image_markup(sanitized) {
                continue;
            }

            let chapter_structure = crate::extraction::html::structure::build_document_structure(sanitized);

            if chapter_structure.nodes.is_empty() {
                let chapter_title =
                    extract_title_from_xhtml(sanitized).unwrap_or_else(|| format!("Chapter {}", index + 1));
                builder.push_heading(1, &chapter_title, None, None);

                let text = extract_text_from_xhtml_budgeted(sanitized, budget);
                for paragraph in text.split("\n\n") {
                    let trimmed = paragraph.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    // MathML subtrees are converted to LaTeX and isolated as their
                    // own `$$...$$` block by `content::render_math_element`; route
                    // those through `push_formula` instead of `push_paragraph`. ~keep
                    if let Some(latex) = trimmed.strip_prefix("$$").and_then(|rest| rest.strip_suffix("$$")) {
                        builder.push_formula(latex, None, None);
                    } else {
                        builder.push_paragraph(trimmed, vec![], None, None);
                    }
                }
            } else {
                let mut first_heading_idx: Option<u32> = None;
                let mut in_list = false;
                // Tracks the highest direct-child index of the most recently opened
                // blockquote, so `push_quote_end()` can be emitted once that child has been
                // processed. This is a one-level approximation (direct children only, not the
                // full subtree) — correct for the common case of a blockquote with no nested
                // blockquote inside it, which covers the overwhelming majority of EPUB content. ~keep
                let mut quote_close_after: Option<u32> = None;
                for (node_index, node) in chapter_structure.nodes.iter().enumerate() {
                    use crate::types::document_structure::NodeContent;

                    if let Some(close_after) = quote_close_after
                        && node_index as u32 > close_after
                    {
                        builder.push_quote_end();
                        quote_close_after = None;
                    }

                    if in_list && !matches!(&node.content, NodeContent::ListItem { .. }) {
                        builder.end_list();
                        in_list = false;
                    }

                    match &node.content {
                        // The blockquote container itself is recorded (issue #127): its
                        // children still follow as ordinary flat entries in `nodes`, so the
                        // quote markers just need to bracket them.
                        NodeContent::Quote => {
                            builder.push_quote_start();
                            quote_close_after = node.children.iter().map(|c| c.0).max();
                            if quote_close_after.is_none() {
                                builder.push_quote_end();
                            }
                        }
                        NodeContent::Heading { level, text } => {
                            let idx = builder.push_heading(*level, text, None, None);
                            if first_heading_idx.is_none() {
                                first_heading_idx = Some(idx);
                            }
                            collect_annotation_uris(&node.annotations, text, &mut builder);
                        }
                        NodeContent::Paragraph { text } => {
                            builder.push_paragraph(text, node.annotations.clone(), None, None);
                            collect_annotation_uris(&node.annotations, text, &mut builder);
                        }
                        NodeContent::ListItem { text } => {
                            if !in_list {
                                builder.push_list(false);
                                in_list = true;
                            }
                            builder.push_list_item(text.as_str(), false, vec![], None, None);
                        }
                        NodeContent::Table { grid } => {
                            let cells: Vec<Vec<String>> = (0..grid.rows)
                                .map(|r| {
                                    grid.cells
                                        .iter()
                                        .filter(|c| c.row == r)
                                        .map(|c| c.content.clone())
                                        .collect()
                                })
                                .collect();
                            if !cells.is_empty() {
                                let cell_count: usize = cells.iter().map(|row| row.len()).sum();
                                let _ = budget.add_cells(cell_count);
                                builder.push_table_from_cells(&cells, None, None);
                            }
                        }
                        NodeContent::Code { text, language } => {
                            builder.push_code(text, language.as_deref(), None, None);
                        }
                        NodeContent::Formula { text } => {
                            builder.push_formula(text, None, None);
                        }
                        NodeContent::Image { description, src, .. } => {
                            if let Some(img_src) = src
                                && !img_src.is_empty()
                            {
                                builder.push_uri(ExtractedUri {
                                    url: img_src.clone(),
                                    label: description.clone(),
                                    page: Some((index + 1) as u32),
                                    kind: UriKind::Image,
                                });
                            }
                            let xhtml_dir = file_path.rsplit_once('/').map(|(d, _)| d).unwrap_or("");
                            let image_data = src.as_ref().and_then(|img_src| {
                                let resolved = resolve_path(xhtml_dir, img_src).ok()?;
                                let mut buf = Vec::new();
                                archive
                                    .by_name(&resolved.path)
                                    .ok()?
                                    .take(MAX_EPUB_MEMBER_SIZE)
                                    .read_to_end(&mut buf)
                                    .ok()?;
                                if buf.is_empty() {
                                    return None;
                                }
                                let fmt = img_src
                                    .rsplit('.')
                                    .next()
                                    .map(|ext| match ext.to_lowercase().as_str() {
                                        "jpg" | "jpeg" => "jpeg",
                                        "png" => "png",
                                        "gif" => "gif",
                                        "webp" => "webp",
                                        "svg" => "svg",
                                        "bmp" => "bmp",
                                        _ => "png",
                                    })
                                    .unwrap_or("png");
                                Some((buf, fmt.to_string()))
                            });

                            if let Some((data, format)) = image_data {
                                let (image_kind, kind_confidence) = crate::extraction::image_kind::classify(
                                    &data, &format, None, None, None, None, false,
                                );

                                let image = crate::types::ExtractedImage {
                                    data: bytes::Bytes::from(data),
                                    format: Cow::Owned(format),
                                    image_index: 0,
                                    page_number: Some((index + 1) as u32),
                                    width: None,
                                    height: None,
                                    colorspace: None,
                                    bits_per_component: None,
                                    is_mask: false,
                                    description: description.clone(),
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
                                builder.push_image(description.as_deref(), image, None, None);
                            } else {
                                let text_val = description.as_deref().unwrap_or("");
                                let elem =
                                    InternalElement::text(ElementKind::Image { image_index: u32::MAX }, text_val, 0);
                                builder.push_element(elem);
                            }
                        }
                        NodeContent::Group {
                            heading_text: Some(_), ..
                        } => {}
                        // Issue #127: these kinds used to fall into the catch-all below and
                        // be dropped entirely, even though the upstream HTML structure
                        // walker (`extraction::html::structure`) does produce them (e.g.
                        // `DefinitionItem` for `<dl>/<dt>/<dd>`). ~keep
                        NodeContent::DefinitionList | NodeContent::List { .. } => {}
                        NodeContent::DefinitionItem { term, definition } => {
                            builder.push_definition_term(term, None);
                            builder.push_definition_description(definition, None);
                        }
                        NodeContent::Citation { key, text } => {
                            builder.push_citation(text, key, None);
                        }
                        NodeContent::Admonition { kind, title } => {
                            builder.push_admonition(kind, title.as_deref(), None);
                        }
                        NodeContent::Footnote { text } => {
                            builder.push_footnote_definition(text, "footnote", None);
                        }
                        NodeContent::Title { text } => {
                            builder.push_heading(1, text, None, None);
                        }
                        NodeContent::PageBreak => {
                            builder.push_page_break();
                        }
                        NodeContent::RawBlock { format, content } => {
                            builder.push_raw_block(format, content, None);
                        }
                        _ => {}
                    }
                }

                if quote_close_after.is_some() {
                    builder.push_quote_end();
                }

                if in_list {
                    builder.end_list();
                }

                let _ = first_heading_idx;
            }
        }

        let mut doc = builder.build();
        doc.processing_warnings.extend(warnings);

        if all_converted_successfully && !pre_rendered_fragments.is_empty() {
            let mut combined = pre_rendered_fragments.join("\n\n");
            // html-to-markdown-rs serialises each <math> element into a comment
            // next to its rendered text; the HTML extractor strips the same
            // comment once the equation is recovered as LaTeX.
            combined = super::html::MATHML_COMMENT_RE.replace_all(&combined, "").into_owned();

            combined = combined
                .lines()
                .map(|line| line.trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            let trimmed_len = combined.trim_end().len();
            if trimmed_len > 0 {
                combined.truncate(trimmed_len);
                combined.push('\n');
            }

            doc.pre_rendered_content = Some(combined);
            // Deliberately not `OutputFormat::DocTags => "doctags"`: `combined` came from
            // `convert_html_to_markdown_with_metadata`, which has no DocTags mode and falls
            // back to Markdown-flavored text for it (`extraction::html::converter::map_output_format`).
            // Tagging that text "doctags" would make `derive_extraction_result`'s DocTags arm
            // take it verbatim instead of rendering the (correctly structured) element tree
            // via `render_doctags`. Falling through to "plain" here is intentional: it makes
            // the tag mismatch so the DocTags arm always renders from `doc.elements` instead. ~keep
            let format_name = match config.output_format {
                OutputFormat::Markdown => "markdown",
                OutputFormat::Djot => "djot",
                _ => "plain",
            };
            doc.metadata.output_format = Some(format_name.to_string());
        }

        Some(doc)
    }
}

/// `ProcessingWarning::source` for every warning this module emits.
#[cfg(feature = "office")]
const EPUB_WARNING_SOURCE: &str = "epub";

/// Extract URIs from document structure annotations (link annotations).
///
/// `ann.start`/`ann.end` are byte offsets recorded by
/// `extraction::html::structure` against the *raw* text buffer (see
/// `structure.rs:749`), but `text` here has already gone through
/// `normalize_whitespace` (`structure.rs:824`), which collapses whitespace runs
/// and trims. That shifts every recorded offset left of where it was taken, so a
/// mid-codepoint landing on a non-ASCII document is routine, not exotic -- the
/// length guard below does not imply the offsets are trustworthy.
#[cfg(feature = "office")]
fn collect_annotation_uris(
    annotations: &[crate::types::document_structure::TextAnnotation],
    text: &str,
    builder: &mut InternalDocumentBuilder,
) {
    use crate::types::document_structure::AnnotationKind;

    for ann in annotations {
        if let AnnotationKind::Link { url, .. } = &ann.kind
            && !url.is_empty()
        {
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
                    // A non-ASCII chapter can have an annotation span whose byte
                    // offsets land mid-codepoint (see the offset-drift note above);
                    // slicing on that would panic. Degrade gracefully instead: drop
                    // the label but keep the URI.
                    builder.add_warning(crate::core::diagnostics::warning(
                        EPUB_WARNING_SOURCE,
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
            builder.push_uri(ExtractedUri {
                url: url.clone(),
                label,
                page: None,
                kind: classify_uri(url),
            });
        }
    }
}

/// Extract the first heading text from XHTML content.
#[cfg(feature = "office")]
fn extract_title_from_xhtml(xhtml: &str) -> Option<String> {
    let sanitized = content::normalize_xhtml(xhtml);
    let doc = roxmltree::Document::parse(&sanitized).ok()?;

    for node in doc.root().descendants() {
        if node.is_element() {
            let tag = node.tag_name().name().to_ascii_lowercase();
            if matches!(tag.as_str(), "h1" | "h2" | "h3") {
                let text: String = node
                    .descendants()
                    .filter(|n| n.is_text())
                    .filter_map(|n| n.text())
                    .collect::<Vec<_>>()
                    .join("");
                let trimmed = text.trim().to_string();
                if !trimmed.is_empty() {
                    return Some(trimmed);
                }
            }
        }
    }
    None
}

impl Plugin for EpubExtractor {
    fn name(&self) -> &str {
        "epub-extractor"
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

    fn description(&self) -> &str {
        "Extracts content and metadata from EPUB documents (native Rust implementation with permissive licenses)"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg(feature = "office")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for EpubExtractor {
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, content, config),
            fields(
                extractor.name = self.name(),
                content.size_bytes = content.len(),
            )
        )
    )]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "epub", size_bytes = content.len(), "extraction starting");
        let mut budget = SecurityBudget::from_config(config);
        let cursor = Cursor::new(content.to_vec());

        let mut archive = ZipArchive::new(cursor).map_err(|e| crate::XbergError::Parsing {
            message: format!("Failed to open EPUB as ZIP: {}", e),
            source: None,
        })?;

        let security_limits = config.security_limits.clone().unwrap_or_default();
        ZipBombValidator::new(security_limits)
            .validate(&mut archive)
            .map_err(|e| crate::XbergError::validation(e.to_string()))?;

        let container_xml = read_file_from_zip(&mut archive, "META-INF/container.xml")?;
        let opf_path = parse_container_xml(&container_xml)?;

        let manifest_dir = if let Some(last_slash) = opf_path.rfind('/') {
            opf_path[..last_slash].to_string()
        } else {
            String::new()
        };

        let opf_xml = read_file_from_zip(&mut archive, &opf_path)?;
        let (package, mut processing_warnings) = parse_opf(&opf_xml, &manifest_dir, &mut budget)?;
        let additional_metadata = build_additional_metadata(&package.metadata);
        let epub_format_metadata = FormatMetadata::Epub(EpubMetadata {
            coverage: package.metadata.coverage.clone(),
            dc_format: package.metadata.format.clone(),
            relation: package.metadata.relation.clone(),
            source: package.metadata.source.clone(),
            dc_type: package.metadata.dc_type.clone(),
            cover_image: package.metadata.cover_image_href.clone(),
        });

        let (spine_documents, mut body_warnings) = content::read_body_documents(&mut archive, &package)?;
        processing_warnings.append(&mut body_warnings);

        let metadata_map: AHashMap<Cow<'static, str>, serde_json::Value> = additional_metadata
            .into_iter()
            .map(|(k, v)| (Cow::Owned(k), v))
            .collect();

        let cover_image_path = package.metadata.cover_image_href.as_deref();
        let mut doc =
            Self::build_internal_document(&mut archive, &spine_documents, cover_image_path, &mut budget, config)
                .unwrap_or_else(|| InternalDocumentBuilder::new("epub").build());
        doc.mime_type = mime_type.to_string();
        doc.processing_warnings.extend(processing_warnings);

        let output_format = doc.metadata.output_format.take();
        let creators = package.metadata.creators;
        let subjects = package.metadata.subjects;
        doc.metadata = Metadata {
            title: package.metadata.title,
            authors: (!creators.is_empty()).then_some(creators),
            keywords: (!subjects.is_empty()).then_some(subjects),
            language: package.metadata.language,
            created_at: package.metadata.date,
            format: Some(epub_format_metadata),
            additional: metadata_map,
            output_format,
            ..Default::default()
        };

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "epub",
            "extraction complete"
        );
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/epub+zip",
            "application/x-epub+zip",
            "application/vnd.epub+zip",
        ]
    }

    fn priority(&self) -> i32 {
        60
    }
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;

    #[test]
    fn test_epub_extractor_plugin_interface() {
        let extractor = EpubExtractor::new();
        assert_eq!(extractor.name(), "epub-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 60);
        assert!(!extractor.supported_mime_types().is_empty());
    }

    #[test]
    fn test_epub_extractor_default() {
        let extractor = EpubExtractor;
        assert_eq!(extractor.name(), "epub-extractor");
    }

    #[tokio::test]
    async fn test_epub_extractor_initialize_shutdown() {
        let extractor = EpubExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_epub_extractor_supported_mime_types() {
        let extractor = EpubExtractor::new();
        let supported = extractor.supported_mime_types();
        assert!(supported.contains(&"application/epub+zip"));
        assert!(supported.contains(&"application/x-epub+zip"));
        assert!(supported.contains(&"application/vnd.epub+zip"));
    }

    #[test]
    fn should_detect_cover_asset_already_rendered_by_spine() {
        let documents = vec![content::EpubSpineDocument {
            file_path: "OEBPS/text/cover.xhtml".to_string(),
            xhtml: r#"<html><body><img src="../images/cover.jpg" alt="Cover"/></body></html>"#.to_string(),
        }];

        assert!(EpubExtractor::spine_references_asset(
            &documents,
            "OEBPS/images/cover.jpg"
        ));
        assert!(!EpubExtractor::spine_references_asset(
            &documents,
            "OEBPS/images/other.jpg"
        ));
    }

    #[test]
    fn test_epub_full_dublin_core_metadata() {
        let opf = r#"<?xml version="1.0" encoding="UTF-8"?>
<package xmlns="http://www.idpf.org/2007/opf" version="3.0">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Test Book</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:language>en</dc:language>
    <dc:coverage>Worldwide</dc:coverage>
    <dc:format>application/epub+zip</dc:format>
    <dc:relation>http://example.com/related</dc:relation>
    <dc:source>Original Manuscript</dc:source>
    <dc:type>Text</dc:type>
    <dc:publisher>Test Publisher</dc:publisher>
    <dc:description>A test book</dc:description>
    <dc:rights>CC BY 4.0</dc:rights>
    <meta name="cover" content="cover-img"/>
  </metadata>
  <manifest>
    <item id="cover-img" href="images/cover.jpg" media-type="image/jpeg"/>
    <item id="ch1" href="ch1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="ch1"/>
  </spine>
</package>"#;

        let mut budget = crate::extractors::security::SecurityBudget::with_defaults();
        let (package, _warnings) = metadata::parse_opf(opf, "", &mut budget).expect("Metadata parse failed");
        let epub_meta = &package.metadata;
        assert_eq!(epub_meta.title, Some("Test Book".to_string()));
        assert_eq!(epub_meta.coverage, Some("Worldwide".to_string()));
        assert_eq!(epub_meta.format, Some("application/epub+zip".to_string()));
        assert_eq!(epub_meta.relation, Some("http://example.com/related".to_string()));
        assert_eq!(epub_meta.source, Some("Original Manuscript".to_string()));
        assert_eq!(epub_meta.dc_type, Some("Text".to_string()));
        assert_eq!(epub_meta.cover_image_href, Some("images/cover.jpg".to_string()));

        let format_meta = FormatMetadata::Epub(EpubMetadata {
            coverage: epub_meta.coverage.clone(),
            dc_format: epub_meta.format.clone(),
            relation: epub_meta.relation.clone(),
            source: epub_meta.source.clone(),
            dc_type: epub_meta.dc_type.clone(),
            cover_image: epub_meta.cover_image_href.clone(),
        });
        match &format_meta {
            FormatMetadata::Epub(em) => {
                assert_eq!(em.coverage.as_deref(), Some("Worldwide"));
                assert_eq!(em.dc_format.as_deref(), Some("application/epub+zip"));
                assert_eq!(em.relation.as_deref(), Some("http://example.com/related"));
                assert_eq!(em.source.as_deref(), Some("Original Manuscript"));
                assert_eq!(em.dc_type.as_deref(), Some("Text"));
                assert_eq!(em.cover_image.as_deref(), Some("images/cover.jpg"));
            }
            _ => panic!("Expected FormatMetadata::Epub variant"),
        }

        let additional = metadata::build_additional_metadata(epub_meta);
        assert!(additional.contains_key("publisher"));
        assert!(additional.contains_key("description"));
        assert!(additional.contains_key("rights"));
        assert!(!additional.contains_key("coverage"));
        assert!(!additional.contains_key("format"));
        assert!(!additional.contains_key("relation"));
        assert!(!additional.contains_key("source"));
        assert!(!additional.contains_key("type"));
        assert!(!additional.contains_key("cover_image"));
    }
}
