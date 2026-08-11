//! PowerPoint presentation extraction functions.
//!
//! This module provides PowerPoint (PPTX) file parsing by directly reading the Office Open XML
//! format. It extracts text content, slide structure, images, and presentation metadata.
//!
//! # Attribution
//!
//! This code is based on the [pptx-to-md](https://github.com/nilskruthoff/pptx-parser) library
//! by Nils Kruthoff, licensed under MIT OR Apache-2.0. The original code has been vendored and
//! adapted to integrate with Xberg's architecture. See ATTRIBUTIONS.md for full license text.
//!
//! # Features
//!
//! - **Slide extraction**: Reads all slides from presentation
//! - **Text formatting**: Preserves bold, italic, underline formatting as Markdown
//! - **Image extraction**: Optionally extracts embedded images with metadata
//! - **Office metadata**: Extracts core properties, custom properties (when `office` feature enabled)
//! - **Structure preservation**: Maintains heading hierarchy and list structure
//!
//! # Supported Formats
//!
//! - `.pptx` - PowerPoint Presentation
//! - `.pptm` - PowerPoint Macro-Enabled Presentation
//! - `.ppsx` - PowerPoint Slide Show
//!
//! # Example
//!
//! ```ignore
//! use xberg::extraction::pptx::{extract_pptx_from_path, PptxExtractionOptions};
//!
//! # fn example() -> xberg::Result<()> {
//! let result = extract_pptx_from_path("presentation.pptx", &PptxExtractionOptions::default())?;
//!
//! println!("Slide count: {}", result.slide_count);
//! println!("Image count: {}", result.image_count);
//! println!("Content:\n{}", result.content);
//! # Ok(())
//! # }
//! ```

mod comments;
mod container;
mod content_builder;
mod elements;
mod image_handling;
mod metadata;
mod parser;

use ahash::AHashMap;
use bytes::Bytes;

use crate::core::diagnostics::push_warning;
use crate::error::Result;
use crate::types::builder::{self, DocumentStructureBuilder};
use crate::types::document_structure::TextAnnotation;
use crate::types::extraction::BoundingBox;
use crate::types::{ExtractedImage, PptxExtractionResult, ProcessingWarning};

use container::{PptxContainer, SlideIterator};
use content_builder::ContentBuilder;
use elements::{ParserConfig, Run, SlideElement};
use image_handling::detect_image_format;
use metadata::{extract_all_notes, extract_metadata, extract_section_names};

/// Options for PPTX content extraction.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub struct PptxExtractionOptions {
    /// Whether to extract embedded images.
    pub extract_images: bool,
    /// Optional page configuration for boundary tracking.
    pub page_config: Option<crate::core::config::PageConfig>,
    /// Whether to output plain text (no markdown).
    pub plain: bool,
    /// Whether to build the `DocumentStructure` tree.
    pub include_structure: bool,
    /// Whether to emit `![alt](target)` references in markdown output.
    pub inject_placeholders: bool,
}

impl Default for PptxExtractionOptions {
    fn default() -> Self {
        Self {
            extract_images: true,
            page_config: None,
            plain: false,
            include_structure: false,
            inject_placeholders: true,
        }
    }
}

/// Join text runs with smart spacing: inserts a space between adjacent runs
/// only when the previous run doesn't end with whitespace and the next run
/// doesn't start with whitespace.
fn join_runs_with_spacing(runs: &[Run], extract: impl Fn(&Run) -> String) -> String {
    let mut result = String::new();
    for run in runs {
        let text = extract(run);
        if !result.is_empty() && !text.is_empty() {
            let ends_ws = result.ends_with(|c: char| c.is_whitespace());
            let starts_ws = text.starts_with(|c: char| c.is_whitespace());
            if !ends_ws && !starts_ws {
                result.push(' ');
            }
        }
        result.push_str(&text);
    }
    result
}

