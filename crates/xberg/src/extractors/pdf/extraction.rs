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
    Option<AnnotationTextFallback>,
    Option<Vec<PdfAnnotation>>,
    Option<Vec<crate::types::ExtractedImage>>,
    Vec<crate::types::PdfFormField>,
    Vec<crate::types::ProcessingWarning>,
    Option<Vec<String>>,
);

#[cfg(feature = "pdf")]
#[derive(Clone)]
pub(crate) struct AnnotationTextFallback {
    native_pages: Vec<String>,
    annotation_pages: Vec<Vec<String>>,
}

#[cfg(feature = "pdf")]
pub(crate) struct AnnotationFallbackTarget<'a> {
    pub(crate) text: &'a mut String,
    pub(crate) page_contents: &'a mut Option<Vec<PageContent>>,
    pub(crate) boundaries: &'a mut Option<Vec<PageBoundary>>,
    pub(crate) pdf_metadata: &'a mut crate::pdf::metadata::PdfExtractionMetadata,
    pub(crate) page_config: Option<&'a crate::core::config::PageConfig>,
}

#[cfg(feature = "layout-detection")]
fn effective_layout_acceleration<'a>(
    config: &'a ExtractionConfig,
    acceleration_override: Option<&'a crate::core::config::acceleration::AccelerationConfig>,
) -> Option<&'a crate::core::config::acceleration::AccelerationConfig> {
    acceleration_override.or_else(|| config.resolved_layout_acceleration())
}

/// Whether the PDF pipeline must retain the full native hierarchy/geometry pass
/// rather than take the cheaper flat-text path.
///
/// `Custom(_)` is deliberately excluded: `OutputFormat::from_str` never rejects an
/// unknown string (any typo becomes `Custom(other)`), so treating every `Custom`
/// value as structure-hungry ran the full hierarchy pass — `extract_all_segments`
/// plus `extract_document_structure_from_segments`, including k-means heading
/// clustering — for a typo'd or unregistered format, only to discard the result
/// and fall back to plain text during derivation
/// (`core/pipeline/format.rs`'s `custom_fallback_to_plain`). `DocTags` is a real,
/// always-registered built-in renderer that needs the same geometry and headings
/// as Markdown/Djot/HTML, so it gets its own explicit arm instead.
fn needs_structured_extraction(
    hierarchy_enabled: bool,
    output_format: &OutputFormat,
    ocr_inline_images: bool,
    content_filter_configured: bool,
) -> bool {
    hierarchy_enabled
        || matches!(
            output_format,
            OutputFormat::Markdown | OutputFormat::Djot | OutputFormat::Html | OutputFormat::DocTags
        )
        || ocr_inline_images
        || content_filter_configured
}

fn hierarchy_cluster_count(config: &ExtractionConfig) -> usize {
    config
        .pdf_options
        .as_ref()
        .and_then(|options| options.hierarchy.as_ref())
        .map_or_else(
            || crate::core::config::HierarchyConfig::default().k_clusters,
            |hierarchy| hierarchy.k_clusters,
        )
}

/// Report a table-extraction failure that took out a whole detector pass, not just one page.
///
/// The per-page warnings in `pdf::native::table` cannot cover these: a stage that fails or
/// panics outright returns no pages at all, so its own warnings never reach the caller and
/// the document comes back with an empty `tables` list indistinguishable from a PDF that
/// genuinely has none. ~keep
///
/// Module-scoped (rather than nested inside `extract_all_from_native_document`) so
/// `mod tests` below can exercise its message format directly — see
/// `table_stage_failure_warning_formats_the_message_it_promises`.
fn table_stage_failure_warning(stage: &str, error: &impl std::fmt::Display) -> crate::types::ProcessingWarning {
    crate::types::ProcessingWarning {
        source: std::borrow::Cow::Borrowed("pdf_tables"),
        message: std::borrow::Cow::Owned(format!(
            "{stage} table extraction failed for the document: {error}; \
             tables from this pass were skipped"
        )),
    }
}

