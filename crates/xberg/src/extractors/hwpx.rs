//! Hangul Word Processor XML (.hwpx) extractor.
//!
//! Extracts text, headings, tables, and images from HWPX documents using the `unhwp` crate.

use std::borrow::Cow;
use std::io::Cursor;

use async_trait::async_trait;
use bytes::Bytes;

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extractors::security::ZipBombValidator;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
use crate::types::document_structure::{AnnotationKind, ContentLayer, TextAnnotation};
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const HWPX_WARNING_SOURCE: &str = "hwpx";

/// Extractor for Hangul Word Processor XML (.hwpx) files.
///
/// Supports HWPX (Open HWPML), the ZIP-based XML successor to the binary HWP 5.0 format.
#[cfg_attr(alef, alef(skip))]
pub struct HwpxExtractor;

impl HwpxExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for HwpxExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for HwpxExtractor {
    fn name(&self) -> &str {
        "hwpx-extractor"
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
        "Hangul Word Processor XML (.hwpx) text extraction"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

fn mime_to_format(mime: &str) -> Cow<'static, str> {
    match mime {
        "image/png" => Cow::Borrowed("png"),
        "image/jpeg" | "image/jpg" => Cow::Borrowed("jpeg"),
        "image/gif" => Cow::Borrowed("gif"),
        "image/bmp" => Cow::Borrowed("bmp"),
        "image/webp" => Cow::Borrowed("webp"),
        // HWPX embeds vector/legacy Windows metafiles (WMF/EMF) and SVG far more often
        // than other office formats — `unhwp`'s resource guesser (hwpx/mod.rs
        // `guess_mime_type`) returns these three MIME strings, and previously every one
        // of them fell through to the generic "bin" bucket, discarding the real format
        // (#120).
        "image/svg+xml" => Cow::Borrowed("svg"),
        "image/x-wmf" => Cow::Borrowed("wmf"),
        "image/x-emf" => Cow::Borrowed("emf"),
        _ => Cow::Borrowed("bin"),
    }
}

/// Collect the LaTeX of every equation, one `Vec` per section, in document order.
///
/// `unhwp` reads an equation only when it sits in a paragraph-level `<hp:ctrl>`;
/// its run reader drops `<hp:equation>`. Hangul writes the equation into the run,
/// so a real document loses every formula. Reading the section XML through the
/// crate's own container keeps the archive, the section order and the script
/// conversion with `unhwp`, and adds only the search for one element.
fn collect_section_formulas(content: &[u8]) -> Vec<Vec<String>> {
    use quick_xml::events::Event;

    let Ok(mut container) = unhwp::hwpx::HwpxContainer::from_bytes(content.to_vec()) else {
        return Vec::new();
    };
    let Ok(section_paths) = container.list_sections() else {
        return Vec::new();
    };

    let mut per_section = Vec::with_capacity(section_paths.len());
    for path in &section_paths {
        let Ok(xml) = container.read_file(path) else {
            per_section.push(Vec::new());
            continue;
        };

        let mut formulas = Vec::new();
        let mut reader = quick_xml::Reader::from_str(&xml);
        let mut in_equation = false;
        let mut in_script = false;
        let mut script = String::new();
        loop {
            match reader.read_event() {
                Ok(Event::Start(e)) => match local_name(e.name().as_ref()) {
                    "equation" | "eqEdit" => in_equation = true,
                    "script" if in_equation => in_script = true,
                    _ => {}
                },
                Ok(Event::Text(t)) if in_script => {
                    script.push_str(&String::from_utf8_lossy(t.as_ref()));
                }
                Ok(Event::End(e)) => match local_name(e.name().as_ref()) {
                    "script" => in_script = false,
                    "equation" | "eqEdit" => {
                        let latex = unhwp::equation::to_latex(std::mem::take(&mut script).trim());
                        if !latex.trim().is_empty() {
                            formulas.push(latex.trim().to_string());
                        }
                        in_equation = false;
                    }
                    _ => {}
                },
                Ok(Event::Eof) | Err(_) => break,
                _ => {}
            }
        }
        per_section.push(formulas);
    }
    per_section
}

/// Return the local part of a possibly prefixed XML qualified name.
fn local_name(qname: &[u8]) -> &str {
    let name = std::str::from_utf8(qname).unwrap_or("");
    match name.rsplit_once(':') {
        Some((_, local)) => local,
        None => name,
    }
}

fn build_hwpx_internal_document(
    doc: unhwp::model::Document,
    mime_type: &str,
    section_formulas: &[Vec<String>],
) -> InternalDocument {
    let mut builder = InternalDocumentBuilder::new("hwpx");
    builder.set_mime_type(Cow::Owned(mime_type.to_string()));

    let mut metadata = crate::types::metadata::Metadata::default();
    if let Some(title) = &doc.metadata.title {
        metadata.title = Some(title.clone());
    }
    if let Some(author) = &doc.metadata.author {
        metadata.authors = Some(vec![author.clone()]);
    }
    if let Some(subject) = &doc.metadata.subject {
        metadata.subject = Some(subject.clone());
    }
    if !doc.metadata.keywords.is_empty() {
        metadata.keywords = Some(doc.metadata.keywords.clone());
    }
    if let Some(created) = &doc.metadata.created {
        metadata.created_at = Some(created.clone());
    }
    if let Some(modified) = &doc.metadata.modified {
        metadata.modified_at = Some(modified.clone());
    }
    if let Some(creator_app) = &doc.metadata.creator_app {
        metadata.additional.insert(
            Cow::Borrowed("creator_app"),
            serde_json::Value::String(creator_app.clone()),
        );
    }
    if let Some(version) = &doc.metadata.format_version {
        metadata.document_version = Some(version.clone());
    }
    if !metadata.is_empty() {
        builder.set_metadata(metadata);
    }

    let mut image_index: usize = 0;
    let mut footnote_counter: u32 = 0;

    for (section_index, section) in doc.sections.iter().enumerate() {
        // Section headers/footers (`unhwp::model::Section::header`/`footer`) previously
        // went unread entirely — only `section.content` was visited (#96). ~keep
        if let Some(header_paragraphs) = &section.header {
            push_header_footer_paragraphs(
                &mut builder,
                header_paragraphs,
                ContentLayer::Header,
                &mut footnote_counter,
            );
        }
        if let Some(footer_paragraphs) = &section.footer {
            push_header_footer_paragraphs(
                &mut builder,
                footer_paragraphs,
                ContentLayer::Footer,
                &mut footnote_counter,
            );
        }

        for block in &section.content {
            match block {
                unhwp::model::Block::Paragraph(p) => {
                    // `Paragraph::has_text_content()` does not count `InlineContent::Equation`
                    // (see `unhwp::model::paragraph`), so an equation-only paragraph (no
                    // surrounding prose) would otherwise be dropped here before its LaTeX
                    // ever reached `build_paragraph_content` (#98). ~keep
                    let has_equation = p
                        .content
                        .iter()
                        .any(|c| matches!(c, unhwp::model::InlineContent::Equation(_)));
                    let (text, annotations) = build_paragraph_content(&mut builder, p, &mut footnote_counter);
                    let (trimmed, adjusted) = trim_text_and_annotations(&text, annotations);
                    if p.style.is_heading() && (p.has_text_content() || has_equation) {
                        if !trimmed.is_empty() {
                            let idx = builder.push_heading(p.style.heading_level, trimmed, None, None);
                            if !adjusted.is_empty() {
                                builder.set_annotations(idx, adjusted);
                            }
                        }
                    } else if (p.has_text_content() || has_equation) && !trimmed.is_empty() {
                        builder.push_paragraph(trimmed, adjusted, None, None);
                    }

                    for inline in &p.content {
                        if let unhwp::model::InlineContent::Image(img_ref) = inline {
                            if let Some(resource) = doc.resources.get(&img_ref.id) {
                                let image = ExtractedImage {
                                    data: Bytes::from(resource.data.clone()),
                                    format: mime_to_format(resource.mime_type.as_deref().unwrap_or("")),
                                    image_index: image_index as u32,
                                    page_number: None,
                                    width: img_ref.width,
                                    height: img_ref.height,
                                    colorspace: None,
                                    bits_per_component: None,
                                    is_mask: false,
                                    description: img_ref.alt_text.clone(),
                                    ocr_result: None,
                                    bounding_box: None,
                                    source_path: None,
                                    image_kind: None,
                                    kind_confidence: None,
                                    cluster_id: None,
                                    caption: None,
                                    qr_codes: None,
                                    data_base64: None,
                                };
                                builder.push_image(img_ref.alt_text.as_deref(), image, None, None);
                                image_index += 1;
                            } else {
                                // `img_ref.id` names a resource that `doc.resources` never
                                // received a binary payload for (a missing/unpacked media
                                // part). A well-formed HWPX package always has a resource
                                // entry for every image reference it emits, so this only
                                // fires on a genuinely incomplete document -- and unlike the
                                // resolved case above, nothing is pushed for it at all: the
                                // image is silently absent from the output (#171).
                                builder.add_warning(crate::core::diagnostics::warning(
                                    HWPX_WARNING_SOURCE,
                                    format!(
                                        "Image reference '{}' has no corresponding entry in the document's \
                                         resources; the image could not be extracted",
                                        img_ref.id
                                    ),
                                ));
                            }
                        }
                    }
                }
                unhwp::model::Block::Table(t) => {
                    if !t.rows.is_empty() {
                        let mut cells: Vec<Vec<String>> = Vec::with_capacity(t.rows.len());
                        for row in &t.rows {
                            let mut row_cells = Vec::with_capacity(row.cells.len());
                            for cell in &row.cells {
                                row_cells.push(cell_plain_text(&mut builder, cell, &mut footnote_counter));
                            }
                            cells.push(row_cells);
                        }
                        push_table(&mut builder, cells, t.has_header);
                    }
                }
            }
        }

        // The scan sees every equation, wherever it sits, so it is the only
        // source of formula elements; the model path handles the text spacing.
        for latex in section_formulas.get(section_index).into_iter().flatten() {
            builder.push_formula(latex, None, None);
        }
    }

    builder.build()
}

/// Push a header's or footer's paragraphs, tagging every element with the given
/// `ContentLayer` so downstream filters (`include_headers`/`include_footers`) can find
/// them (#96 — these were previously not read from `unhwp`'s model at all).
fn push_header_footer_paragraphs(
    builder: &mut InternalDocumentBuilder,
    paragraphs: &[unhwp::model::Paragraph],
    layer: ContentLayer,
    footnote_counter: &mut u32,
) {
    for p in paragraphs {
        let (text, annotations) = build_paragraph_content(builder, p, footnote_counter);
        let (trimmed, adjusted) = trim_text_and_annotations(&text, annotations);
        if trimmed.is_empty() {
            continue;
        }
        let idx = builder.push_paragraph(trimmed, adjusted, None, None);
        builder.set_layer(idx, layer);
    }
}

/// Builds a paragraph's flattened text plus byte-offset annotations from its inline
/// content, and records footnote definitions as a side effect.
///
/// This replaces the previous reliance on `Paragraph::plain_text()`, which silently
/// dropped footnote text (`InlineContent::Footnote` fell through `plain_text()`'s
/// catch-all arm) and hyperlink targets (only the anchor text survived) (#97), and
/// rendered equations as their raw HWP script instead of the LaTeX `unhwp` already
/// computes (#98).
fn build_paragraph_content(
    builder: &mut InternalDocumentBuilder,
    p: &unhwp::model::Paragraph,
    footnote_counter: &mut u32,
) -> (String, Vec<TextAnnotation>) {
    let mut text = String::new();
    let mut annotations = Vec::new();
    // An equation leaves the text, so the spaces that surrounded it would
    // otherwise meet and read as a gap.
    let mut after_equation = false;

    for item in &p.content {
        match item {
            unhwp::model::InlineContent::Text(run) => {
                let value = if after_equation && text.ends_with(char::is_whitespace) {
                    run.text.trim_start()
                } else {
                    run.text.as_str()
                };
                text.push_str(value);
                after_equation = false;
            }
            unhwp::model::InlineContent::LineBreak => text.push('\n'),
            unhwp::model::InlineContent::Link { text: link_text, url } => {
                let start = text.len() as u32;
                text.push_str(link_text);
                let end = text.len() as u32;
                if start < end {
                    annotations.push(TextAnnotation {
                        start,
                        end,
                        kind: AnnotationKind::Link {
                            url: url.clone(),
                            title: None,
                        },
                    });
                }
            }
            unhwp::model::InlineContent::Equation(eq) => {
                // `unhwp::equation::to_latex` converts HWP's own EQEdit script DSL
                // (not MathML/OMML — HWP equations use a distinct script format) to
                // LaTeX. The parser leaves `Equation.latex` unset (only `unhwp`'s own
                // Markdown renderer calls `to_latex`), so it is invoked directly here
                // instead of duplicating a second HWP-script-to-LaTeX converter (#98). ~keep
                let latex = eq
                    .latex
                    .clone()
                    .unwrap_or_else(|| unhwp::equation::to_latex(&eq.script));
                // An HWP equation is an object rather than inline notation, so it
                // leaves the paragraph text, the shape DOCX and PPTX use for a math
                // run. `collect_section_formulas` emits the element for it.
                if !latex.trim().is_empty() {
                    after_equation = true;
                }
            }
            unhwp::model::InlineContent::Footnote(note_text) => {
                *footnote_counter += 1;
                let key = format!("hwpx-fn{footnote_counter}");
                text.push_str(&format!("[^{footnote_counter}]"));
                if !note_text.trim().is_empty() {
                    let idx = builder.push_footnote_definition(note_text.trim(), &key, None);
                    builder.set_layer(idx, ContentLayer::Footnote);
                }
            }
            unhwp::model::InlineContent::Image(_) => {
                // Images are extracted separately by the caller, which has access to
                // `doc.resources` for the binary payload.
            }
        }
    }

    (text, annotations)
}

/// Trims leading/trailing whitespace from `text` and shifts `annotations`' byte
/// offsets to match, dropping any annotation that fell entirely within the trimmed
/// prefix. Byte offsets are computed against the untrimmed text in
/// `build_paragraph_content`, so trimming without this adjustment would silently
/// misalign every annotation.
fn trim_text_and_annotations(text: &str, annotations: Vec<TextAnnotation>) -> (&str, Vec<TextAnnotation>) {
    let trimmed = text.trim();
    let trim_start = (text.len() - text.trim_start().len()) as u32;
    let trimmed_len = trimmed.len() as u32;

    let adjusted = annotations
        .into_iter()
        .filter_map(|mut annotation| {
            if annotation.end <= trim_start {
                return None;
            }
            annotation.start = annotation.start.saturating_sub(trim_start).min(trimmed_len);
            annotation.end = annotation.end.saturating_sub(trim_start).min(trimmed_len);
            if annotation.start >= annotation.end {
                None
            } else {
                Some(annotation)
            }
        })
        .collect();

    (trimmed, adjusted)
}

/// Flattens a table cell's paragraphs to a single string for the cell grid, still
/// routing through `build_paragraph_content` so footnotes/links/equations inside a
/// cell are captured rather than silently dropped by `TableCell::plain_text()` (#120).
fn cell_plain_text(
    builder: &mut InternalDocumentBuilder,
    cell: &unhwp::model::TableCell,
    footnote_counter: &mut u32,
) -> String {
    let mut lines = Vec::with_capacity(cell.content.len());
    for p in &cell.content {
        // `build_paragraph_content`'s `InlineContent::Image` arm is a no-op: it relies
        // on a second pass over `p.content` that only the top-level section/paragraph
        // loop performs (to reach `doc.resources` for the binary payload). Table cells
        // never get that second pass, so an image inside a cell is dropped outright --
        // not even as a placeholder -- unlike a resolvable top-level image (#171). ~keep
        if p.content
            .iter()
            .any(|c| matches!(c, unhwp::model::InlineContent::Image(_)))
        {
            builder.add_warning(crate::core::diagnostics::warning(
                HWPX_WARNING_SOURCE,
                "A table cell contains an image; images inside table cells are not \
                 extracted and were omitted from the output",
            ));
        }
        let (text, _annotations) = build_paragraph_content(builder, p, footnote_counter);
        lines.push(text.trim().to_string());
    }
    lines.join("\n")
}

/// Pushes a table, setting `Table::columns` from the first row when `unhwp` reports a
/// header row — previously always `None` regardless of `has_header` (#120). Note:
/// `unhwp`'s HWPX parser currently sets `has_header` on every non-empty table
/// (`hwpx/section.rs::parse_table`), so this is best-effort until that upstream signal
/// is more precise; it is still strictly more informative than never populating
/// `columns` at all.
fn push_table(builder: &mut InternalDocumentBuilder, cells: Vec<Vec<String>>, has_header: bool) -> u32 {
    let markdown = crate::rendering::common::render_table_markdown(&cells);
    let columns = if has_header { cells.first().cloned() } else { None };
    let table = crate::types::Table {
        cells,
        markdown,
        columns,
        ..Default::default()
    };
    builder.push_table(table, None, None)
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for HwpxExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();

        if content.len() as u64 > limits.max_archive_size as u64 {
            return Err(crate::XbergError::validation(format!(
                "HWPX file exceeds size limit ({} > {} bytes)",
                content.len(),
                limits.max_archive_size
            )));
        }

        let cursor = Cursor::new(content);
        let mut archive =
            zip::ZipArchive::new(cursor).map_err(|e| crate::XbergError::parsing(format!("invalid HWPX zip: {e}")))?;
        ZipBombValidator::new(limits)
            .validate(&mut archive)
            .map_err(|e| crate::XbergError::validation(e.to_string()))?;

        let doc = unhwp::parse_bytes(content)
            .map_err(|e| crate::XbergError::parsing(format!("Failed to parse HWPX: {e}")))?;
        let section_formulas = collect_section_formulas(content);
        Ok(build_hwpx_internal_document(doc, mime_type, &section_formulas))
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/haansofthwpx"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal::ElementKind;
    use unhwp::model::{
        Block, Document, Equation, InlineContent, Paragraph, Section, Table, TableCell, TableRow, TextRun,
    };

    #[test]
    fn test_mime_to_format_maps_svg_wmf_emf() {
        assert_eq!(mime_to_format("image/svg+xml"), Cow::Borrowed("svg"));
        assert_eq!(mime_to_format("image/x-wmf"), Cow::Borrowed("wmf"));
        assert_eq!(mime_to_format("image/x-emf"), Cow::Borrowed("emf"));
        assert_eq!(mime_to_format("image/png"), Cow::Borrowed("png"));
        assert_eq!(mime_to_format("application/octet-stream"), Cow::Borrowed("bin"));
    }

    #[test]
    fn test_section_header_and_footer_are_extracted() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        section.header = Some(vec![Paragraph::text("Confidential Draft")]);
        section.footer = Some(vec![Paragraph::text("Page footer text")]);
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let header = internal
            .elements
            .iter()
            .find(|e| e.layer == ContentLayer::Header)
            .expect("header element must be present");
        assert_eq!(header.text, "Confidential Draft");

        let footer = internal
            .elements
            .iter()
            .find(|e| e.layer == ContentLayer::Footer)
            .expect("footer element must be present");
        assert_eq!(footer.text, "Page footer text");
    }