/// Extract PPTX content from a file path.
///
/// # Arguments
///
/// * `path` - Path to the PPTX file
/// * `options` - Extraction options controlling image extraction, formatting, etc.
///
/// # Returns
///
/// A `PptxExtractionResult` containing extracted content, metadata, and images.
pub(crate) fn extract_pptx_from_path(
    path: &str,
    options: &PptxExtractionOptions,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<PptxExtractionResult> {
    let container = PptxContainer::open(path)?;
    extract_pptx_from_container(container, options, warnings)
}

/// Extract PPTX content from a byte buffer.
///
/// # Arguments
///
/// * `data` - Raw PPTX file bytes
/// * `options` - Extraction options controlling image extraction, formatting, etc.
///
/// # Returns
///
/// A `PptxExtractionResult` containing extracted content, metadata, and images.
pub(crate) fn extract_pptx_from_bytes(
    data: &[u8],
    options: &PptxExtractionOptions,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<PptxExtractionResult> {
    let container = PptxContainer::from_bytes(data)?;
    extract_pptx_from_container(container, options, warnings)
}

fn extract_pptx_from_container<R: std::io::Read + std::io::Seek>(
    mut container: PptxContainer<R>,
    options: &PptxExtractionOptions,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<PptxExtractionResult> {
    let config = ParserConfig {
        extract_images: options.extract_images,
        plain: options.plain,
        inject_placeholders: options.inject_placeholders,
        ..Default::default()
    };
    let page_config = options.page_config.as_ref();
    let include_structure = options.include_structure;

    let (metadata, office_metadata) = extract_metadata(&mut container.archive, warnings);

    let notes = extract_all_notes(&mut container, warnings)?;
    let section_names = extract_section_names(&mut container)?;

    let slide_paths_for_comments = container.slide_paths().to_vec();
    let revisions = comments::extract_comments(&mut container, &slide_paths_for_comments, warnings);

    let mut iterator = SlideIterator::new(container);
    let slide_count = iterator.slide_count();

    let estimated_capacity = slide_count.saturating_mul(1000).max(8192);
    let mut content_builder = ContentBuilder::with_page_config(estimated_capacity, page_config.cloned(), options.plain);

    let mut total_image_count = 0;
    let mut total_table_count = 0;
    let mut extracted_images = Vec::new();
    let mut collected_hyperlinks: Vec<(String, Option<String>)> = Vec::new();
    let mut doc_builder = if include_structure {
        Some(DocumentStructureBuilder::new().source_format("pptx"))
    } else {
        None
    };
    let mut image_index_counter: u32 = 0;

    while let Some(slide) = iterator.next_slide(warnings)? {
        let byte_start = if page_config.is_some() {
            content_builder.start_slide(slide.slide_number)
        } else {
            0
        };

        let slide_content = slide.to_markdown(&config);
        // Preserve the archive-derived slide number for the internal document
        // builder. The marker is consumed before final rendering and is kept out
        // of `slide_content`, so it does not leak into per-slide output.
        content_builder.add_slide_header(slide.slide_number);
        content_builder.add_text(&slide_content);

        let slide_notes = notes.get(&slide.slide_number).cloned();
        if let Some(ref note_text) = slide_notes {
            content_builder.add_notes(note_text);
        }

        let slide_section = section_names.get(&slide.slide_number).cloned();

        if page_config.is_some() {
            content_builder.end_slide(
                slide.slide_number,
                byte_start,
                slide_content.clone(),
                slide_notes,
                slide_section,
            );
        }

        if let Some(ref mut builder) = doc_builder {
            build_slide_structure(&slide, builder, &mut image_index_counter);
        }

        collect_slide_hyperlinks(&slide, &mut collected_hyperlinks);

        if config.extract_images
            && let Ok(image_data) = iterator.get_slide_images(&slide)
        {
            // Pair each image element with its bytes by relationship ID, not by
            // iteration position: `image_data` is a hash map, so its iteration
            // order is unrelated to the document order of `slide.elements`.
            // Indexing into a separately-collected, document-ordered Vec by a
            // hash-map enumeration index silently mismatched dimensions/alt-text
            // with the wrong shape whenever a slide had more than one image (#91).
            for (img_ref, pos) in slide.elements.iter().filter_map(|e| {
                if let SlideElement::Image(img_ref, pos) = e {
                    Some((img_ref, pos))
                } else {
                    None
                }
            }) {
                let Some(data) = image_data.get(&img_ref.id) else {
                    push_warning(
                        warnings,
                        "pptx",
                        format!(
                            "Image '{}' referenced on slide {} could not be read; it was not extracted",
                            img_ref.id, slide.slide_number
                        ),
                    );
                    continue;
                };

                let format = detect_image_format(data);
                let image_index = extracted_images.len();

                let width = if pos.cx > 0 { Some((pos.cx / 9525) as u32) } else { None };
                let height = if pos.cy > 0 { Some((pos.cy / 9525) as u32) } else { None };
                let description = img_ref.description.clone();
                let bbox = position_to_bbox(pos);

                let (image_kind, kind_confidence) =
                    crate::extraction::image_kind::classify(data, format.as_ref(), width, height, None, None, false);

                extracted_images.push(ExtractedImage {
                    data: Bytes::from(data.clone()),
                    format,
                    image_index: image_index as u32,
                    page_number: Some(slide.slide_number),
                    width,
                    height,
                    colorspace: None,
                    bits_per_component: None,
                    is_mask: false,
                    description,
                    ocr_result: None,
                    bounding_box: bbox,
                    source_path: None,
                    image_kind: Some(image_kind),
                    kind_confidence: Some(kind_confidence),
                    cluster_id: None,
                    caption: None,
                    qr_codes: None,
                    data_base64: None,
                });
            }
        }

        total_image_count += slide.image_count();
        total_table_count += slide.table_count();
    }

    let (content, boundaries, mut page_contents) = content_builder.build();

    if let Some(ref mut pcs) = page_contents {
        for pc in pcs.iter_mut() {
            if extracted_images
                .iter()
                .any(|img| img.page_number == Some(pc.page_number))
            {
                pc.is_blank = Some(false);
            }
        }
    }

    let page_structure = boundaries.as_ref().map(|bounds| crate::types::PageStructure {
        total_count: slide_count as u32,
        unit_type: crate::types::PageUnitType::Slide,
        boundaries: Some(bounds.clone()),
        pages: page_contents.as_ref().map(|pcs| {
            pcs.iter()
                .map(|pc| crate::types::PageInfo {
                    number: pc.page_number,
                    title: None,
                    dimensions: None,
                    image_count: None,
                    table_count: None,
                    hidden: None,
                    is_blank: pc.is_blank,
                    has_vector_graphics: false,
                })
                .collect()
        }),
    });

    let document = doc_builder.map(|b| b.build()).filter(|d| !d.is_empty());

    Ok(PptxExtractionResult {
        content,
        metadata,
        slide_count,
        image_count: total_image_count,
        table_count: total_table_count,
        images: extracted_images,
        page_structure,
        page_contents,
        document,
        hyperlinks: collected_hyperlinks,
        office_metadata,
        revisions,
    })
}

/// Build annotations from a sequence of text runs, tracking byte offsets.
///
/// Returns the concatenated plain text and the corresponding annotations.
fn runs_to_text_and_annotations(runs: &[Run]) -> (String, Vec<TextAnnotation>) {
    let mut text = String::new();
    let mut annotations = Vec::new();

    for run in runs {
        let run_text = run.extract();
        if run_text.is_empty() {
            continue;
        }

        if !text.is_empty() {
            let ends_ws = text.ends_with(|c: char| c.is_whitespace());
            let starts_ws = run_text.starts_with(|c: char| c.is_whitespace());
            if !ends_ws && !starts_ws {
                text.push(' ');
            }
        }

        let start = text.len() as u32;
        text.push_str(&run_text);
        let end = text.len() as u32;

        // Math runs carry their content as LaTeX with no bold/italic/etc.
        // formatting to annotate.
        if run.math_latex.is_some() {
            continue;
        }

        if run.formatting.bold {
            annotations.push(builder::bold(start, end));
        }
        if run.formatting.italic {
            annotations.push(builder::italic(start, end));
        }
        if run.formatting.underlined {
            annotations.push(builder::underline(start, end));
        }
        if run.formatting.strikethrough {
            annotations.push(builder::strikethrough(start, end));
        }
        if let Some(sz) = run.formatting.font_size {
            let pts = sz as f64 / 100.0;
            let value = if pts.fract() == 0.0 {
                format!("{}pt", pts as u32)
            } else {
                format!("{:.1}pt", pts)
            };
            annotations.push(builder::font_size(start, end, &value));
        }
    }

    (text, annotations)
}

/// Convert an `ElementPosition` with dimensions to a `BoundingBox`.
///
/// EMU coordinates are converted to points (1 inch = 914400 EMU = 72 pt).
fn position_to_bbox(pos: &elements::ElementPosition) -> Option<BoundingBox> {
    if pos.x == 0 && pos.y == 0 && pos.cx == 0 && pos.cy == 0 {
        return None;
    }
    const EMU_PER_PT: f64 = 914_400.0 / 72.0;
    Some(BoundingBox {
        x0: pos.x as f64 / EMU_PER_PT,
        y0: pos.y as f64 / EMU_PER_PT,
        x1: (pos.x + pos.cx) as f64 / EMU_PER_PT,
        y1: (pos.y + pos.cy) as f64 / EMU_PER_PT,
    })
}

/// Populate the document structure builder for a single slide.
fn build_slide_structure(
    slide: &elements::Slide,
    doc_builder: &mut DocumentStructureBuilder,
    image_index_counter: &mut u32,
) {
    let mut sorted_indices: Vec<usize> = (0..slide.elements.len()).collect();
    sorted_indices.sort_by_key(|&i| {
        let pos = slide.elements[i].position();
        (pos.y, pos.x)
    });

    let slide_title = sorted_indices
        .iter()
        .find_map(|&idx| {
            if let SlideElement::Text(text, _) = &slide.elements[idx]
                && text.is_title
            {
                let plain = join_runs_with_spacing(&text.runs, Run::extract);
                if !plain.trim().is_empty() {
                    return Some(plain.trim().to_string());
                }
            }
            None
        })
        .or_else(|| {
            sorted_indices.iter().find_map(|&idx| {
                if let SlideElement::Text(text, _) = &slide.elements[idx] {
                    let plain = join_runs_with_spacing(&text.runs, Run::extract);
                    let normalized = plain.replace('\n', " ");
                    if normalized.len() < 100 && !normalized.trim().is_empty() {
                        return Some(normalized.trim().to_string());
                    }
                }
                None
            })
        });

    doc_builder.push_slide(slide.slide_number, slide_title.as_deref());

    let mut first_title_seen = false;

    for &idx in &sorted_indices {
        let elem = &slide.elements[idx];
        let pos = elem.position();
        let bbox = position_to_bbox(&pos);

        match elem {
            SlideElement::Text(text, _) => {
                let (plain_text, annotations) = runs_to_text_and_annotations(&text.runs);
                let normalized = plain_text.replace('\n', " ");
                let is_title_elem = text.is_title || (normalized.len() < 100 && !normalized.trim().is_empty());

                if is_title_elem && !first_title_seen {
                    first_title_seen = true;
                    doc_builder.push_heading(1, normalized.trim(), None, bbox);
                } else if !plain_text.trim().is_empty() {
                    let node_idx = doc_builder.push_paragraph(&plain_text, annotations, None, bbox);

                    if let Some(lang) = text.runs.iter().find_map(|r| {
                        let l = &r.formatting.lang;
                        if l.is_empty() { None } else { Some(l.clone()) }
                    }) {
                        let mut attrs = AHashMap::new();
                        attrs.insert("lang".to_string(), lang);
                        doc_builder.set_attributes(node_idx, attrs);
                    }
                }
            }
            SlideElement::Table(table, _) => {
                let cells: Vec<Vec<String>> = table
                    .rows
                    .iter()
                    .map(|row| {
                        row.cells
                            .iter()
                            .map(|cell| join_runs_with_spacing(&cell.runs, Run::extract))
                            .collect()
                    })
                    .collect();
                if !cells.is_empty() {
                    doc_builder.push_table_from_cells(&cells, None);
                }
            }
            SlideElement::List(list, _) => {
                if !list.items.is_empty() {
                    let is_ordered = list.items.first().is_some_and(|item| item.is_ordered);
                    let list_node = doc_builder.push_list(is_ordered, None);
                    for item in &list.items {
                        let item_text = join_runs_with_spacing(&item.runs, Run::extract);
                        if !item_text.trim().is_empty() {
                            doc_builder.push_list_item(list_node, item_text.trim(), None);
                        }
                    }
                }
            }
            SlideElement::Image(img_ref, _) => {
                let desc = img_ref.description.as_deref().or({
                    if img_ref.target.is_empty() {
                        None
                    } else {
                        Some(img_ref.target.as_str())
                    }
                });
                doc_builder.push_image(desc, Some(*image_index_counter), None, bbox);
                *image_index_counter += 1;
            }
            SlideElement::Chart(chart_ref, _) => {
                if let Some(text) = chart_ref.resolved_text.as_deref()
                    && !text.trim().is_empty()
                {
                    doc_builder.push_paragraph(text, vec![], None, bbox);
                }
            }
            SlideElement::SmartArt(diagram_ref, _) => {
                if let Some(text) = diagram_ref.resolved_text.as_deref()
                    && !text.trim().is_empty()
                {
                    doc_builder.push_paragraph(text, vec![], None, bbox);
                }
            }
            SlideElement::Unknown => {}
        }
    }

    doc_builder.exit_container();
}

/// Collect hyperlinks from all runs in a slide by resolving `hlinkClick` rIds
/// against the slide's hyperlink relationships.
fn collect_slide_hyperlinks(slide: &elements::Slide, out: &mut Vec<(String, Option<String>)>) {
    let mut visit_runs = |runs: &[Run]| {
        for run in runs {
            if let Some(ref hlink_id) = run.hyperlink_id
                && let Some(href) = slide.hyperlinks.iter().find(|h| h.id == *hlink_id)
            {
                let label = if run.text.trim().is_empty() {
                    None
                } else {
                    Some(run.text.trim().to_string())
                };
                out.push((href.url.clone(), label));
            }
        }
    };

    for elem in &slide.elements {
        match elem {
            SlideElement::Text(text, _) => visit_runs(&text.runs),
            SlideElement::List(list, _) => {
                for item in &list.items {
                    visit_runs(&item.runs);
                }
            }
            SlideElement::Table(table, _) => {
                for row in &table.rows {
                    for cell in &row.cells {
                        visit_runs(&cell.runs);
                    }
                }
            }
            _ => {}
        }
    }
}

impl elements::Slide {
    fn from_xml(slide_number: u32, xml_data: &[u8], rels_data: Option<&[u8]>) -> Result<Self> {
        let elements = parser::parse_slide_xml(xml_data)?;

        let (images, hyperlinks, rel_targets) = if let Some(rels) = rels_data {
            let slide_rels = parser::parse_slide_rels(rels)?;
            (slide_rels.images, slide_rels.hyperlinks, slide_rels.targets)
        } else {
            (Vec::new(), Vec::new(), AHashMap::new())
        };

        Ok(Self {
            slide_number,
            elements,
            images,
            hyperlinks,
            rel_targets,
        })
    }

    fn to_markdown(&self, config: &ParserConfig) -> String {
        let mut builder = ContentBuilder::new(config.plain);

        if config.include_slide_comment {
            builder.add_slide_header(self.slide_number);
        }

        let mut element_indices: Vec<usize> = (0..self.elements.len()).collect();
        element_indices.sort_by_key(|&i| {
            let pos = self.elements[i].position();
            (pos.y, pos.x)
        });

        let title_idx = element_indices
            .iter()
            .find_map(|&idx| {
                if let SlideElement::Text(text, _) = &self.elements[idx]
                    && text.is_title
                {
                    let plain = join_runs_with_spacing(&text.runs, Run::extract);
                    if !plain.trim().is_empty() {
                        return Some(idx);
                    }
                }
                None
            })
            .or_else(|| {
                element_indices.iter().find_map(|&idx| {
                    if let SlideElement::Text(text, _) = &self.elements[idx] {
                        let plain = join_runs_with_spacing(&text.runs, Run::extract);
                        let normalized = plain.replace('\n', " ");
                        if normalized.len() < 100 && !normalized.trim().is_empty() {
                            return Some(idx);
                        }
                    }
                    None
                })
            });

        if let Some(tidx) = title_idx
            && let SlideElement::Text(text, _) = &self.elements[tidx]
        {
            let text_content: String = if config.plain {
                join_runs_with_spacing(&text.runs, Run::extract)
            } else {
                join_runs_with_spacing(&text.runs, Run::render_as_md)
            };
            let normalized = text_content.replace('\n', " ");
            builder.add_title(normalized.trim());
        }

        for &idx in &element_indices {
            if Some(idx) == title_idx {
                continue;
            }

            match &self.elements[idx] {
                SlideElement::Text(text, _) => {
                    let text_content: String = if config.plain {
                        join_runs_with_spacing(&text.runs, Run::extract)
                    } else {
                        join_runs_with_spacing(&text.runs, Run::render_as_md)
                    };

                    builder.add_text(&text_content);
                }
                SlideElement::Table(table, _) => {
                    let extract_fn: fn(&Run) -> String = if config.plain { Run::extract } else { Run::render_as_md };
                    let table_rows: Vec<Vec<String>> = table
                        .rows
                        .iter()
                        .map(|row| {
                            row.cells
                                .iter()
                                .map(|cell| join_runs_with_spacing(&cell.runs, extract_fn))
                                .collect()
                        })
                        .collect();
                    builder.add_table(&table_rows);
                }
                SlideElement::List(list, _) => {
                    let extract_fn: fn(&Run) -> String = if config.plain { Run::extract } else { Run::render_as_md };
                    for item in &list.items {
                        let item_text = join_runs_with_spacing(&item.runs, extract_fn);
                        if item.has_bullet {
                            builder.add_list_item(item.level, item.is_ordered, &item_text);
                        } else {
                            builder.add_text(&item_text);
                        }
                    }
                }
                SlideElement::Image(img_ref, _) => {
                    if config.inject_placeholders {
                        let target = self
                            .images
                            .iter()
                            .find(|rel| rel.id == img_ref.id)
                            .map(|rel| rel.target.as_str())
                            .unwrap_or("");
                        builder.add_image_with_desc(&img_ref.id, img_ref.description.as_deref(), target);
                    }
                }
                SlideElement::Chart(chart_ref, _) => {
                    if let Some(text) = chart_ref.resolved_text.as_deref() {
                        builder.add_text(text);
                    }
                }
                SlideElement::SmartArt(diagram_ref, _) => {
                    if let Some(text) = diagram_ref.resolved_text.as_deref() {
                        builder.add_text(text);
                    }
                }
                SlideElement::Unknown => {}
            }
        }

        builder.build().0
    }

    fn image_count(&self) -> usize {
        self.elements
            .iter()
            .filter(|e| matches!(e, SlideElement::Image(_, _)))
            .count()
    }

    fn table_count(&self) -> usize {
        self.elements
            .iter()
            .filter(|e| matches!(e, SlideElement::Table(_, _)))
            .count()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn create_test_pptx_bytes(slides: Vec<&str>) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
</Types>"#,
            )
            .unwrap();

            zip.start_file("ppt/presentation.xml", options).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?><presentation/>").unwrap();

            zip.start_file("_rels/.rels", options).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#).unwrap();

            let mut rels_xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
            );
            for (i, _) in slides.iter().enumerate() {
                use std::fmt::Write;
                let _ = write!(
                    rels_xml,
                    r#"<Relationship Id="rId{}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{}.xml"/>"#,
                    i + 1,
                    i + 1
                );
            }
            rels_xml.push_str("</Relationships>");
            zip.start_file("ppt/_rels/presentation.xml.rels", options).unwrap();
            zip.write_all(rels_xml.as_bytes()).unwrap();

            for (i, text) in slides.iter().enumerate() {
                let slide_xml = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld>
        <p:spTree>
            <p:sp>
                <p:txBody>
                    <a:p>
                        <a:r>
                            <a:t>{}</a:t>
                        </a:r>
                    </a:p>
                </p:txBody>
            </p:sp>
        </p:spTree>
    </p:cSld>
</p:sld>"#,
                    text
                );
                zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), options)
                    .unwrap();
                zip.write_all(slide_xml.as_bytes()).unwrap();
            }

            zip.start_file("docProps/core.xml", options).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/"
                   xmlns:dcterms="http://purl.org/dc/terms/">
    <dc:title>Test Presentation</dc:title>
    <dc:creator>Test Author</dc:creator>
    <dc:description>Test Description</dc:description>
    <dc:subject>Test Subject</dc:subject>