#[cfg(feature = "pdf")]
fn extract_annotations_with_empty_body_fallback(
    doc: &mut crate::pdf::native::NativeDocument,
    requested: bool,
    native_text: &str,
    page_contents: Option<&[PageContent]>,
    boundaries: Option<&[PageBoundary]>,
) -> (
    Option<Vec<PdfAnnotation>>,
    Option<AnnotationTextFallback>,
    Vec<crate::types::ProcessingWarning>,
) {
    if requested {
        let (annotations, warnings) = crate::pdf::native::annotations::extract_annotations(doc);
        return ((!annotations.is_empty()).then_some(annotations), None, warnings);
    }

    let page_count = match doc.doc.page_count() {
        Ok(page_count) => page_count,
        Err(error) => {
            return (
                None,
                None,
                vec![crate::pdf::native::annotations::page_count_failure_warning(&error)],
            );
        }
    };
    let native_pages = native_page_texts(native_text, page_contents, boundaries, page_count);
    if native_pages.len() != page_count {
        return (None, None, Vec::new());
    }
    let eligible_pages: Vec<bool> = native_pages.iter().map(|page| page.trim().is_empty()).collect();
    if !eligible_pages.iter().any(|eligible| *eligible) {
        return (None, None, Vec::new());
    }
    let (annotations, mut warnings) =
        crate::pdf::native::annotations::extract_visible_free_text_annotations(doc, &eligible_pages);
    let (annotation_pages, recovered_count) = annotation_text_by_page(&annotations, page_count);
    if recovered_count > 0 {
        warnings.push(crate::types::ProcessingWarning {
            source: std::borrow::Cow::Borrowed("pdf_annotations"),
            message: std::borrow::Cow::Owned(format!(
                "native PDF page text was empty; recovered {} text-bearing annotation(s) as document content",
                recovered_count
            )),
        });
    }

    let fallback = (recovered_count > 0).then_some(AnnotationTextFallback {
        native_pages,
        annotation_pages,
    });
    (None, fallback, warnings)
}

#[cfg(feature = "pdf")]
fn native_page_texts(
    native_text: &str,
    page_contents: Option<&[PageContent]>,
    boundaries: Option<&[PageBoundary]>,
    page_count: usize,
) -> Vec<String> {
    if let Some(pages) = page_contents.filter(|pages| pages.len() == page_count) {
        return pages.iter().map(|page| page.content.clone()).collect();
    }
    if let Some(boundaries) = boundaries.filter(|boundaries| boundaries.len() == page_count) {
        return boundaries
            .iter()
            .map(|boundary| {
                native_text
                    .get(boundary.byte_start..boundary.byte_end)
                    .unwrap_or_default()
                    .to_string()
            })
            .collect();
    }
    if native_text.trim().is_empty() {
        return vec![String::new(); page_count];
    }
    vec![native_text.to_string()]
}

#[cfg(feature = "pdf")]
fn annotation_text_by_page(annotations: &[PdfAnnotation], page_count: usize) -> (Vec<Vec<String>>, usize) {
    let mut pages = vec![Vec::new(); page_count];
    let mut recovered_count = 0;
    for annotation in annotations {
        let Some(content) = annotation
            .content
            .as_deref()
            .map(str::trim)
            .filter(|content| !content.is_empty())
        else {
            continue;
        };
        let Some(page) = annotation
            .page_number
            .checked_sub(1)
            .and_then(|page| pages.get_mut(page as usize))
        else {
            continue;
        };
        page.push(content.to_string());
        recovered_count += 1;
    }
    (pages, recovered_count)
}