    #[test]
    fn test_footnote_produces_marker_and_definition() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.push_text(TextRun::new("See "));
        p.content.push(InlineContent::Footnote("The note body.".to_string()));
        p.push_text(TextRun::new(" done"));
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let body = internal
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::Paragraph)
            .expect("body paragraph must be present");
        assert_eq!(body.text, "See [^1] done");

        let definition = internal
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::FootnoteDefinition)
            .expect("footnote definition element must be present");
        assert_eq!(definition.text, "The note body.");
        assert_eq!(definition.layer, ContentLayer::Footnote);
    }

    #[test]
    fn test_link_produces_link_annotation_with_correct_offsets() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.push_text(TextRun::new("Go to "));
        p.content.push(InlineContent::Link {
            text: "our site".to_string(),
            url: "https://example.com".to_string(),
        });
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let elem = internal
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::Paragraph)
            .expect("paragraph must be present");
        assert_eq!(elem.text, "Go to our site");
        assert_eq!(elem.annotations.len(), 1);
        let annotation = &elem.annotations[0];
        assert_eq!(annotation.start, 6);
        assert_eq!(annotation.end, 14);
        assert_eq!(
            annotation.kind,
            AnnotationKind::Link {
                url: "https://example.com".to_string(),
                title: None,
            }
        );
    }

    /// An equation is an object, so its LaTeX becomes a formula element and
    /// leaves the paragraph's own text.
    #[test]
    fn test_equation_becomes_a_formula_element() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.push_text(TextRun::new("Result: "));
        p.content.push(InlineContent::Equation(Equation::new("FRAC{a}{b}")));
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);

        let section_formulas = vec![vec!["\\frac{a}{b}".to_string()]];
        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &section_formulas);

        let formulas: Vec<&str> = internal
            .elements
            .iter()
            .filter(|e| e.kind == ElementKind::Formula)
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(formulas, vec!["\\frac{a}{b}"]);

        let elem = internal
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::Paragraph)
            .expect("paragraph must be present");
        assert_eq!(elem.text, "Result:");
    }

    /// The spaces that surrounded an equation would meet once it leaves the
    /// text, so the sentence keeps exactly one.
    #[test]
    fn test_removing_an_equation_leaves_one_space() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.push_text(TextRun::new("The ratio is "));
        p.content.push(InlineContent::Equation(Equation::new("x OVER y")));
        p.push_text(TextRun::new(" per unit."));
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let para = internal
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::Paragraph)
            .expect("paragraph must be present");
        assert_eq!(para.text, "The ratio is per unit.");
    }

    /// A paragraph holding nothing but an equation still carries its math: the
    /// formula element stands in for the paragraph that no longer has text.
    #[test]
    fn test_equation_only_paragraph_keeps_its_math() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.content.push(InlineContent::Equation(Equation::new("FRAC{a}{b}")));
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);

        let section_formulas = vec![vec!["\\frac{a}{b}".to_string()]];
        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &section_formulas);

        let formulas: Vec<&str> = internal
            .elements
            .iter()
            .filter(|e| e.kind == ElementKind::Formula)
            .map(|e| e.text.as_str())
            .collect();
        assert_eq!(formulas, vec!["\\frac{a}{b}"], "the equation survives (#98)");
    }

    #[test]
    fn test_table_header_row_populates_columns_and_cell_content() {
        let mut doc = Document::new();
        let mut section = Section::new(0);

        let mut table = Table::new();
        table.has_header = true;
        let mut header_row = TableRow::new();
        header_row.cells.push(TableCell::text("Name"));
        header_row.cells.push(TableCell::text("Age"));
        table.rows.push(header_row);
        let mut data_row = TableRow::new();
        data_row.cells.push(TableCell::text("Alice"));
        data_row.cells.push(TableCell::text("30"));
        table.rows.push(data_row);

        section.content.push(Block::Table(table));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let table_elem = internal
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::Table { .. }))
            .expect("table element must be present");
        let ElementKind::Table { table_index } = table_elem.kind else {
            unreachable!()
        };
        let extracted_table = &internal.tables[table_index as usize];
        assert_eq!(
            extracted_table.columns,
            Some(vec!["Name".to_string(), "Age".to_string()])
        );
        assert_eq!(
            extracted_table.cells,
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ]
        );
    }

    #[test]
    fn test_table_without_header_leaves_columns_unset() {
        let mut doc = Document::new();
        let mut section = Section::new(0);

        let mut table = Table::new();
        table.has_header = false;
        let mut row = TableRow::new();
        row.cells.push(TableCell::text("A"));
        table.rows.push(row);

        section.content.push(Block::Table(table));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let table_elem = internal
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::Table { .. }))
            .expect("table element must be present");
        let ElementKind::Table { table_index } = table_elem.kind else {
            unreachable!()
        };
        assert_eq!(internal.tables[table_index as usize].columns, None);
    }

    #[test]
    fn test_table_cell_footnote_is_extracted_not_dropped() {
        let mut doc = Document::new();
        let mut section = Section::new(0);

        let mut table = Table::new();
        let mut row = TableRow::new();
        let mut cell = TableCell::new();
        let mut cell_para = Paragraph::new();
        cell_para.content.push(InlineContent::Footnote("cell note".to_string()));
        cell_para.push_text(TextRun::new("cell body"));
        cell.content.push(cell_para);
        row.cells.push(cell);
        table.rows.push(row);

        section.content.push(Block::Table(table));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let definition = internal
            .elements
            .iter()
            .find(|e| e.kind == ElementKind::FootnoteDefinition)
            .expect("footnote inside a table cell must still produce a definition");
        assert_eq!(definition.text, "cell note");
    }

    fn hwpx_warnings(doc: &InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == HWPX_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #171: an `InlineContent::Image` whose `id` has no entry in
    /// `doc.resources` cannot be resolved to binary data and is dropped
    /// entirely (no image element, no placeholder).
    #[test]
    fn should_warn_when_image_resource_id_is_missing_from_resources() {
        let mut doc = Document::new();
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.content
            .push(InlineContent::Image(unhwp::model::ImageRef::new("bin0")));
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);
        // Deliberately leave `doc.resources` empty: "bin0" is never registered.

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let warnings = hwpx_warnings(&internal);
        assert_eq!(warnings.len(), 1, "expected exactly one hwpx warning, got {warnings:?}");
        assert!(
            warnings[0].contains("bin0") && warnings[0].contains("could not be extracted"),
            "warning must name the unresolved image id, got {warnings:?}"
        );
        assert!(
            internal.images.is_empty(),
            "an unresolved image must not produce an image element"
        );
    }

    /// A resolvable image (its id present in `doc.resources`) must not warn.
    #[test]
    fn should_not_warn_when_image_resource_resolves() {
        let mut doc = Document::new();
        doc.resources.insert(
            "bin0".to_string(),
            unhwp::model::Resource::new(unhwp::model::ResourceType::Image, vec![0x89, 0x50, 0x4E, 0x47]),
        );
        let mut section = Section::new(0);
        let mut p = Paragraph::new();
        p.content
            .push(InlineContent::Image(unhwp::model::ImageRef::new("bin0")));
        section.content.push(Block::Paragraph(p));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        assert!(
            hwpx_warnings(&internal).is_empty(),
            "a resolvable image must not warn, got {:?}",
            hwpx_warnings(&internal)
        );
    }

    /// #171: `build_paragraph_content`'s `InlineContent::Image` arm is a no-op
    /// everywhere; only the top-level section/paragraph loop performs the
    /// second pass that actually resolves and pushes an image element. A
    /// table cell never gets that second pass, so an image inside a cell is
    /// dropped outright.
    #[test]
    fn should_warn_when_table_cell_contains_an_image() {
        let mut doc = Document::new();
        doc.resources.insert(
            "bin0".to_string(),
            unhwp::model::Resource::new(unhwp::model::ResourceType::Image, vec![0x89, 0x50, 0x4E, 0x47]),
        );
        let mut section = Section::new(0);

        let mut table = Table::new();
        let mut row = TableRow::new();
        let mut cell = TableCell::new();
        let mut cell_para = Paragraph::new();
        cell_para
            .content
            .push(InlineContent::Image(unhwp::model::ImageRef::new("bin0")));
        cell.content.push(cell_para);
        row.cells.push(cell);
        table.rows.push(row);

        section.content.push(Block::Table(table));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        let warnings = hwpx_warnings(&internal);
        assert_eq!(warnings.len(), 1, "expected exactly one hwpx warning, got {warnings:?}");
        assert!(
            warnings[0].contains("table cell") && warnings[0].contains("not extracted"),
            "warning must describe the dropped table-cell image, got {warnings:?}"
        );
        assert!(
            internal.images.is_empty(),
            "an image inside a table cell must not produce an image element"
        );
    }

    /// An ordinary table with only text cells must not warn.
    #[test]
    fn should_not_warn_for_table_with_only_text_cells() {
        let mut doc = Document::new();
        let mut section = Section::new(0);

        let mut table = Table::new();
        let mut row = TableRow::new();
        row.cells.push(TableCell::text("Alice"));
        row.cells.push(TableCell::text("30"));
        table.rows.push(row);

        section.content.push(Block::Table(table));
        doc.sections.push(section);

        let internal = build_hwpx_internal_document(doc, "application/haansofthwpx", &[]);

        assert!(
            hwpx_warnings(&internal).is_empty(),
            "a table with only text cells must not warn, got {:?}",
            hwpx_warnings(&internal)
        );
    }

    #[test]
    fn test_hwpx_extractor_plugin_interface() {
        let extractor = HwpxExtractor::new();
        assert_eq!(extractor.name(), "hwpx-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert_eq!(extractor.supported_mime_types(), &["application/haansofthwpx"]);
    }

    #[test]
    fn test_hwpx_extractor_initialize_shutdown() {
        let extractor = HwpxExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[tokio::test]
    async fn test_hwpx_extract_real_document() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../test_documents/hwpx/simple.hwpx");
        let content = std::fs::read(path).expect("test_documents/hwpx/simple.hwpx must exist");
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(&content, "application/haansofthwpx", &ExtractionConfig::default())
            .await
            .expect("extraction of simple.hwpx must succeed");

        let text = result.content();
        assert!(
            text.contains("Hello from HWPX document"),
            "expected body text not found; got: {text}"
        );
    }

    #[tokio::test]
    async fn test_hwpx_extract_corrupted_returns_err() {
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(b"not a zip", "application/haansofthwpx", &ExtractionConfig::default())
            .await;
        assert!(result.is_err(), "corrupted input must return Err, not panic");
    }

    fn make_zip_with_ratio(uncompressed_len: usize) -> Vec<u8> {
        use std::io::Write as _;
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
        zw.start_file("content.hml", opts).unwrap();
        zw.write_all(&vec![0u8; uncompressed_len]).unwrap();
        zw.finish().unwrap();
        buf.into_inner()
    }

    fn make_zip_with_n_files(n: usize) -> Vec<u8> {
        use std::io::Write as _;
        let mut buf = std::io::Cursor::new(Vec::new());
        let mut zw = zip::ZipWriter::new(&mut buf);
        let opts = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        for i in 0..n {
            zw.start_file(format!("f{i}.bin"), opts).unwrap();
            zw.write_all(b"x").unwrap();
        }
        zw.finish().unwrap();
        buf.into_inner()
    }

    #[tokio::test]
    async fn test_hwpx_rejects_zip_bomb_default_limits() {
        let zip_bytes = make_zip_with_ratio(256 * 1024);
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(&zip_bytes, "application/haansofthwpx", &ExtractionConfig::default())
            .await;
        assert!(result.is_err(), "default limits must block a >100:1 zip bomb");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ZIP bomb") || err.contains("ratio") || err.contains("validation"),
            "error should mention bomb/ratio/validation, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_hwpx_rejects_zip_bomb() {
        use crate::extractors::security::SecurityLimits;
        let zip_bytes = make_zip_with_ratio(8 * 1024);
        let config = ExtractionConfig {
            security_limits: Some(SecurityLimits {
                max_compression_ratio: 1,
                ..SecurityLimits::default()
            }),
            ..ExtractionConfig::default()
        };
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(&zip_bytes, "application/haansofthwpx", &config)
            .await;
        assert!(result.is_err(), "zip bomb must be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("ZIP bomb") || err.contains("ratio") || err.contains("validation"),
            "error should mention bomb/ratio/validation, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_hwpx_rejects_oversized_file() {
        use crate::extractors::security::SecurityLimits;
        let limits = SecurityLimits {
            max_archive_size: 10,
            ..SecurityLimits::default()
        };
        let config = ExtractionConfig {
            security_limits: Some(limits),
            ..ExtractionConfig::default()
        };
        let oversized = vec![0u8; 11];
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(&oversized, "application/haansofthwpx", &config)
            .await;
        assert!(result.is_err(), "oversized file must be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("size limit") || err.contains("validation"),
            "error should mention size limit, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_hwpx_rejects_too_many_files() {
        use crate::extractors::security::SecurityLimits;
        let zip_bytes = make_zip_with_n_files(3);
        let config = ExtractionConfig {
            security_limits: Some(SecurityLimits {
                max_files_in_archive: 2,
                ..SecurityLimits::default()
            }),
            ..ExtractionConfig::default()
        };
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(&zip_bytes, "application/haansofthwpx", &config)
            .await;
        assert!(result.is_err(), "archive exceeding file-count limit must be rejected");
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("files") || err.contains("count") || err.contains("validation"),
            "error should mention file count, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_hwpx_valid_zip_passes_security_check() {
        use crate::extractors::security::SecurityLimits;
        let zip_bytes = make_zip_with_ratio(1024);
        let config = ExtractionConfig {
            security_limits: Some(SecurityLimits {
                max_compression_ratio: 10_000,
                max_archive_size: 10 * 1024 * 1024,
                max_files_in_archive: 1_000,
                ..SecurityLimits::default()
            }),
            ..ExtractionConfig::default()
        };
        let extractor = HwpxExtractor::new();
        let result = extractor
            .extract_content(&zip_bytes, "application/haansofthwpx", &config)
            .await;
        let is_parse_err = match &result {
            Err(e) => {
                let msg = e.to_string();
                !msg.contains("ZIP bomb")
                    && !msg.contains("ratio")
                    && !msg.contains("size limit")
                    && !msg.contains("too many files")
            }
            Ok(_) => true,
        };
        assert!(
            is_parse_err,
            "security validator must not reject a safe ZIP; got: {result:?}"
        );
    }
}