</cp:coreProperties>"#,
            )
            .unwrap();

            let app_xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"
            xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes">
    <Slides>{}</Slides>
    <Application>Microsoft Office PowerPoint</Application>
</Properties>"#,
                slides.len()
            );
            zip.start_file("docProps/app.xml", options).unwrap();
            zip.write_all(app_xml.as_bytes()).unwrap();

            let _ = zip.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn test_extract_pptx_from_bytes_single_slide() {
        let pptx_bytes = create_test_pptx_bytes(vec!["Hello World"]);
        let result = extract_pptx_from_bytes(
            &pptx_bytes,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(result.slide_count, 1);
        assert!(
            result.content.contains("Hello World"),
            "Content was: {}",
            result.content
        );
        assert_eq!(result.image_count, 0);
        assert_eq!(result.table_count, 0);
    }

    #[test]
    fn test_extract_pptx_from_bytes_multiple_slides() {
        let pptx_bytes = create_test_pptx_bytes(vec!["Slide 1", "Slide 2", "Slide 3"]);
        let result = extract_pptx_from_bytes(
            &pptx_bytes,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(result.slide_count, 3);
        assert!(result.content.contains("Slide 1"));
        assert!(result.content.contains("Slide 2"));
        assert!(result.content.contains("Slide 3"));
        for slide_number in 1..=3 {
            assert!(
                result
                    .content
                    .contains(&format!("<!-- Slide number: {slide_number} -->")),
                "combined content should preserve the boundary for slide {slide_number}"
            );
        }
    }

    #[test]
    fn test_extract_pptx_metadata() {
        let pptx_bytes = create_test_pptx_bytes(vec!["Content"]);
        let result = extract_pptx_from_bytes(
            &pptx_bytes,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(result.metadata.slide_count, 1);
    }

    #[test]
    fn test_extract_pptx_empty_slides() {
        let pptx_bytes = create_test_pptx_bytes(vec!["", "", ""]);
        let result = extract_pptx_from_bytes(
            &pptx_bytes,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert_eq!(result.slide_count, 3);
    }

    #[test]
    fn test_extract_pptx_from_bytes_invalid_data() {
        use crate::error::XbergError;

        let invalid_bytes = b"not a valid pptx file";
        let result = extract_pptx_from_bytes(
            invalid_bytes,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        );

        assert!(result.is_err());
        if let Err(XbergError::Parsing { message: msg, .. }) = result {
            assert!(msg.contains("Failed to read PPTX archive") || msg.contains("Failed to write temp PPTX file"));
        } else {
            panic!("Expected ParsingError");
        }
    }

    #[test]
    fn test_extract_pptx_from_bytes_empty_data() {
        let empty_bytes: &[u8] = &[];
        let result = extract_pptx_from_bytes(
            empty_bytes,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        );

        assert!(result.is_err());
    }

    /// Build a PPTX bytes with sections and optional speaker notes for integration testing.
    pub(crate) fn create_pptx_with_sections_and_notes(
        slides: &[(&str, Option<&str>)],
        sections: &[(&str, &[usize])],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let opts = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
</Types>"#,
        )
        .unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#).unwrap();

        let mut pres_rels = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (i, _) in slides.iter().enumerate() {
            use std::fmt::Write as FmtWrite;
            let _ = write!(
                pres_rels,
                r#"<Relationship Id="rId{id}" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide{id}.xml"/>"#,
                id = i + 1
            );
        }
        pres_rels.push_str("</Relationships>");
        zip.start_file("ppt/_rels/presentation.xml.rels", opts).unwrap();
        zip.write_all(pres_rels.as_bytes()).unwrap();

        let base_id: u32 = 256;
        let mut sld_id_lst = String::new();
        for (i, _) in slides.iter().enumerate() {
            use std::fmt::Write as FmtWrite;
            let _ = write!(
                sld_id_lst,
                r#"<p:sldId id="{}" r:id="rId{}"/>"#,
                base_id + i as u32,
                i + 1
            );
        }

        let mut section_lst = String::new();
        for (name, positions) in sections {
            use std::fmt::Write as FmtWrite;
            let mut sld_ids = String::new();
            for &pos in *positions {
                let _ = write!(sld_ids, r#"<p14:sectionSldId id="{}"/>"#, base_id + (pos as u32 - 1));
            }
            let _ = write!(
                section_lst,
                r#"<p14:section name="{}"><p14:sectionSldIdLst>{}</p14:sectionSldIdLst></p14:section>"#,
                name, sld_ids
            );
        }

        let ext_lst = if section_lst.is_empty() {
            String::new()
        } else {
            format!(
                r#"<p:extLst><p:ext uri="{{521415D9-36F7-43E2-AB2F-B90AF26B5E84}}"><p14:sectionLst xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main">{}</p14:sectionLst></p:ext></p:extLst>"#,
                section_lst
            )
        };

        let presentation_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
                xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
  <p:sldIdLst>{}</p:sldIdLst>
  {}
</p:presentation>"#,
            sld_id_lst, ext_lst
        );
        zip.start_file("ppt/presentation.xml", opts).unwrap();
        zip.write_all(presentation_xml.as_bytes()).unwrap();

        for (i, (text, notes)) in slides.iter().enumerate() {
            let slide_xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#,
                text
            );
            zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), opts).unwrap();
            zip.write_all(slide_xml.as_bytes()).unwrap();

            if let Some(note_text) = notes {
                let notes_xml = format!(
                    r#"<?xml version="1.0" encoding="UTF-8"?>
<p:notes xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
         xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld>
</p:notes>"#,
                    note_text
                );
                zip.start_file(format!("ppt/notesSlides/notesSlide{}.xml", i + 1), opts)
                    .unwrap();
                zip.write_all(notes_xml.as_bytes()).unwrap();
            }
        }

        zip.start_file("docProps/core.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Test</dc:title></cp:coreProperties>"#,
        )
        .unwrap();

        let app_xml = format!(
            r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Slides>{}</Slides></Properties>"#,
            slides.len()
        );
        zip.start_file("docProps/app.xml", opts).unwrap();
        zip.write_all(app_xml.as_bytes()).unwrap();

        let _ = zip.finish().unwrap();
        buffer
    }

    #[test]
    fn test_speaker_notes_in_page_contents() {
        use crate::core::config::PageConfig;

        let pptx = create_pptx_with_sections_and_notes(
            &[
                ("Intro Slide", Some("These are intro notes.")),
                ("Content Slide", None),
                ("Outro Slide", Some("Final notes here.")),
            ],
            &[],
        );

        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                page_config: Some(PageConfig::default()),
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        let pages = result
            .page_contents
            .as_ref()
            .expect("page_contents should be populated");
        assert_eq!(pages.len(), 3);

        assert_eq!(pages[0].speaker_notes.as_deref(), Some("These are intro notes."));
        assert!(pages[1].speaker_notes.is_none(), "slide 2 has no notes");
        assert_eq!(pages[2].speaker_notes.as_deref(), Some("Final notes here."));
    }

    #[test]
    fn test_section_name_in_page_contents() {
        use crate::core::config::PageConfig;

        let pptx = create_pptx_with_sections_and_notes(
            &[("Slide 1", None), ("Slide 2", None), ("Slide 3", None)],
            &[("Introduction", &[1]), ("Deep Dive", &[2, 3])],
        );

        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                page_config: Some(PageConfig::default()),
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        let pages = result
            .page_contents
            .as_ref()
            .expect("page_contents should be populated");
        assert_eq!(pages.len(), 3);

        assert_eq!(pages[0].section_name.as_deref(), Some("Introduction"));
        assert_eq!(pages[1].section_name.as_deref(), Some("Deep Dive"));
        assert_eq!(pages[2].section_name.as_deref(), Some("Deep Dive"));
    }

    #[test]
    fn test_section_name_and_notes_combined() {
        use crate::core::config::PageConfig;

        let pptx = create_pptx_with_sections_and_notes(
            &[
                ("Title Slide", Some("Welcome notes.")),
                ("Methods", Some("Methodology notes.")),
            ],
            &[("Part One", &[1, 2])],
        );

        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                page_config: Some(PageConfig::default()),
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        let pages = result
            .page_contents
            .as_ref()
            .expect("page_contents should be populated");
        assert_eq!(pages[0].section_name.as_deref(), Some("Part One"));
        assert_eq!(pages[0].speaker_notes.as_deref(), Some("Welcome notes."));
        assert_eq!(pages[1].section_name.as_deref(), Some("Part One"));
        assert_eq!(pages[1].speaker_notes.as_deref(), Some("Methodology notes."));
    }

    #[test]
    fn test_no_page_contents_without_page_config() {
        let pptx = create_pptx_with_sections_and_notes(&[("Slide 1", Some("Notes."))], &[("Section A", &[1])]);

        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert!(
            result.page_contents.is_none(),
            "page_contents should be None when page_config is not set"
        );
    }

    #[test]
    fn test_detect_image_format_jpeg() {
        let jpeg_header = vec![0xFF, 0xD8, 0xFF, 0xE0];
        assert_eq!(detect_image_format(&jpeg_header), "jpeg");
    }

    #[test]
    fn test_detect_image_format_png() {
        let png_header = vec![0x89, 0x50, 0x4E, 0x47];
        assert_eq!(detect_image_format(&png_header), "png");
    }

    #[test]
    fn test_detect_image_format_gif() {
        let gif_header = b"GIF89a";
        assert_eq!(detect_image_format(gif_header), "gif");
    }

    #[test]
    fn test_detect_image_format_bmp() {
        let bmp_header = b"BM";
        assert_eq!(detect_image_format(bmp_header), "bmp");
    }

    #[test]
    fn test_detect_image_format_svg() {
        let svg_header = b"<svg xmlns=\"http://www.w3.org/2000/svg\">";
        assert_eq!(detect_image_format(svg_header), "svg");
    }

    #[test]
    fn test_detect_image_format_tiff_little_endian() {
        let tiff_header = vec![0x49, 0x49, 0x2A, 0x00];
        assert_eq!(detect_image_format(&tiff_header), "tiff");
    }

    #[test]
    fn test_detect_image_format_tiff_big_endian() {
        let tiff_header = vec![0x4D, 0x4D, 0x00, 0x2A];
        assert_eq!(detect_image_format(&tiff_header), "tiff");
    }

    #[test]
    fn test_detect_image_format_unknown() {
        let unknown_data = b"unknown format";
        assert_eq!(detect_image_format(unknown_data), "unknown");
    }

    #[test]
    fn test_get_slide_rels_path() {
        assert_eq!(
            image_handling::get_slide_rels_path("ppt/slides/slide1.xml"),
            "ppt/slides/_rels/slide1.xml.rels"
        );
        assert_eq!(
            image_handling::get_slide_rels_path("ppt/slides/slide10.xml"),
            "ppt/slides/_rels/slide10.xml.rels"
        );
    }

    #[test]
    fn test_get_full_image_path_relative() {
        assert_eq!(
            image_handling::get_full_image_path("ppt/slides/slide1.xml", "../media/image1.png"),
            "ppt/media/image1.png"
        );
    }

    #[test]
    fn test_get_full_image_path_direct() {
        assert_eq!(
            image_handling::get_full_image_path("ppt/slides/slide1.xml", "image1.png"),
            "ppt/slides/image1.png"
        );
    }

    /// Build a minimal PPTX ZIP with one slide that contains an image (<p:pic>) element.
    ///
    /// The slide XML includes a picture shape referencing rel id "rId2", and the
    /// slide rels file maps that id to "media/image1.png".
    fn create_pptx_with_image_slide(slide_text: &str) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        {
            let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let opts = SimpleFileOptions::default();

            zip.start_file("[Content_Types].xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
    <Default Extension="png" ContentType="image/png"/>
</Types>"#,
            )
            .unwrap();

            zip.start_file("_rels/.rels", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#).unwrap();

            zip.start_file("ppt/presentation.xml", opts).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?><presentation/>").unwrap();

            zip.start_file("ppt/_rels/presentation.xml.rels", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#).unwrap();

            let slide_xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
    <p:cSld>
        <p:spTree>
            <p:sp>
                <p:txBody>
                    <a:p><a:r><a:t>{slide_text}</a:t></a:r></a:p>
                </p:txBody>
            </p:sp>
            <p:pic>
                <p:nvPicPr>
                    <p:nvPr><p:ph type="pic"/></p:nvPr>
                    <p:cNvPicPr/>
                    <p:nvPr><a:hlinkClick r:id=""/></p:nvPr>
                </p:nvPicPr>
                <p:blipFill>
                    <a:blip r:embed="rId2" descr="Test chart"/>
                </p:blipFill>
                <p:spPr>
                    <a:xfrm><a:off x="0" y="0"/><a:ext cx="1000000" cy="1000000"/></a:xfrm>
                </p:spPr>
            </p:pic>
        </p:spTree>
    </p:cSld>
</p:sld>"#
            );
            zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
            zip.write_all(slide_xml.as_bytes()).unwrap();

            zip.start_file("ppt/slides/_rels/slide1.xml.rels", opts).unwrap();
            zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
</Relationships>"#).unwrap();

            zip.start_file("ppt/media/image1.png", opts).unwrap();
            zip.write_all(b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82").unwrap();

            zip.start_file("docProps/core.xml", opts).unwrap();
            zip.write_all(
                br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Image Test</dc:title>
</cp:coreProperties>"#,
            )
            .unwrap();

            zip.start_file("docProps/app.xml", opts).unwrap();
            zip.write_all(b"<?xml version=\"1.0\"?><Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Slides>1</Slides></Properties>").unwrap();

            let _ = zip.finish().unwrap();
        }
        buffer
    }

    #[test]
    fn test_inject_placeholders_true_emits_image_reference() {
        let pptx = create_pptx_with_image_slide("Hello");
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            result.content.contains("!["),
            "inject_placeholders=true must emit image reference, got: {:?}",
            result.content
        );
    }

    #[test]
    fn test_inject_placeholders_false_suppresses_image_reference() {
        let pptx = create_pptx_with_image_slide("Hello");
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                inject_placeholders: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            !result.content.contains("!["),
            "inject_placeholders=false must NOT emit image reference, got: {:?}",
            result.content
        );
    }

    #[test]
    fn test_inject_placeholders_false_preserves_text_content() {
        let pptx = create_pptx_with_image_slide("Quarterly Review");
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                inject_placeholders: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            result.content.contains("Quarterly Review"),
            "Text content must survive when inject_placeholders=false, got: {:?}",
            result.content
        );
    }

    #[test]
    fn test_default_inject_placeholders_preserves_existing_behaviour() {
        let pptx = create_pptx_with_image_slide("Slide Title");
        let result_true = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
        let result_false = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                inject_placeholders: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            result_true.content.len() >= result_false.content.len(),
            "inject_placeholders=true should produce >= content length vs false"
        );
    }

    /// Build a minimal PPTX with optional comment XML parts.
    ///
    /// `slides` is a list of slide text strings.
    /// `comments_per_slide` is indexed by slide (0-based); each entry is a list
    /// of `(idx, author_id, datetime, comment_text)` tuples.
    /// `authors` is a list of `(id, name)` tuples written to `commentAuthors.xml`.
    fn create_pptx_with_comments(
        slides: &[&str],
        comments_per_slide: &[Vec<(u32, u32, &str, &str)>],
        authors: &[(u32, &str)],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let opts = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="rels"
      ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
</Types>"#,
        )
        .unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1"
      Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument"
      Target="ppt/presentation.xml"/>