#[cfg(feature = "pdf")]
pub(crate) fn apply_annotation_text_fallback(
    fallback: &AnnotationTextFallback,
    ocr_page_texts: Option<&[String]>,
    ocr_results: Option<&ahash::AHashMap<u32, String>>,
    full_ocr: bool,
    target: AnnotationFallbackTarget<'_>,
) {
    let mut native_pages = fallback.native_pages.clone();
    let single_string_ocr = native_pages.len() > 1 && ocr_page_texts.is_some_and(|pages| pages.len() == 1);
    if full_ocr && single_string_ocr {
        let authoritative_text = ocr_page_texts
            .and_then(|pages| pages.first())
            .cloned()
            .unwrap_or_else(|| target.text.clone());
        native_pages.fill(String::new());
        if let Some(first_page) = native_pages.first_mut() {
            *first_page = authoritative_text;
        }
    } else if full_ocr {
        if let Some(ocr_pages) = ocr_page_texts.filter(|pages| pages.len() == native_pages.len()) {
            native_pages.clone_from_slice(ocr_pages);
        } else {
            let authoritative_text = target.text.clone();
            native_pages.fill(String::new());
            if let Some(first_page) = native_pages.first_mut() {
                *first_page = authoritative_text;
            }
        }
    } else if let Some(ocr_results) = ocr_results {
        for (page_number, content) in ocr_results {
            if let Some(page) = page_number
                .checked_sub(1)
                .and_then(|page| native_pages.get_mut(page as usize))
            {
                page.clone_from(content);
            }
        }
    }
    for (page, annotations) in native_pages.iter_mut().zip(&fallback.annotation_pages) {
        for annotation in annotations {
            if page_has_exact_text_block(page, annotation) {
                continue;
            }
            if !page.is_empty() {
                page.push('\n');
            }
            page.push_str(annotation);
        }
    }

    if let Some(pages) = target.page_contents.as_mut() {
        for page in pages.iter_mut() {
            if let Some(content) = native_pages.get(page.page_number.saturating_sub(1) as usize) {
                page.content.clone_from(content);
                page.is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(content));
            }
        }
    }

    let (rebuilt_text, rebuilt_boundaries) = join_pages_with_boundaries(&native_pages, target.page_config);
    *target.text = rebuilt_text;
    if target.boundaries.is_some() || target.page_contents.is_some() || target.pdf_metadata.page_structure.is_some() {
        *target.boundaries = Some(rebuilt_boundaries.clone());
    }
    if let Some(page_structure) = target.pdf_metadata.page_structure.as_mut() {
        page_structure.boundaries = Some(rebuilt_boundaries);
        if let Some(pages) = page_structure.pages.as_mut() {
            for page in pages {
                page.is_blank = native_pages
                    .get(page.number.saturating_sub(1) as usize)
                    .map(|content| crate::extraction::blank_detection::is_page_text_blank(content));
            }
        }
    }
}

fn page_has_exact_text_block(page: &str, annotation: &str) -> bool {
    let annotation_lines: Vec<_> = annotation.trim().lines().map(str::trim).collect();
    if annotation_lines.is_empty() {
        return false;
    }

    let page_lines: Vec<_> = page.lines().map(str::trim).collect();
    page_lines
        .windows(annotation_lines.len())
        .any(|lines| lines == annotation_lines)
}

#[cfg(feature = "pdf")]
pub(crate) fn append_annotation_fallback_elements(
    fallback: &AnnotationTextFallback,
    document: &mut crate::types::internal::InternalDocument,
) {
    for (page_index, annotations) in fallback.annotation_pages.iter().enumerate() {
        for annotation in annotations {
            if document.elements.iter().any(|element| {
                element.page == Some(page_index as u32 + 1) && page_has_exact_text_block(&element.text, annotation)
            }) {
                continue;
            }
            document.push_element(
                crate::types::internal::InternalElement::text(
                    crate::types::internal::ElementKind::Paragraph,
                    annotation,
                    0,
                )
                .with_page(page_index as u32 + 1),
            );
        }
    }
}

#[cfg(feature = "pdf")]
fn retain_segments_inside_page_margins(
    segments: &mut Vec<crate::pdf::hierarchy::SegmentData>,
    page_bottom: f32,
    page_top: f32,
    margins: crate::pdf::native::text::PageMarginFractions,
) {
    segments.retain(|segment| {
        crate::pdf::native::text::baseline_is_inside_page_margins(segment.baseline_y, page_bottom, page_top, margins)
    });
}

