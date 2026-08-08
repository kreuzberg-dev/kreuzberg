//! Core PDF extraction functionality.
//!
//! Handles document loading, text extraction, metadata parsing, and table detection.

use crate::Result;
use crate::core::config::{ExtractionConfig, OutputFormat};
use crate::types::{PageBoundary, PageContent, PdfAnnotation};

#[cfg(feature = "pdf")]
use crate::types::Table;

#[cfg(feature = "pdf")]
pub(crate) type PdfExtractionPhaseResult = (
    crate::pdf::metadata::PdfExtractionMetadata,
    String,
    Vec<Table>,
    Option<Vec<PageContent>>,
    Option<Vec<PageBoundary>>,
    Option<crate::types::internal::InternalDocument>,
    bool,
    Option<Vec<PdfAnnotation>>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::PdfFormField>,
);

#[cfg(feature = "layout-detection")]
fn effective_layout_acceleration<'a>(
    config: &'a ExtractionConfig,
    acceleration_override: Option<&'a crate::core::config::acceleration::AccelerationConfig>,
) -> Option<&'a crate::core::config::acceleration::AccelerationConfig> {
    acceleration_override.or_else(|| config.resolved_layout_acceleration())
}

/// Extract text, metadata, tables, and annotations from a PDF document using the pdf_oxide backend.
///
/// Accepts an authenticated `OxideDocument`, then delegates to each oxide extraction module.
/// The return type is `PdfExtractionPhaseResult` so callers can switch transparently between
/// backends.
///
/// # Notes
///
/// - Layout detection is not yet supported on the oxide path.
/// - When output format is Markdown/Djot/HTML, the oxide hierarchy module extracts font
///   metrics and feeds them into the backend-agnostic structure pipeline for heading detection.
/// - Font encoding issue detection is not available; the flag is always `false`.
#[cfg(feature = "pdf")]
pub(crate) fn extract_all_from_oxide_document(
    mut doc: crate::pdf::oxide::OxideDocument,
    config: &ExtractionConfig,
    outline_entries: &[crate::pdf::bookmarks::PdfOutlineEntry],
    layout_hints: Option<&[Vec<crate::pdf::structure::types::LayoutHint>]>,
    #[cfg(feature = "layout-detection")] layout_images: Option<&[image::RgbImage]>,
    #[cfg(not(feature = "layout-detection"))] _layout_images: Option<()>,
    #[cfg(feature = "layout-detection")] layout_results: Option<&[crate::pdf::structure::types::PageLayoutResult]>,
    #[cfg(not(feature = "layout-detection"))] _layout_results: Option<()>,
    #[cfg(feature = "layout-detection")] layout_acceleration_override: Option<
        &crate::core::config::acceleration::AccelerationConfig,
    >,
) -> Result<PdfExtractionPhaseResult> {
    let _span = tracing::debug_span!("extract_pdf_oxide").entered();

    #[cfg_attr(not(feature = "layout-detection"), allow(unused_mut))]
    let (mut native_text, mut boundaries, page_contents, mut pdf_metadata) =
        crate::pdf::oxide::text::extract_text_and_metadata(&mut doc, Some(config)).map_err(|e| {
            crate::error::XbergError::Parsing {
                message: format!("pdf_oxide text extraction failed: {e}"),
                source: None,
            }
        })?;

    #[cfg(feature = "layout-detection")]
    if config.pdf_options.as_ref().is_some_and(|opts| opts.reading_order)
        && let Some(hints) = layout_hints
    {
        match apply_reading_order_reordering(&mut doc, &native_text, hints, config.pages.as_ref()) {
            Ok((reordered, reordered_boundaries)) => {
                native_text = reordered;
                // Reordering rebuilds the text, so boundaries computed against
                // the original extraction order no longer index it. ~keep
                if !reordered_boundaries.is_empty() {
                    if let Some(ref mut page_structure) = pdf_metadata.page_structure {
                        page_structure.boundaries = Some(reordered_boundaries.clone());
                    }
                    if boundaries.is_some() {
                        boundaries = Some(reordered_boundaries);
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    "reading-order reordering failed; using native text extraction order"
                );
            }
        }
    }
    #[cfg(not(feature = "layout-detection"))]
    let _ = layout_hints;

    let ocr_inline_images = config
        .pdf_options
        .as_ref()
        .map(|options| options.ocr_inline_images)
        .unwrap_or(false);
    let hierarchy_enabled = config
        .pdf_options
        .as_ref()
        .is_some_and(|options| options.hierarchy.as_ref().is_some_and(|hierarchy| hierarchy.enabled));
    // `Custom` covers renderers registered in the `RendererRegistry`, e.g.
    // `doctags`. They consume structure and geometry exactly as the built-in
    // markup formats do, and the flat path would leave their elements with no
    // headings and no bounding boxes at all.
    let needs_structured = hierarchy_enabled
        || matches!(
            config.output_format,
            OutputFormat::Markdown | OutputFormat::Djot | OutputFormat::Html | OutputFormat::Custom(_)
        )
        || ocr_inline_images;
    let retain_hierarchy_segments = needs_structured && !config.force_ocr;

    let extract_tables_flag = config.pdf_options.as_ref().is_none_or(|opts| opts.extract_tables);
    let allow_single_column = config
        .pdf_options
        .as_ref()
        .is_some_and(|o| o.allow_single_column_tables);
    let (tables, mut extracted_hierarchy_segments) = if extract_tables_flag {
        crate::pdf::oxide::guard_oxide_panic(
            || -> Result<(Vec<Table>, Option<crate::pdf::oxide::table::ExtractedHierarchySegments>)> {
                let mut combined = crate::pdf::oxide::table::extract_tables_native(&mut doc).unwrap_or_else(|e| {
                    tracing::warn!("pdf_oxide native table extraction failed, skipping tables: {e}");
                    Vec::new()
                });
                let native_pages: std::collections::HashSet<u32> = combined.iter().map(|t| t.page_number).collect();
                let bordered = crate::pdf::oxide::table::extract_tables_bordered(&mut doc, &native_pages)
                    .unwrap_or_else(|e| {
                        tracing::warn!("pdf_oxide bordered table extraction failed, skipping tables: {e}");
                        Vec::new()
                    });
                combined.extend(bordered);
                let covered_pages: std::collections::HashSet<u32> = combined.iter().map(|t| t.page_number).collect();
                let hierarchy_segments = match crate::pdf::oxide::table::extract_tables_heuristic(
                    &mut doc,
                    allow_single_column,
                    &covered_pages,
                ) {
                    Ok(extraction) => {
                        combined.extend(extraction.tables);
                        retain_hierarchy_segments.then_some(extraction.hierarchy_segments)
                    }
                    Err(error) => {
                        tracing::warn!("pdf_oxide heuristic table extraction failed, skipping tables: {error}");
                        None
                    }
                };
                let repaired_tables = combined
                    .iter_mut()
                    .map(crate::pdf::table_normalize::repair_consistently_merged_numeric_column)
                    .filter(|repaired| *repaired)
                    .count();
                if repaired_tables > 0 {
                    tracing::debug!(repaired_tables, "repaired collapsed PDF table columns");
                }
                Ok((combined, hierarchy_segments))
            },
            |panic| crate::error::XbergError::Parsing {
                message: format!("pdf_oxide panicked during table extraction: {panic}"),
                source: None,
            },
        )
        .unwrap_or_else(|e| {
            tracing::warn!("pdf_oxide table extraction panicked, skipping tables: {e}");
            (Vec::new(), None)
        })
    } else {
        (Vec::new(), None)
    };

    let annotations = if config.pdf_options.as_ref().is_some_and(|opts| opts.extract_annotations) {
        let extracted = crate::pdf::oxide::annotations::extract_annotations(&mut doc);
        if extracted.is_empty() { None } else { Some(extracted) }
    } else {
        None
    };

    let images_extraction_enabled =
        config.needs_image_data() || config.pdf_options.as_ref().map(|p| p.extract_images).unwrap_or(false);

    let (images, image_positions) = if images_extraction_enabled || ocr_inline_images {
        let max_images = config.images.as_ref().and_then(|i| i.max_images_per_page);
        let extracted =
            crate::pdf::oxide::images::extract_images_with_data(&mut doc, max_images, config.cancel_token.as_ref())
                .map_err(|e| crate::error::XbergError::Parsing {
                    message: format!("pdf_oxide image extraction failed: {e}"),
                    source: None,
                })?;

        let positions: Vec<(u32, u32)> = extracted
            .iter()
            .map(|img| (img.page_number.unwrap_or(1), img.image_index))
            .collect();
        (Some(extracted), positions)
    } else {
        (None, Vec::new())
    };

    if config.cancel_token.as_ref().is_some_and(|t| t.is_cancelled()) {
        return Err(crate::error::XbergError::Cancelled);
    }

    let pre_rendered_doc = if needs_structured && !config.force_ocr {
        let k = config
            .pdf_options
            .as_ref()
            .and_then(|opts| opts.hierarchy.as_ref())
            .map(|h| h.k_clusters)
            .unwrap_or(4);

        let (strip_repeating_text, include_headers, include_footers) = config
            .content_filter
            .as_ref()
            .map(|cf| (cf.strip_repeating_text, cf.include_headers, cf.include_footers))
            .unwrap_or((true, false, false));

        let (all_page_segments, used_structure_tree) = match extracted_hierarchy_segments.take() {
            Some(segments) => (segments.pages, segments.used_structure_tree),
            None => crate::pdf::oxide::hierarchy::extract_all_segments(&mut doc).map_err(|e| {
                crate::error::XbergError::Parsing {
                    message: format!("pdf_oxide hierarchy extraction failed: {e}"),
                    source: None,
                }
            })?,
        };

        let total_segs: usize = all_page_segments.iter().map(|s| s.len()).sum();
        tracing::debug!(
            total_segs,
            k,
            used_structure_tree,
            "oxide structure: extracted segments for heading detection"
        );

        let inject_placeholders =
            images_extraction_enabled && config.images.as_ref().map(|c| c.inject_placeholders).unwrap_or(false);

        match crate::pdf::structure::extract_document_structure_from_segments(
            all_page_segments,
            crate::pdf::structure::SegmentStructureConfig {
                k_clusters: k,
                tables: &tables,
                outline_entries,
                strip_repeating_text,
                include_headers,
                include_footers,
                used_structure_tree,
                image_positions: &image_positions,
                images: images.as_deref(),
                inject_placeholders,
                layout_hints,
                allow_single_column,
                cancel_token: config.cancel_token.as_ref(),
                #[cfg(feature = "layout-detection")]
                layout_images,
                #[cfg(feature = "layout-detection")]
                layout_results,
                #[cfg(feature = "layout-detection")]
                table_model: config.layout.as_ref().map(|l| l.table_model).unwrap_or_default(),
                #[cfg(feature = "layout-detection")]
                table_overlap_preference: config
                    .layout
                    .as_ref()
                    .map(|l| l.table_overlap_preference)
                    .unwrap_or_default(),
                #[cfg(feature = "layout-detection")]
                acceleration: effective_layout_acceleration(config, layout_acceleration_override),
                #[cfg(feature = "layout-detection")]
                session_thread_budget: crate::core::config::concurrency::resolve_thread_budget(
                    config.concurrency.as_ref(),
                ),
            },
        ) {
            Ok(structured_doc) if !structured_doc.elements.is_empty() => {
                tracing::debug!(
                    elements = structured_doc.elements.len(),
                    has_headings = structured_doc
                        .elements
                        .iter()
                        .any(|e| matches!(e.kind, crate::types::internal::ElementKind::Heading { .. })),
                    "oxide structure: render succeeded"
                );
                Some(structured_doc)
            }
            Ok(_) => {
                tracing::warn!("oxide structure: rendering produced empty output, falling back to plain text");
                None
            }
            Err(e) => {
                tracing::warn!("oxide structure: rendering failed: {:?}, falling back to plain text", e);
                None
            }
        }
    } else {
        None
    };

    let has_font_encoding_issues = false;

    let form_fields = if config.pdf_options.as_ref().is_none_or(|opts| opts.extract_form_fields) {
        crate::pdf::oxide::forms::extract_form_fields(&mut doc)
    } else {
        Vec::new()
    };

    Ok((
        pdf_metadata,
        native_text,
        tables,
        page_contents,
        boundaries,
        pre_rendered_doc,
        has_font_encoding_issues,
        annotations,
        images,
        form_fields,
    ))
}

/// Apply reading-order reordering using layout-detected regions.
///
/// Extracts text spans from each page, projects them onto layout regions,
/// performs column detection, and rebuilds the text in natural reading order.
///
/// Returns the reordered text string together with page boundaries recomputed
/// against it — the rebuilt string has a different byte layout, so boundaries
/// from the original extraction must not be used to index it. An empty
/// boundary vector means the text was returned unchanged. Page markers from
/// `page_config` are preserved in the rebuilt text.
#[cfg(feature = "layout-detection")]
fn apply_reading_order_reordering(
    doc: &mut crate::pdf::oxide::OxideDocument,
    native_text: &str,
    layout_hints_per_page: &[Vec<crate::pdf::structure::types::LayoutHint>],
    page_config: Option<&crate::core::config::PageConfig>,
) -> Result<(String, Vec<crate::types::PageBoundary>)> {
    use crate::extractors::pdf::reading_order;

    let page_count = doc.doc.page_count().map_err(|e| crate::error::XbergError::Parsing {
        message: format!("reading-order reordering: failed to get page count: {e}"),
        source: None,
    })?;

    if layout_hints_per_page.len() != page_count {
        return Err(crate::error::XbergError::Parsing {
            message: format!(
                "reading-order reordering: layout hints count ({}) != page count ({})",
                layout_hints_per_page.len(),
                page_count
            ),
            source: None,
        });
    }

    let mut reordered_pages = Vec::with_capacity(page_count);

    for (page_idx, hints) in layout_hints_per_page.iter().enumerate().take(page_count) {
        let (spans, reordered_sparse_columns) =
            crate::pdf::oxide::text::extract_spans_from_page(&mut doc.doc, page_idx).map_err(|e| {
                crate::error::XbergError::Parsing {
                    message: format!(
                        "reading-order reordering: failed to extract spans from page {}: {e}",
                        page_idx + 1
                    ),
                    source: None,
                }
            })?;

        let span_order: Vec<usize> = if hints.is_empty() || (reordered_sparse_columns && hints.len() == 1) {
            (0..spans.len()).collect()
        } else {
            reading_order::reorder_spans_by_layout(&spans, hints)
        };

        let mut page_text = String::new();
        for &span_idx in &span_order {
            if span_idx < spans.len() {
                page_text.push_str(&spans[span_idx].text);
            }
        }

        reordered_pages.push(page_text);
    }

    if reordered_pages.is_empty() {
        return Ok((native_text.to_string(), Vec::new()));
    }

    Ok(join_pages_with_boundaries(&reordered_pages, page_config))
}

/// Join per-page texts, recording each page's byte range in the combined
/// string, faithful to how `extract_text_from_oxide_document` assembles it:
/// a rendered page marker before each page when `insert_page_markers` is on,
/// otherwise `"\n\n"` separators between pages. Markers and separators belong
/// to no page.
#[cfg(feature = "layout-detection")]
fn join_pages_with_boundaries(
    pages: &[String],
    page_config: Option<&crate::core::config::PageConfig>,
) -> (String, Vec<crate::types::PageBoundary>) {
    let markers = page_config.filter(|c| c.insert_page_markers);
    let mut content = String::new();
    let mut boundaries = Vec::with_capacity(pages.len());
    for (idx, page_text) in pages.iter().enumerate() {
        if let Some(config) = markers {
            let marker = config.marker_format.replace("{page_num}", &(idx + 1).to_string());
            content.push_str(&marker);
        } else if idx > 0 {
            content.push_str("\n\n");
        }
        let byte_start = content.len();
        content.push_str(page_text);
        boundaries.push(crate::types::PageBoundary {
            byte_start,
            byte_end: content.len(),
            page_number: idx as u32 + 1,
        });
    }
    (content, boundaries)
}

#[cfg(test)]
mod tests {

    #[cfg(feature = "layout-detection")]
    #[test]
    fn cpu_layout_retry_override_controls_native_table_model() {
        use crate::core::config::{
            acceleration::{AccelerationConfig, ExecutionProviderType},
            layout::LayoutDetectionConfig,
        };

        let config = crate::ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                acceleration: Some(AccelerationConfig {
                    provider: ExecutionProviderType::CoreMl,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        let cpu = AccelerationConfig {
            provider: ExecutionProviderType::Cpu,
            ..Default::default()
        };

        assert_eq!(
            super::effective_layout_acceleration(&config, Some(&cpu)).map(|acceleration| &acceleration.provider),
            Some(&ExecutionProviderType::Cpu)
        );
        assert_eq!(
            super::effective_layout_acceleration(&config, None).map(|acceleration| &acceleration.provider),
            Some(&ExecutionProviderType::CoreMl)
        );
    }

    /// Boundaries produced alongside reordered text must index it exactly:
    /// char-boundary-valid offsets whose slice is the page's text, with the
    /// `"\n\n"` separators belonging to no page.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_join_pages_with_boundaries_multibyte() {
        let pages = vec![
            "CLASSIFICATION • Classification: SECRET".to_string(),
            "second — page's text with curly \u{201c}quotes\u{201d}".to_string(),
            String::new(),
            "final page".to_string(),
        ];
        let (content, boundaries) = super::join_pages_with_boundaries(&pages, None);
        assert_eq!(content, pages.join("\n\n"));
        assert_eq!(boundaries.len(), pages.len());
        for (i, b) in boundaries.iter().enumerate() {
            assert_eq!(b.page_number, i as u32 + 1);
            assert!(content.is_char_boundary(b.byte_start));
            assert!(content.is_char_boundary(b.byte_end));
            assert_eq!(&content[b.byte_start..b.byte_end], pages[i]);
        }
    }

    /// With `insert_page_markers` on, the reordered rebuild must emit the same
    /// rendered markers as initial extraction: one before each page, outside
    /// the page's byte range.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn test_join_pages_with_boundaries_page_markers() {
        let pages = vec!["first • page".to_string(), "second page".to_string()];
        let page_config = crate::core::config::PageConfig {
            insert_page_markers: true,
            marker_format: "\n\n<!-- PAGE {page_num} -->\n\n".to_string(),
            ..Default::default()
        };
        let (content, boundaries) = super::join_pages_with_boundaries(&pages, Some(&page_config));
        assert_eq!(
            content,
            "\n\n<!-- PAGE 1 -->\n\nfirst • page\n\n<!-- PAGE 2 -->\n\nsecond page"
        );
        for (i, b) in boundaries.iter().enumerate() {
            assert_eq!(
                &content[b.byte_start..b.byte_end],
                pages[i],
                "markers stay outside page ranges"
            );
        }
    }

    #[test]
    fn test_bounding_box_coordinate_conversion() {
        let page_height = 800.0_f64;

        let img_left = 50.0_f64;
        let img_top = 100.0_f64;
        let img_right = 300.0_f64;
        let img_bottom = 150.0_f64;

        let bbox = crate::types::BoundingBox {
            x0: img_left,
            y0: page_height - img_bottom,
            x1: img_right,
            y1: page_height - img_top,
        };

        assert_eq!(bbox.x0, 50.0);
        assert_eq!(bbox.y0, 650.0);
        assert_eq!(bbox.x1, 300.0);
        assert_eq!(bbox.y1, 700.0);
        assert!(bbox.y1 > bbox.y0);
    }

    #[test]
    fn test_bounding_box_coordinate_conversion_different_scales() {
        let page_height = 1000.0_f64;

        let img_left = 100.0_f64;
        let img_top = 50.0_f64;
        let img_right = 600.0_f64;
        let img_bottom = 400.0_f64;

        let bbox = crate::types::BoundingBox {
            x0: img_left,
            y0: page_height - img_bottom,
            x1: img_right,
            y1: page_height - img_top,
        };

        assert_eq!(bbox.x0, 100.0);
        assert_eq!(bbox.y0, 600.0);
        assert_eq!(bbox.x1, 600.0);
        assert_eq!(bbox.y1, 950.0);
        assert_eq!(bbox.y1 - bbox.y0, 350.0);
    }

    #[test]
    fn test_bounding_box_coordinate_conversion_preserves_width() {
        let page_height = 595.0_f64;

        let img_left = 72.0_f64;
        let img_right = 522.0_f64;
        let img_top = 36.0_f64;
        let img_bottom = 300.0_f64;

        let bbox = crate::types::BoundingBox {
            x0: img_left,
            y0: page_height - img_bottom,
            x1: img_right,
            y1: page_height - img_top,
        };

        let expected_width = img_right - img_left;
        let actual_width = bbox.x1 - bbox.x0;
        assert_eq!(actual_width, expected_width);
        assert_eq!(actual_width, 450.0);
    }

    #[test]
    fn test_bounding_box_serialization_round_trip() {
        let original = crate::types::BoundingBox {
            x0: 10.5,
            y0: 20.25,
            x1: 100.75,
            y1: 200.5,
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: crate::types::BoundingBox = serde_json::from_str(&json).unwrap();

        assert_eq!(original, deserialized);
        assert_eq!(deserialized.x0, 10.5);
        assert_eq!(deserialized.y0, 20.25);
        assert_eq!(deserialized.x1, 100.75);
        assert_eq!(deserialized.y1, 200.5);
    }
}