</Relationships>"#,
        )
        .unwrap();

        let mut rels = String::from(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">"#,
        );
        for (i, _) in slides.iter().enumerate() {
            use std::fmt::Write as FmtWrite;
            let _ = write!(
                rels,
                r#"<Relationship Id="rId{id}"
  Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide"
  Target="slides/slide{id}.xml"/>"#,
                id = i + 1
            );
        }
        rels.push_str("</Relationships>");
        zip.start_file("ppt/_rels/presentation.xml.rels", opts).unwrap();
        zip.write_all(rels.as_bytes()).unwrap();

        zip.start_file("ppt/presentation.xml", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:sldIdLst/></p:presentation>"#).unwrap();

        for (i, text) in slides.iter().enumerate() {
            let slide_xml = format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld><p:spTree><p:sp><p:txBody>
        <a:p><a:r><a:t>{text}</a:t></a:r></a:p>
    </p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#
            );
            zip.start_file(format!("ppt/slides/slide{}.xml", i + 1), opts).unwrap();
            zip.write_all(slide_xml.as_bytes()).unwrap();
        }

        if !authors.is_empty() {
            let mut authors_xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<p:cmAuthorLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">"#,
            );
            for (id, name) in authors {
                use std::fmt::Write as FmtWrite;
                let _ = write!(
                    authors_xml,
                    r#"<p:cmAuthor id="{id}" name="{name}" initials="A" lastIdx="0" clrIdx="0"/>"#
                );
            }
            authors_xml.push_str("</p:cmAuthorLst>");
            zip.start_file("ppt/commentAuthors.xml", opts).unwrap();
            zip.write_all(authors_xml.as_bytes()).unwrap();
        }

        for (slide_idx, slide_comments) in comments_per_slide.iter().enumerate() {
            if slide_comments.is_empty() {
                continue;
            }
            let mut cm_xml = String::from(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<p:cmLst xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">"#,
            );
            for (idx, author_id, dt, text) in slide_comments {
                use std::fmt::Write as FmtWrite;
                let _ = write!(
                    cm_xml,
                    r#"<p:cm authorId="{author_id}" dt="{dt}" idx="{idx}"><p:text>{text}</p:text></p:cm>"#
                );
            }
            cm_xml.push_str("</p:cmLst>");
            zip.start_file(format!("ppt/comments/comment{}.xml", slide_idx + 1), opts)
                .unwrap();
            zip.write_all(cm_xml.as_bytes()).unwrap();
        }

        zip.start_file("docProps/core.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Test</dc:title></cp:coreProperties>"#,
        )
        .unwrap();
        zip.start_file("docProps/app.xml", opts).unwrap();
        let app = format!(
            r#"<?xml version="1.0"?><Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties"><Slides>{}</Slides></Properties>"#,
            slides.len()
        );
        zip.write_all(app.as_bytes()).unwrap();

        let _ = zip.finish().unwrap();
        buffer
    }

    #[test]
    fn should_return_none_revisions_when_no_comment_files_exist() {
        let pptx = create_test_pptx_bytes(vec!["Slide with no comments"]);
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();
        assert!(
            result.revisions.is_none(),
            "revisions should be None when no ppt/comments/ files exist"
        );
    }

    #[test]
    fn should_surface_single_comment_as_revision_with_correct_fields() {
        let pptx = create_pptx_with_comments(
            &["Slide One"],
            &[vec![(1, 0, "2024-03-15T10:30:00Z", "Please revise this slide")]],
            &[(0, "Alice")],
        );
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        let revisions = result
            .revisions
            .as_ref()
            .expect("revisions should be Some when comment files exist");
        assert_eq!(revisions.len(), 1, "expected 1 revision for 1 comment");

        let rev = &revisions[0];
        assert_eq!(rev.revision_id, "1");
        assert_eq!(rev.author.as_deref(), Some("Alice"));
        assert_eq!(rev.timestamp.as_deref(), Some("2024-03-15T10:30:00Z"));
        use crate::types::revisions::{DiffLine, RevisionAnchor, RevisionKind};
        assert!(matches!(rev.kind, RevisionKind::Comment));
        assert!(
            matches!(&rev.anchor, Some(RevisionAnchor::Slide { index: 0 })),
            "slide anchor should be 0 (0-indexed) for the first slide"
        );
        assert_eq!(rev.delta.content.len(), 1);
        assert!(matches!(&rev.delta.content[0], DiffLine::Context(t) if t == "Please revise this slide"));
    }

    #[test]
    fn should_surface_comments_from_multiple_slides_with_correct_anchors() {
        let pptx = create_pptx_with_comments(
            &["Slide 1", "Slide 2", "Slide 3"],
            &[
                vec![(1, 0, "2024-03-15T09:00:00Z", "Comment on slide 1")],
                vec![],
                vec![
                    (1, 0, "2024-03-15T11:00:00Z", "Comment A on slide 3"),
                    (2, 1, "2024-03-15T11:05:00Z", "Comment B on slide 3"),
                ],
            ],
            &[(0, "Alice"), (1, "Bob")],
        );
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        let revisions = result.revisions.as_ref().expect("revisions should be Some");
        assert_eq!(revisions.len(), 3);

        use crate::types::revisions::RevisionAnchor;
        assert!(matches!(&revisions[0].anchor, Some(RevisionAnchor::Slide { index: 0 })));
        assert!(matches!(&revisions[1].anchor, Some(RevisionAnchor::Slide { index: 2 })));
        assert!(matches!(&revisions[2].anchor, Some(RevisionAnchor::Slide { index: 2 })));
        assert_eq!(revisions[1].author.as_deref(), Some("Alice"));
        assert_eq!(revisions[2].author.as_deref(), Some("Bob"));
    }

    // --- Regression tests for #47, #79, #80, #90, #91, #238 ---
    //
    // Shared helper used by 5 of the 6 tests below (`build_single_slide_pptx`);
    // #238 builds its own bytes since it needs corrupt, not merely custom,
    // `docProps/core.xml`/comment parts. ~keep

    /// Build a minimal single-slide PPTX with caller-supplied slide XML,
    /// optional slide relationships XML, and optional extra ZIP parts (used
    /// for chart/SmartArt data parts and referenced media).
    fn build_single_slide_pptx(
        slide_xml: &str,
        slide_rels_xml: Option<&str>,
        extra_parts: &[(&str, &[u8])],
    ) -> Vec<u8> {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let opts = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
    <Default Extension="png" ContentType="image/png"/>
</Types>"#,
        )
        .unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#).unwrap();

        zip.start_file("ppt/presentation.xml", opts).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><presentation/>").unwrap();

        zip.start_file("ppt/_rels/presentation.xml.rels", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#).unwrap();

        zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
        zip.write_all(slide_xml.as_bytes()).unwrap();

        if let Some(rels) = slide_rels_xml {
            zip.start_file("ppt/slides/_rels/slide1.xml.rels", opts).unwrap();
            zip.write_all(rels.as_bytes()).unwrap();
        }

        zip.start_file("docProps/core.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties"
                   xmlns:dc="http://purl.org/dc/elements/1.1/"><dc:title>Test</dc:title></cp:coreProperties>"#,
        )
        .unwrap();

        zip.start_file("docProps/app.xml", opts).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Slides>1</Slides></Properties>").unwrap();

        for (path, data) in extra_parts {
            zip.start_file(*path, opts).unwrap();
            zip.write_all(data).unwrap();
        }

        let _ = zip.finish().unwrap();
        buffer
    }

    /// #47: OMML math wrapped in `mc:AlternateContent`/`mc:Choice`/`a14:m` must be
    /// converted to LaTeX via the shared `docx::math` OMML converter, not dropped,
    /// and the `mc:Fallback` text must not also appear (Choice content wins).
    #[test]
    fn test_issue_47_omml_math_wired_into_pptx() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
       xmlns:a14="http://schemas.microsoft.com/office/drawing/2010/main"
       xmlns:m="http://schemas.openxmlformats.org/officeDocument/2006/math">
    <p:cSld><p:spTree><p:sp><p:txBody>
        <a:p>
            <mc:AlternateContent>
                <mc:Choice Requires="a14">
                    <a14:m>
                        <m:oMathPara>
                            <m:oMath>
                                <m:sSup>
                                    <m:e><m:r><m:t>x</m:t></m:r></m:e>
                                    <m:sup><m:r><m:t>2</m:t></m:r></m:sup>
                                </m:sSup>
                            </m:oMath>
                        </m:oMathPara>
                    </a14:m>
                </mc:Choice>
                <mc:Fallback>
                    <a:r><a:t>[equation]</a:t></a:r>
                </mc:Fallback>
            </mc:AlternateContent>
        </a:p>
    </p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#;

        let pptx = build_single_slide_pptx(slide_xml, None, &[]);
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert!(
            result.content.contains("$$x^{2}$$"),
            "expected OMML math rendered as display LaTeX, got: {:?}",
            result.content
        );
        assert!(
            !result.content.contains("[equation]"),
            "mc:Choice content succeeded, so mc:Fallback text must not also appear, got: {:?}",
            result.content
        );
    }

    /// #79: `mc:AlternateContent` wrapping a whole shape must fall back to
    /// `mc:Fallback` when `mc:Choice` carries no PresentationML-recognized
    /// content, and `p:cxnSp` connectors must contribute their text.
    #[test]
    fn test_issue_79_alternate_content_fallback_and_connector_text() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:mc="http://schemas.openxmlformats.org/markup-compatibility/2006"
       xmlns:p14="http://schemas.microsoft.com/office/powerpoint/2010/main">
    <p:cSld><p:spTree>
        <mc:AlternateContent>
            <mc:Choice Requires="p159">
                <p14:futureShape/>
            </mc:Choice>
            <mc:Fallback>
                <p:sp>
                    <p:txBody><a:p><a:r><a:t>Fallback shape text</a:t></a:r></a:p></p:txBody>
                </p:sp>
            </mc:Fallback>
        </mc:AlternateContent>
        <p:cxnSp>
            <p:txBody><a:p><a:r><a:t>Connector label</a:t></a:r></a:p></p:txBody>
        </p:cxnSp>
    </p:spTree></p:cSld>