/// Extract text, metadata, tables, and annotations from a PDF document using the xberg_native_pdf backend.
///
/// Accepts an authenticated `NativeDocument`, then delegates to each native extraction module.
/// The return type is `PdfExtractionPhaseResult` so callers can switch transparently between
/// backends.
///
/// # Notes
///
/// - With the `layout-detection` feature, layout images, per-page layout results, the configured
///   table model and table-overlap preference, and the resolved layout acceleration are threaded
///   into `SegmentStructureConfig`; layout hints additionally drive reading-order reordering.
/// - When output format is Markdown/Djot/HTML, the native hierarchy module extracts font
///   metrics and feeds them into the backend-agnostic structure pipeline for heading detection.
/// - Font encoding issue detection is not available; the flag is always `false`.
#[cfg(feature = "pdf")]
pub(crate) fn extract_all_from_native_document(
    mut doc: crate::pdf::native::NativeDocument,
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
    let _span = tracing::debug_span!("extract_xberg_native_pdf").entered();
    let margins = crate::pdf::native::text::PageMarginFractions::from_extraction_config(Some(config));
    let annotation_fallback_requested = !config
        .pdf_options
        .as_ref()
        .is_some_and(|options| options.extract_annotations);
    let force_annotation_page_tracking = annotation_fallback_requested && config.pages.is_none();
    let hierarchy_enabled = config
        .pdf_options
        .as_ref()
        .is_some_and(|options| options.hierarchy.as_ref().is_some_and(|hierarchy| hierarchy.enabled));
    // ~keep A `PageHierarchy` can only be hung off a `PageContent` (`pages.rs`'s
    // `assign_hierarchy_to_pages`), and `page_contents` is produced only when
    // `pages.extract_pages` is set. `PageConfig::default()` leaves that `false`, so
    // `pdf_options.hierarchy.enabled` was a silent no-op unless the caller also set an
    // unrelated flag nothing in the hierarchy config points at: headings were detected, then
    // dropped for want of a page to attach them to (CI E2E `test_pdf_hierarchy_config`).
    // Asking for the hierarchy is the opt-in for the per-page tracking it requires.
    let force_hierarchy_page_tracking =
        hierarchy_enabled && !config.pages.as_ref().is_some_and(|pages| pages.extract_pages);
    let mut tracked_config;
    let text_config = if force_annotation_page_tracking || force_hierarchy_page_tracking {
        tracked_config = config.clone();
        let mut page_config = tracked_config.pages.take().unwrap_or_default();
        if force_hierarchy_page_tracking {
            page_config.extract_pages = true;
        }
        tracked_config.pages = Some(page_config);
        &tracked_config
    } else {
        config
    };
    let page_structure_was_implicitly_tracked = force_annotation_page_tracking
        && config.ocr.is_none()
        && config.force_ocr_pages.as_ref().is_none_or(Vec::is_empty);

    #[cfg_attr(not(feature = "layout-detection"), allow(unused_mut))]
    let (mut native_text, mut boundaries, mut page_contents, mut pdf_metadata) =
        crate::pdf::native::text::extract_text_and_metadata(&mut doc, Some(text_config)).map_err(|e| {
            crate::error::XbergError::Parsing {
                message: format!("xberg_native_pdf text extraction failed: {e}"),
                source: None,
            }
        })?;

    #[cfg(feature = "layout-detection")]
    if config.pdf_options.as_ref().is_some_and(|opts| opts.reading_order)
        && let Some(hints) = layout_hints
    {
        match apply_reading_order_reordering(&mut doc, &native_text, hints, config.pages.as_ref(), margins) {
            Ok((reordered, reordered_boundaries, reordered_per_page)) => {
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
                apply_reordered_text_to_page_contents(&mut page_contents, reordered_per_page);
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
    let needs_structured = needs_structured_extraction(
        hierarchy_enabled,
        &config.output_format,
        ocr_inline_images,
        config.content_filter.is_some(),
    );
    let retain_hierarchy_segments = needs_structured && !config.force_ocr;

    let mut extraction_warnings: Vec<crate::types::ProcessingWarning> = Vec::new();

    let extract_tables_flag = config.pdf_options.as_ref().is_none_or(|opts| opts.extract_tables);
    let allow_single_column = config
        .pdf_options
        .as_ref()
        .is_some_and(|o| o.allow_single_column_tables);
    let (tables, mut extracted_hierarchy_segments, table_warnings) = if extract_tables_flag {
        crate::pdf::native::guard_native_panic(
            || -> Result<(
                Vec<Table>,
                Option<crate::pdf::native::table::ExtractedHierarchySegments>,
                Vec<crate::types::ProcessingWarning>,
            )> {
                let mut warnings = Vec::new();
                let (mut combined, native_warnings) =
                    crate::pdf::native::table::extract_tables_native(&mut doc).unwrap_or_else(|e| {
                        tracing::warn!("xberg_native_pdf native table extraction failed, skipping tables: {e}");
                        (Vec::new(), vec![table_stage_failure_warning("native", &e)])
                    });
                warnings.extend(native_warnings);
                let native_pages: std::collections::HashSet<u32> = combined.iter().map(|t| t.page_number).collect();
                let (bordered, bordered_warnings) =
                    crate::pdf::native::table::extract_tables_bordered(&mut doc, &native_pages).unwrap_or_else(|e| {
                        tracing::warn!("xberg_native_pdf bordered table extraction failed, skipping tables: {e}");
                        (Vec::new(), vec![table_stage_failure_warning("bordered", &e)])
                    });
                combined.extend(bordered);
                warnings.extend(bordered_warnings);
                let covered_pages: std::collections::HashSet<u32> = combined.iter().map(|t| t.page_number).collect();
                let hierarchy_segments = match crate::pdf::native::table::extract_tables_heuristic(
                    &mut doc,
                    allow_single_column,
                    &covered_pages,
                ) {
                    Ok(extraction) => {
                        combined.extend(extraction.tables);
                        retain_hierarchy_segments.then_some(extraction.hierarchy_segments)
                    }
                    Err(error) => {
                        tracing::warn!("xberg_native_pdf heuristic table extraction failed, skipping tables: {error}");
                        warnings.push(table_stage_failure_warning("heuristic", &error));
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
                Ok((combined, hierarchy_segments, warnings))
            },
            |panic| crate::error::XbergError::Parsing {
                message: format!("xberg_native_pdf panicked during table extraction: {panic}"),
                source: None,
            },
        )
        .unwrap_or_else(|e| {
            tracing::warn!("xberg_native_pdf table extraction panicked, skipping tables: {e}");
            (Vec::new(), None, vec![table_stage_failure_warning("whole-document", &e)])
        })
    } else {
        (Vec::new(), None, Vec::new())
    };
    extraction_warnings.extend(table_warnings);

    let (annotations, annotation_text_fallback, annotation_warnings) = extract_annotations_with_empty_body_fallback(
        &mut doc,
        config
            .pdf_options
            .as_ref()
            .is_some_and(|options| options.extract_annotations),
        &native_text,
        page_contents.as_deref(),
        boundaries.as_deref(),
    );
    extraction_warnings.extend(annotation_warnings);
    if page_structure_was_implicitly_tracked {
        pdf_metadata.page_structure = None;
    }

    let images_extraction_enabled =
        config.needs_image_data() || config.pdf_options.as_ref().map(|p| p.extract_images).unwrap_or(false);

    let (images, image_positions) = if images_extraction_enabled || ocr_inline_images {
        let max_images = config.images.as_ref().and_then(|i| i.max_images_per_page);
        let (extracted, image_warnings) =
            crate::pdf::native::images::extract_images_with_data(&mut doc, max_images, config.cancel_token.as_ref())
                .map_err(|e| crate::error::XbergError::Parsing {
                    message: format!("xberg_native_pdf image extraction failed: {e}"),
                    source: None,
                })?;
        extraction_warnings.extend(image_warnings);

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
        let k = hierarchy_cluster_count(config);

        // `HierarchyConfig::include_bbox` (default `true`): when explicitly disabled,
        // bounding-box info is stripped from every structural element below so
        // `assign_hierarchy_to_pages` (extractors/pdf/pages.rs) — which maps
        // `element.bbox` straight onto each `HierarchicalBlock.bbox` — omits it too. ~keep
        let include_bbox = config
            .pdf_options
            .as_ref()
            .and_then(|opts| opts.hierarchy.as_ref())
            .map(|h| h.include_bbox)
            .unwrap_or(true);

        let (strip_repeating_text, include_headers, include_footers, include_footnotes, include_watermarks) = config
            .content_filter
            .as_ref()
            .map(|cf| {
                (
                    cf.strip_repeating_text,
                    cf.include_headers,
                    cf.include_footers,
                    cf.include_footnotes,
                    cf.include_watermarks,
                )
            })
            .unwrap_or((true, false, false, false, false));

        let (mut all_page_segments, used_structure_tree) = match extracted_hierarchy_segments.take() {
            Some(segments) => (segments.pages, segments.used_structure_tree),
            None => crate::pdf::native::hierarchy::extract_all_segments(&mut doc).map_err(|e| {
                crate::error::XbergError::Parsing {
                    message: format!("xberg_native_pdf hierarchy extraction failed: {e}"),
                    source: None,
                }
            })?,
        };

        for (page_index, segments) in all_page_segments.iter_mut().enumerate() {
            let (_, lower_y, _, upper_y) =
                doc.doc
                    .get_page_media_box(page_index)
                    .map_err(|error| crate::error::XbergError::Parsing {
                        message: format!(
                            "xberg_native_pdf failed to read page {} media box for margin filtering: {error}",
                            page_index + 1
                        ),
                        source: None,
                    })?;
            retain_segments_inside_page_margins(segments, lower_y.min(upper_y), lower_y.max(upper_y), margins);
        }

        let total_segs: usize = all_page_segments.iter().map(|s| s.len()).sum();
        tracing::debug!(
            total_segs,
            k,
            used_structure_tree,
            "native structure: extracted segments for heading detection"
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
                include_footnotes,
                include_watermarks,
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
            Ok(mut structured_doc) if !structured_doc.elements.is_empty() => {
                tracing::debug!(
                    elements = structured_doc.elements.len(),
                    has_headings = structured_doc
                        .elements
                        .iter()
                        .any(|e| matches!(e.kind, crate::types::internal::ElementKind::Heading { .. })),
                    "native structure: render succeeded"
                );
                if !include_bbox {
                    for element in &mut structured_doc.elements {
                        element.bbox = None;
                    }
                }
                Some(structured_doc)
            }
            Ok(_) => {
                tracing::warn!("native structure: rendering produced empty output, falling back to plain text");
                None
            }
            Err(e) => {
                tracing::warn!(
                    "native structure: rendering failed: {:?}, falling back to plain text",
                    e
                );
                None
            }
        }
    } else {
        None
    };

    let has_font_encoding_issues = false;

    let form_fields = if config.pdf_options.as_ref().is_none_or(|opts| opts.extract_form_fields) {
        let (fields, form_warnings) = crate::pdf::native::forms::extract_form_fields(&mut doc);
        extraction_warnings.extend(form_warnings);
        fields
    } else {
        Vec::new()
    };

    // Issue #66: `/PageLabels` (roman-numeral front matter, per-section
    // numbering, ...). `None` when the document defines none, which is the
    // common case.
    let page_labels = crate::pdf::native::metadata::extract_page_labels_all(&mut doc).unwrap_or_else(|e| {
        tracing::debug!("page label extraction failed: {e}");
        None
    });

    Ok((
        pdf_metadata,
        native_text,
        tables,
        page_contents,
        boundaries,
        pre_rendered_doc,
        has_font_encoding_issues,
        annotation_text_fallback,
        annotations,
        images,
        form_fields,
        extraction_warnings,
        page_labels,
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
///
/// Also returns the reordered text for each page individually (no markers or
/// separators), so callers can patch per-page assemblies — such as
/// `PageContent::content` — that are built independently of the joined
/// document text and would otherwise keep the original, unreordered text.
#[cfg(feature = "layout-detection")]
fn apply_reading_order_reordering(
    doc: &mut crate::pdf::native::NativeDocument,
    native_text: &str,
    layout_hints_per_page: &[Vec<crate::pdf::structure::types::LayoutHint>],
    page_config: Option<&crate::core::config::PageConfig>,
    margins: crate::pdf::native::text::PageMarginFractions,
) -> Result<(String, Vec<crate::types::PageBoundary>, Vec<String>)> {
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
            crate::pdf::native::text::extract_spans_from_page(&mut doc.doc, page_idx, margins).map_err(|e| {
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

        reordered_pages.push(reading_order::assemble_reading_order_text(&spans, &span_order));
    }

    if reordered_pages.is_empty() {
        return Ok((native_text.to_string(), Vec::new(), Vec::new()));
    }

    let (content, boundaries) = join_pages_with_boundaries(&reordered_pages, page_config);
    Ok((content, boundaries, reordered_pages))
}

/// Patch each page's `PageContent::content` (and `is_blank`) with its
/// reordered text.
///
/// `PageContent` is built independently of the joined `native_text`/
/// `boundaries` returned by `extract_text_and_metadata`'s own per-page split
/// (see `extract_text_from_native_document` in `pdf/native/text.rs`), so
/// reordering only the joined document text — as `apply_reading_order_reordering`
/// used to do before this patch existed — left every page's own `content`
/// field stuck with the original, unreordered per-page text. GH#1358: this is
/// why AUTO and ALWAYS reading-order modes returned byte-identical
/// `pages[].content` even though the top-level text was already being
/// reordered — the two were never wired together.
///
/// Pairs by position: both vectors are built by iterating `0..page_count` in
/// the same order, so a short read on either side (mismatched page counts)
/// simply leaves the excess pages on either side untouched rather than
/// panicking.
#[cfg(feature = "layout-detection")]
fn apply_reordered_text_to_page_contents(
    page_contents: &mut Option<Vec<PageContent>>,
    reordered_per_page: Vec<String>,
) {
    let Some(pages) = page_contents else {
        return;
    };
    for (page, reordered_text) in pages.iter_mut().zip(reordered_per_page) {
        page.is_blank = Some(crate::extraction::blank_detection::is_page_text_blank(&reordered_text));
        page.content = reordered_text;
    }
}

/// Join per-page texts, recording each page's byte range in the combined
/// string, faithful to how `extract_text_from_native_document` assembles it:
/// a rendered page marker before each page when `insert_page_markers` is on,
/// otherwise `"\n\n"` separators between pages. Markers and separators belong
/// to no page.
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
    use super::{
        hierarchy_cluster_count, needs_structured_extraction, page_has_exact_text_block,
        retain_segments_inside_page_margins, table_stage_failure_warning,
    };
    use crate::core::config::OutputFormat;

    #[test]
    fn should_match_multiline_annotation_as_contiguous_lines_inside_page_text() {
        assert!(page_has_exact_text_block(
            "HEADER\nLINE 1\nLINE 2\nFOOTER",
            "LINE 1\nLINE 2"
        ));
        assert!(!page_has_exact_text_block(
            "HEADER\nLINE 1\nUNRELATED\nLINE 2\nFOOTER",
            "LINE 1\nLINE 2"
        ));
    }

    #[test]
    fn should_use_public_hierarchy_default_for_implicit_structure() {
        let config = crate::core::config::ExtractionConfig::default();

        assert_eq!(
            hierarchy_cluster_count(&config),
            crate::core::config::HierarchyConfig::default().k_clusters
        );
    }

    #[test]
    fn should_filter_reused_hierarchy_segments_by_page_margins() {
        let segment = |text: &str, baseline_y: f32| crate::pdf::hierarchy::SegmentData {
            text: text.to_string(),
            x: 20.0,
            y: baseline_y,
            width: 100.0,
            height: 10.0,
            font_size: 10.0,
            is_bold: false,
            is_italic: false,
            is_monospace: false,
            baseline_y,
            rotation_degrees: 0.0,
            assigned_role: None,
        };
        let mut segments = vec![
            segment("header", 1000.01),
            segment("top boundary", 1000.0),
            segment("body", 500.0),
            segment("bottom boundary", 150.0),
            segment("footer", 149.99),
        ];

        retain_segments_inside_page_margins(
            &mut segments,
            100.0,
            1100.0,
            crate::pdf::native::text::PageMarginFractions {
                top: 0.10,
                bottom: 0.05,
            },
        );

        assert_eq!(
            segments.iter().map(|segment| segment.text.as_str()).collect::<Vec<_>>(),
            ["top boundary", "body", "bottom boundary"]
        );
    }

    /// Characterises `table_stage_failure_warning`'s message format only — it constructs the
    /// error directly rather than driving a real xberg_native_pdf failure, so it does NOT prove any
    /// table-extraction pass can fail or panic. That coverage used to live in
    /// `crates/xberg/tests/issue_603_pdf_table_stage_failure_warning.rs` as
    /// `should_emit_pdf_tables_warning_when_whole_document_table_pass_panics`, driven through the
    /// public API against `test_documents/pdf/total_order_panic_1198_tables_path.pdf`. That
    /// fixture stopped panicking once the `xberg-native` fork picked up upstream commit
    /// `9b0f9c99` ("Fix reading-order sort panics on scanned/malformed PDFs (#807)"), which
    /// replaced the non-transitive pairwise tategaki column comparator with
    /// `sort_vertical_tategaki`, a genuine total order — `9b0f9c99` is an ancestor of the
    /// `xberg-native` 1.0.1 version this crate now consumes. No fixture in `test_documents/`
    /// still reaches a real `xberg_native_pdf` panic through the table-detection stage, so the old test
    /// asserted a panic-recovery warning against code that no longer panics. Do not re-add a
    /// test that asserts a panic on this fixture.
    #[test]
    fn table_stage_failure_warning_formats_the_message_it_promises() {
        let cause = "xberg_native_pdf panicked during table extraction: boom";
        let warning = table_stage_failure_warning("whole-document", &cause);

        assert_eq!(warning.source, "pdf_tables");
        let message = warning.message.as_ref();
        assert_eq!(
            message,
            "whole-document table extraction failed for the document: xberg_native_pdf panicked during table \
             extraction: boom; tables from this pass were skipped"
        );
    }

    /// Regression test for the defective `Custom(_)` catch-all shipped in #1388:
    /// `OutputFormat::FromStr` never rejects an unknown string, so a typo like
    /// `Custom("markdwon")` must not trigger the expensive native hierarchy pass —
    /// it can only ever fall back to plain text (`custom_fallback_to_plain` in
    /// `core/pipeline/format.rs`), so running the full structure pass for it was
    /// pure waste.
    #[test]
    fn should_not_trigger_structured_extraction_for_unregistered_custom_format() {
        let output_format = OutputFormat::Custom("markdwon".to_string());
        assert!(!needs_structured_extraction(false, &output_format, false, false));
    }

    /// `DocTags` is a real, always-registered built-in renderer (see
    /// `plugins::registry::renderer::RendererRegistry`) and needs the same
    /// geometry/headings as Markdown, Djot, and HTML.
    #[test]
    fn should_trigger_structured_extraction_for_doctags_format() {
        assert!(needs_structured_extraction(false, &OutputFormat::DocTags, false, false));
    }

    /// The pre-existing markup formats must keep triggering the structured path.
    #[test]
    fn should_trigger_structured_extraction_for_markdown_djot_and_html() {
        assert!(needs_structured_extraction(
            false,
            &OutputFormat::Markdown,
            false,
            false
        ));
        assert!(needs_structured_extraction(false, &OutputFormat::Djot, false, false));
        assert!(needs_structured_extraction(false, &OutputFormat::Html, false, false));
    }

    /// `Plain` and `Json` must not trigger the structured path on their own.
    #[test]
    fn should_not_trigger_structured_extraction_for_plain_or_json() {
        assert!(!needs_structured_extraction(false, &OutputFormat::Plain, false, false));
        assert!(!needs_structured_extraction(false, &OutputFormat::Json, false, false));
    }

    /// Hierarchy, inline-image OCR, and explicit content filtering require the
    /// structured path regardless of output format.
    #[test]
    fn should_trigger_structured_extraction_when_structure_dependent_options_are_enabled() {
        assert!(needs_structured_extraction(true, &OutputFormat::Plain, false, false));
        assert!(needs_structured_extraction(false, &OutputFormat::Plain, true, false));
        assert!(needs_structured_extraction(false, &OutputFormat::Plain, false, true));
    }

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

    /// Regression test for GH#1358: `pages[].content` is a per-page assembly
    /// built independently of the joined document text (see
    /// `extract_text_and_metadata`'s own page split in `pdf/native/text.rs`),
    /// so a fix that only reorders the joined text — leaving this wiring
    /// out — cannot repair rotated-text scrambling reported through the
    /// per-page API. This asserts each `PageContent::content` takes the
    /// reordered per-page text (here, a rotated run reassembled by
    /// `assemble_reading_order_text` in a prior pass), not the original
    /// scrambled text produced by `extract_text_and_metadata`.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_patch_page_content_with_reordered_text_not_leave_it_scrambled() {
        use crate::types::PageContent;

        let mut page_contents = Some(vec![
            PageContent {
                page_number: 1,
                content: "the meet only need oil Engine".to_string(),
                tables: Vec::new(),
                image_indices: Vec::new(),
                hierarchy: None,
                is_blank: Some(false),
                layout_regions: None,
                speaker_notes: None,
                section_name: None,
                sheet_name: None,
                ocr_confidence: None,
                image_preprocessing: None,
            },
            PageContent {
                page_number: 2,
                content: "stale second-page text".to_string(),
                tables: Vec::new(),
                image_indices: Vec::new(),
                hierarchy: None,
                is_blank: Some(false),
                layout_regions: None,
                speaker_notes: None,
                section_name: None,
                sheet_name: None,
                ocr_confidence: None,
                image_preprocessing: None,
            },
        ]);
        let reordered_per_page = vec![
            "Engine oil need only meet the".to_string(),
            "reordered second-page text".to_string(),
        ];

        super::apply_reordered_text_to_page_contents(&mut page_contents, reordered_per_page);

        let pages = page_contents.expect("page_contents must stay Some");
        assert_eq!(
            pages[0].content, "Engine oil need only meet the",
            "page 1 content must take the reordered text, not the scrambled original"
        );
        assert_eq!(
            pages[1].content, "reordered second-page text",
            "page 2 content must also be patched, independent of page 1"
        );
        assert_eq!(pages[0].page_number, 1, "page identity must be preserved");
        assert_eq!(pages[1].page_number, 2, "page identity must be preserved");
    }

    /// `page_contents == None` (per-page tracking disabled) must be a no-op,
    /// not a panic.
    #[cfg(feature = "layout-detection")]
    #[test]
    fn should_leave_none_page_contents_untouched() {
        let mut page_contents: Option<Vec<crate::types::PageContent>> = None;

        super::apply_reordered_text_to_page_contents(&mut page_contents, vec!["ignored".to_string()]);

        assert!(page_contents.is_none(), "None page_contents must stay None");
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