</p:sld>"#;

        let pptx = build_single_slide_pptx(slide_xml, None, &[]);
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert!(
            result.content.contains("Fallback shape text"),
            "mc:Choice has no PresentationML content, so mc:Fallback must be used, got: {:?}",
            result.content
        );
        assert!(
            result.content.contains("Connector label"),
            "p:cxnSp connector text must be extracted, got: {:?}",
            result.content
        );
    }

    /// #80: chart (`c:chart`) and SmartArt/diagram (`dgm:relIds`) graphic frames
    /// reference text-bearing parts in separate ZIP entries; that text must be
    /// resolved and included in the extracted content.
    #[test]
    fn test_issue_80_chart_and_smartart_text_extracted() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
    <p:cSld><p:spTree>
        <p:graphicFrame>
            <p:nvGraphicFramePr><p:cNvPr id="2" name="Chart 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
            <a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/chart">
                    <c:chart xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart" r:id="rId2"/>
                </a:graphicData>
            </a:graphic>
        </p:graphicFrame>
        <p:graphicFrame>
            <p:nvGraphicFramePr><p:cNvPr id="3" name="SmartArt 1"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr>
            <a:graphic>
                <a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/diagram">
                    <dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId3"/>
                </a:graphicData>
            </a:graphic>
        </p:graphicFrame>
    </p:spTree></p:cSld>
</p:sld>"#;

        let slide_rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/chart" Target="../charts/chart1.xml"/>
    <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData" Target="../diagrams/data1.xml"/>
</Relationships>"#;

        let chart_xml: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<c:chartSpace xmlns:c="http://schemas.openxmlformats.org/drawingml/2006/chart"
              xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <c:chart>
    <c:title><c:tx><c:rich><a:p><a:r><a:t>Revenue by Quarter</a:t></a:r></a:p></c:rich></c:tx></c:title>
    <c:plotArea><c:barChart><c:ser>
      <c:cat><c:strRef><c:strCache><c:pt idx="0"><c:v>Q1</c:v></c:pt></c:strCache></c:strRef></c:cat>
      <c:val><c:numRef><c:numCache><c:pt idx="0"><c:v>42</c:v></c:pt></c:numCache></c:numRef></c:val>
    </c:ser></c:barChart></c:plotArea>
  </c:chart>
</c:chartSpace>"#;

        let diagram_xml: &[u8] = br#"<?xml version="1.0" encoding="UTF-8"?>
<dgm:dataModel xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram"
               xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main">
  <dgm:ptLst>
    <dgm:pt modelId="1" type="node"><dgm:t><a:p><a:r><a:t>Step One</a:t></a:r></a:p></dgm:t></dgm:pt>
    <dgm:pt modelId="2" type="node"><dgm:t><a:p><a:r><a:t>Step Two</a:t></a:r></a:p></dgm:t></dgm:pt>
  </dgm:ptLst>
</dgm:dataModel>"#;

        let pptx = build_single_slide_pptx(
            slide_xml,
            Some(slide_rels),
            &[
                ("ppt/charts/chart1.xml", chart_xml),
                ("ppt/diagrams/data1.xml", diagram_xml),
            ],
        );

        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert!(
            result.content.contains("Revenue by Quarter"),
            "chart title must be extracted, got: {:?}",
            result.content
        );
        assert!(
            result.content.contains("Q1") && result.content.contains("42"),
            "chart category/value text must be extracted, got: {:?}",
            result.content
        );
        assert!(
            result.content.contains("Step One") && result.content.contains("Step Two"),
            "SmartArt node text must be extracted, got: {:?}",
            result.content
        );
    }

    /// #90: `a:fld` (cached field text, e.g. slide number) and `a:br` (explicit
    /// line break) must contribute to paragraph text instead of being skipped.
    ///
    /// A separate title placeholder shape is included so the body paragraph
    /// below is rendered through the normal paragraph path (`add_text`, which
    /// preserves internal newlines) rather than being picked by the "shortest
    /// text becomes the title" heuristic, whose title path collapses `\n` to
    /// a space and would otherwise mask the very break this test verifies.
    #[test]
    fn test_issue_90_field_and_break_runs_extracted() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld><p:spTree>
        <p:sp>
            <p:nvSpPr><p:cNvPr id="2" name="Title"/><p:cNvSpPr/><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
            <p:txBody><a:p><a:r><a:t>Slide Title</a:t></a:r></a:p></p:txBody>
        </p:sp>
        <p:sp>
            <p:txBody>
                <a:p>
                    <a:r><a:t>Page </a:t></a:r>
                    <a:fld id="{00000000-0000-0000-0000-000000000000}" type="slidenum"><a:t>3</a:t></a:fld>
                    <a:br/>
                    <a:r><a:t>Second line</a:t></a:r>
                </a:p>
            </p:txBody>
        </p:sp>
    </p:spTree></p:cSld>
</p:sld>"#;

        let pptx = build_single_slide_pptx(slide_xml, None, &[]);
        let result = extract_pptx_from_bytes(
            &pptx,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut Vec::new(),
        )
        .unwrap();

        assert!(
            result.content.contains("Page 3\nSecond line"),
            "a:fld cached text and a:br line break must both be extracted in order, got: {:?}",
            result.content
        );
    }

    /// #91: images must be paired with their own shape's dimensions/alt-text by
    /// relationship ID, not by hash-map iteration order, and an image whose
    /// target cannot be read must be warned about and skipped rather than
    /// silently mis-paired.
    #[test]
    fn test_issue_91_image_pairing_by_rel_id_and_unreadable_image_warns() {
        let slide_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"
       xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships">
    <p:cSld><p:spTree>
        <p:pic>
            <p:nvPicPr><p:cNvPr id="2" name="Pic1" descr="First image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
            <p:blipFill><a:blip r:embed="rId2"/></p:blipFill>
            <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="952500" cy="952500"/></a:xfrm></p:spPr>
        </p:pic>
        <p:pic>
            <p:nvPicPr><p:cNvPr id="3" name="Pic2" descr="Second image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
            <p:blipFill><a:blip r:embed="rId3"/></p:blipFill>
            <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1905000" cy="1905000"/></a:xfrm></p:spPr>
        </p:pic>
        <p:pic>
            <p:nvPicPr><p:cNvPr id="4" name="Pic3" descr="Missing image"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr>
            <p:blipFill><a:blip r:embed="rId4"/></p:blipFill>
            <p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="100" cy="100"/></a:xfrm></p:spPr>
        </p:pic>
    </p:spTree></p:cSld>
</p:sld>"#;

        let slide_rels = r#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId2" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image1.png"/>
    <Relationship Id="rId3" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/image2.png"/>
    <Relationship Id="rId4" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/image" Target="../media/missing.png"/>
</Relationships>"#;

        let png_bytes: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x01\x00\x00\x00\x01\x08\x06\x00\x00\x00\x1f\x15\xc4\x89\x00\x00\x00\nIDATx\x9cc\x00\x01\x00\x00\x05\x00\x01\r\n-\xb4\x00\x00\x00\x00IEND\xaeB`\x82";

        let pptx = build_single_slide_pptx(
            slide_xml,
            Some(slide_rels),
            &[("ppt/media/image1.png", png_bytes), ("ppt/media/image2.png", png_bytes)],
        );

        let mut warnings = Vec::new();
        let result = extract_pptx_from_bytes(&pptx, &PptxExtractionOptions::default(), &mut warnings).unwrap();

        assert_eq!(
            result.images.len(),
            2,
            "the image with a missing target must not be counted as extracted"
        );
        assert_eq!(result.images[0].description.as_deref(), Some("First image"));
        assert_eq!(result.images[0].width, Some(100));
        assert_eq!(result.images[1].description.as_deref(), Some("Second image"));
        assert_eq!(result.images[1].width, Some(200));

        assert!(
            warnings.iter().any(|w| w.source == "pptx"
                && w.message == "Image 'rId4' referenced on slide 1 could not be read; it was not extracted"),
            "expected a warning naming the unreadable image, got: {:?}",
            warnings
        );
    }

    /// #238: a corrupt `docProps/core.xml` or comment part must be surfaced as a
    /// `ProcessingWarning` naming the part, not silently dropped while the rest
    /// of the document is returned as if it were complete.
    #[test]
    fn test_issue_238_warns_on_corrupt_metadata_and_comments() {
        use std::io::Write;
        use zip::write::{SimpleFileOptions, ZipWriter};

        let mut buffer = Vec::new();
        let mut zip = ZipWriter::new(std::io::Cursor::new(&mut buffer));
        let opts = SimpleFileOptions::default();

        zip.start_file("[Content_Types].xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">
    <Default Extension="xml" ContentType="application/xml"/>
    <Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>
</Types>"#,
        )
        .unwrap();

        zip.start_file("_rels/.rels", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/>
</Relationships>"#).unwrap();

        zip.start_file("ppt/presentation.xml", opts).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><presentation/>").unwrap();

        zip.start_file("ppt/_rels/presentation.xml.rels", opts).unwrap();
        zip.write_all(br#"<?xml version="1.0" encoding="UTF-8"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">
    <Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/>
</Relationships>"#).unwrap();

        zip.start_file("ppt/slides/slide1.xml", opts).unwrap();
        zip.write_all(
            br#"<?xml version="1.0" encoding="UTF-8"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"
       xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
    <p:cSld><p:spTree><p:sp><p:txBody><a:p><a:r><a:t>Slide text</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld>
</p:sld>"#,
        )
        .unwrap();

        zip.start_file("ppt/comments/comment1.xml", opts).unwrap();
        zip.write_all(b"not valid xml content").unwrap();

        zip.start_file("docProps/core.xml", opts).unwrap();
        zip.write_all(b"not valid xml content").unwrap();

        zip.start_file("docProps/app.xml", opts).unwrap();
        zip.write_all(b"<?xml version=\"1.0\"?><Properties xmlns=\"http://schemas.openxmlformats.org/officeDocument/2006/extended-properties\"><Slides>1</Slides></Properties>").unwrap();

        let _ = zip.finish().unwrap();

        let mut warnings = Vec::new();
        let result = extract_pptx_from_bytes(
            &buffer,
            &PptxExtractionOptions {
                extract_images: false,
                ..Default::default()
            },
            &mut warnings,
        )
        .unwrap();

        assert!(
            result.content.contains("Slide text"),
            "extraction of the rest of the document must still succeed"
        );

        assert!(
            warnings
                .iter()
                .any(|w| w.source == "pptx" && w.message.contains("docProps/core.xml")),
            "expected a warning naming the corrupt docProps/core.xml, got: {:?}",
            warnings
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.source == "pptx" && w.message.contains("comment1.xml")),
            "expected a warning naming the corrupt comment file, got: {:?}",
            warnings
        );
    }
}
