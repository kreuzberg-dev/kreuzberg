//! Per-region VLM extraction for PDF layout regions.
//!
//! When `vlm_fallback != Disabled` and layout detection has identified
//! `Picture` (figure), `Table` (dense table), or `Other` (unclassified complex
//! layout) regions, this module crops those regions from the page raster, calls
//! the VLM via [`crate::llm::region_extractor`], and splices the resulting
//! markdown back into the assembled document at the region's original position.
//!
//! Only compiled when `liter-llm` + `layout-detection` are both enabled
//! (not on Windows).

#![cfg(all(feature = "liter-llm", feature = "layout-detection"))]

use std::io::Cursor;

use image::{ExtendedColorType, ImageEncoder};

use crate::core::config::LlmConfig;
use crate::llm::region_extractor::{RegionKind, extract_region_with_vlm};
use crate::pdf::structure::types::{LayoutHint, LayoutHintClass};

/// Minimum confidence threshold for a layout hint to trigger VLM region extraction.
const MIN_REGION_CONFIDENCE: f32 = 0.6;

/// Minimum pixel area for a cropped region to be sent to VLM.
///
/// Regions smaller than this are skipped — they are unlikely to contain meaningful
/// content and would waste VLM API tokens.
const MIN_REGION_PIXEL_AREA: u32 = 1_000;

/// A VLM-extracted markdown string and the page index it belongs to.
pub(crate) struct RegionVlmResult {
    /// 0-based page index.
    pub page_index: usize,
    /// Markdown text from the VLM.
    pub markdown: String,
    /// The layout hint that triggered this extraction. Used by [`inject_region_results`]
    /// to splice the result at the region's original position rather than at the end
    /// of the page.
    pub hint: LayoutHint,
}

/// Maps a layout hint class to the [`RegionKind`] that should handle it via VLM,
/// or `None` if the class should stay on the classical extraction path.
///
/// - [`LayoutHintClass::Picture`] → [`RegionKind::Figure`]: figures/diagrams never
///   have usable classical text.
/// - [`LayoutHintClass::Table`] → [`RegionKind::DenseTable`]: classical table
///   reconstruction is prone to garbling dense/borderless tables.
/// - [`LayoutHintClass::Other`] → [`RegionKind::ComplexLayout`]: by definition, a
///   region the classical layout classifier could not categorise.
///
/// Wrapper classes with dedicated classical handling (`Form`, `KeyValueRegion`,
/// `DocumentIndex`) are intentionally excluded to avoid double-processing regions
/// that already extract cleanly.
const fn region_kind_for_hint(class_name: LayoutHintClass) -> Option<RegionKind> {
    match class_name {
        LayoutHintClass::Picture => Some(RegionKind::Figure),
        LayoutHintClass::Table => Some(RegionKind::DenseTable),
        LayoutHintClass::Other => Some(RegionKind::ComplexLayout),
        _ => None,
    }
}

/// Extract markdown from VLM for all figure / complex-layout regions across all pages.
///
/// Iterates the layout hints for each page. For hints whose class warrants VLM
/// extraction ([`LayoutHintClass::Picture`] and, when requested, dense tables),
/// crops the region from the page raster image, encodes it as PNG, and calls
/// [`extract_region_with_vlm`]. Results from all pages are collected and returned.
///
/// # Arguments
///
/// * `layout_images` — One rasterised `RgbImage` per page (same order as `layout_hints`).
/// * `layout_hints` — Per-page layout detection results, each a `Vec<LayoutHint>`.
/// * `llm_config` — LLM provider configuration for the VLM calls.
///
/// # Returns
///
/// A `Vec<RegionVlmResult>` with one entry per successfully extracted region.
/// Errors from individual regions are logged as warnings but do not propagate —
/// a VLM failure on one region must not abort the whole extraction.
pub(crate) async fn extract_vlm_regions(
    layout_images: &[image::RgbImage],
    layout_hints: &[Vec<LayoutHint>],
    llm_config: &LlmConfig,
) -> Vec<RegionVlmResult> {
    let mut results: Vec<RegionVlmResult> = Vec::new();

    for (page_index, (page_image, hints)) in layout_images.iter().zip(layout_hints.iter()).enumerate() {
        let img_width = page_image.width();
        let img_height = page_image.height();

        for hint in hints {
            if hint.confidence < MIN_REGION_CONFIDENCE {
                continue;
            }

            let Some(region_kind) = region_kind_for_hint(hint.class_name) else {
                continue;
            };

            let pdf_top = hint.top;
            let pdf_bottom = hint.bottom;
            let pdf_left = hint.left;
            let pdf_right = hint.right;

            let pixel_y1 = (img_height as f32 - pdf_top).max(0.0).min(img_height as f32) as u32;
            let pixel_y2 = (img_height as f32 - pdf_bottom).max(0.0).min(img_height as f32) as u32;
            let pixel_x1 = pdf_left.max(0.0).min(img_width as f32) as u32;
            let pixel_x2 = pdf_right.max(0.0).min(img_width as f32) as u32;

            let (y_top, y_bot) = if pixel_y1 <= pixel_y2 {
                (pixel_y1, pixel_y2)
            } else {
                (pixel_y2, pixel_y1)
            };
            let (x_left, x_right) = if pixel_x1 <= pixel_x2 {
                (pixel_x1, pixel_x2)
            } else {
                (pixel_x2, pixel_x1)
            };

            let crop_w = x_right.saturating_sub(x_left);
            let crop_h = y_bot.saturating_sub(y_top);

            if crop_w * crop_h < MIN_REGION_PIXEL_AREA {
                tracing::trace!(
                    page = page_index,
                    crop_w,
                    crop_h,
                    "region too small for VLM extraction; skipping"
                );
                continue;
            }

            let crop = image::imageops::crop_imm(page_image, x_left, y_top, crop_w, crop_h).to_image();

            let mut png_buf = Cursor::new(Vec::<u8>::new());
            let encode_result = image::codecs::png::PngEncoder::new(&mut png_buf).write_image(
                crop.as_raw(),
                crop.width(),
                crop.height(),
                ExtendedColorType::Rgb8,
            );
            if let Err(e) = encode_result {
                tracing::warn!(
                    page = page_index,
                    error = %e,
                    "failed to PNG-encode region crop; skipping VLM call"
                );
                continue;
            }
            let crop_bytes = png_buf.into_inner();

            tracing::debug!(
                page = page_index,
                region_kind = ?region_kind,
                confidence = hint.confidence,
                crop_w,
                crop_h,
                "sending region to VLM"
            );

            match extract_region_with_vlm(&crop_bytes, "image/png", region_kind, llm_config, None).await {
                Ok(markdown) => {
                    let trimmed = markdown.trim().to_string();
                    if !trimmed.is_empty() {
                        results.push(RegionVlmResult {
                            page_index,
                            markdown: trimmed,
                            hint: hint.clone(),
                        });
                    }
                }
                Err(e) => {
                    tracing::warn!(
                        page = page_index,
                        region_kind = ?region_kind,
                        error = %e,
                        "VLM region extraction failed; region suppressed"
                    );
                }
            }
        }
    }

    results
}

/// Finds the index in `elements` at which a region with top edge `hint_top` (PDF
/// coordinate space, y=0 at page bottom) belonging to `page_num` should be spliced.
///
/// Scans elements belonging to `page_num` and returns the index of the first one
/// whose bounding box starts below `hint_top` (i.e. the region sits above it in
/// reading order) — the new element is inserted right before that element. If no
/// element on the page has a bbox positioned below the region, the region is
/// inserted immediately after the last element of that page. If the page has no
/// elements at all, it is inserted right before the first element of the next
/// page, or at the end of the document if there is none.
fn find_insertion_index(elements: &[crate::types::internal::InternalElement], page_num: u32, hint_top: f32) -> usize {
    let mut insert_at = elements.len();
    for (index, element) in elements.iter().enumerate() {
        match element.page {
            Some(page) if page == page_num => {
                if let Some(bbox) = element.bbox
                    && (bbox.y1 as f32) < hint_top
                {
                    return index;
                }
                insert_at = index + 1;
            }
            Some(page) if page > page_num => return insert_at.min(index),
            _ => {}
        }
    }
    insert_at
}

/// Shifts every `Relationship` index at or past `from_index` by one position.
///
/// `InternalDocument::elements` is documented as append-only during extraction
/// precisely because `Relationship::source` and `RelationshipTarget::Index` store
/// raw positions into that vector (e.g. caption-to-figure links set up in
/// `pdf::structure::assembly`). Splicing a new element into the middle of
/// `elements` — as [`inject_region_results`] does to land VLM output at its
/// anchor — must shift those already-recorded indices in lockstep, or the
/// relationship silently starts pointing at the wrong element.
fn shift_relationship_indices(document: &mut crate::types::internal::InternalDocument, from_index: usize) {
    use crate::types::internal::RelationshipTarget;

    let Ok(from_index) = u32::try_from(from_index) else {
        return;
    };
    for relationship in &mut document.relationships {
        if relationship.source >= from_index {
            relationship.source += 1;
        }
        if let RelationshipTarget::Index(target_index) = &mut relationship.target
            && *target_index >= from_index
        {
            *target_index += 1;
        }
    }
}

/// Inject VLM-extracted region markdown into an `InternalDocument`.
///
/// For each `RegionVlmResult`, splices a paragraph element carrying the VLM markdown
/// into `document.elements` at the position of the originating layout hint, using
/// [`find_insertion_index`] to locate it among the page's existing elements by
/// bounding-box geometry, rather than appending it after the page's content.
/// Existing `Relationship` indices are shifted via [`shift_relationship_indices`]
/// so caption/link relationships recorded before the splice keep pointing at the
/// correct element.
pub(crate) fn inject_region_results(
    document: &mut crate::types::internal::InternalDocument,
    results: Vec<RegionVlmResult>,
) {
    use crate::types::internal::{ElementKind, InternalElement};

    for result in results {
        let page_num = (result.page_index + 1) as u32;
        let insert_at = find_insertion_index(&document.elements, page_num, result.hint.top);
        shift_relationship_indices(document, insert_at);
        document.elements.insert(
            insert_at,
            InternalElement::text(ElementKind::Paragraph, result.markdown, 0).with_page(page_num),
        );
        tracing::debug!(page = page_num, insert_at, "injected VLM region result into document");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::structure::types::LayoutHintClass;

    fn make_hint(class: LayoutHintClass, confidence: f32) -> LayoutHint {
        LayoutHint {
            class_name: class,
            confidence,
            left: 50.0,
            bottom: 600.0,
            right: 400.0,
            top: 750.0,
        }
    }

    #[test]
    fn test_low_confidence_hints_are_skipped() {
        let hint = make_hint(LayoutHintClass::Picture, 0.3);
        assert!(hint.confidence < MIN_REGION_CONFIDENCE);
    }

    #[test]
    fn should_skip_hints_with_dedicated_classical_handling() {
        let classically_handled = [
            LayoutHintClass::Text,
            LayoutHintClass::SectionHeader,
            LayoutHintClass::Title,
            LayoutHintClass::PageHeader,
            LayoutHintClass::PageFooter,
            LayoutHintClass::Caption,
            LayoutHintClass::Code,
            LayoutHintClass::Formula,
            LayoutHintClass::Footnote,
            LayoutHintClass::ListItem,
            LayoutHintClass::Form,
            LayoutHintClass::KeyValueRegion,
            LayoutHintClass::DocumentIndex,
        ];
        for class in classically_handled {
            assert_eq!(
                region_kind_for_hint(class),
                None,
                "{class:?} must not be routed to the VLM"
            );
        }
    }

    #[test]
    fn should_route_dense_table_hint_to_dense_table_region_kind() {
        assert_eq!(
            region_kind_for_hint(LayoutHintClass::Table),
            Some(RegionKind::DenseTable)
        );
    }

    #[test]
    fn should_route_other_hint_to_complex_layout_region_kind() {
        assert_eq!(
            region_kind_for_hint(LayoutHintClass::Other),
            Some(RegionKind::ComplexLayout)
        );
    }

    #[test]
    fn should_route_picture_hint_to_figure_region_kind() {
        assert_eq!(region_kind_for_hint(LayoutHintClass::Picture), Some(RegionKind::Figure));
    }

    #[test]
    fn test_min_pixel_area_constant() {
        const { assert!(MIN_REGION_PIXEL_AREA > 0) };
    }

    fn element_with_bbox(page: u32, top: f64) -> crate::types::internal::InternalElement {
        use crate::types::BoundingBox;
        use crate::types::internal::{ElementKind, InternalElement};

        let mut element = InternalElement::text(ElementKind::Paragraph, "existing", 0);
        element.page = Some(page);
        element.bbox = Some(BoundingBox {
            x0: 0.0,
            y0: top - 10.0,
            x1: 100.0,
            y1: top,
        });
        element
    }

    fn hint_with_top(top: f32) -> LayoutHint {
        LayoutHint {
            class_name: LayoutHintClass::Table,
            confidence: 0.9,
            left: 50.0,
            bottom: top - 100.0,
            right: 400.0,
            top,
        }
    }

    #[test]
    fn should_splice_vlm_result_at_its_bbox_anchor_not_at_document_end() {
        use crate::types::internal::InternalDocument;

        let mut document = InternalDocument::default();
        // Page 1: a paragraph above the region (top=750) and one below it (top=400).
        document.elements.push(element_with_bbox(1, 750.0));
        document.elements.push(element_with_bbox(1, 400.0));
        // Page 2: an unrelated paragraph, which must stay after the injected result.
        document.elements.push(element_with_bbox(2, 700.0));

        let results = vec![RegionVlmResult {
            page_index: 0,
            markdown: "VLM TABLE".to_string(),
            hint: hint_with_top(600.0),
        }];

        inject_region_results(&mut document, results);

        assert_eq!(document.elements.len(), 4);
        assert_eq!(document.elements[0].text, "existing");
        assert_eq!(document.elements[0].page, Some(1));
        assert_eq!(document.elements[1].text, "VLM TABLE");
        assert_eq!(document.elements[1].page, Some(1));
        assert_eq!(document.elements[2].text, "existing");
        assert_eq!(document.elements[2].page, Some(1));
        assert_eq!(document.elements[3].page, Some(2));
    }

    #[test]
    fn should_append_vlm_result_after_page_when_no_element_bbox_is_below_it() {
        use crate::types::internal::InternalDocument;

        let mut document = InternalDocument::default();
        document.elements.push(element_with_bbox(1, 750.0));
        document.elements.push(element_with_bbox(1, 700.0));

        let results = vec![RegionVlmResult {
            page_index: 0,
            markdown: "VLM CAPTION".to_string(),
            hint: hint_with_top(600.0),
        }];

        inject_region_results(&mut document, results);

        assert_eq!(document.elements.len(), 3);
        assert_eq!(document.elements[2].text, "VLM CAPTION");
    }

    #[test]
    fn should_shift_relationship_indices_when_splicing_before_referenced_elements() {
        use crate::types::internal::{InternalDocument, Relationship, RelationshipKind, RelationshipTarget};

        let mut document = InternalDocument::default();
        // index 0: paragraph above the region (stays before the splice point)
        document.elements.push(element_with_bbox(1, 750.0));
        // index 1: paragraph below the region (splice must land before this one)
        document.elements.push(element_with_bbox(1, 400.0));
        // index 2: unrelated element on page 2
        document.elements.push(element_with_bbox(2, 700.0));

        // A caption relationship recorded before splicing, pointing at raw indices.
        document.relationships.push(Relationship {
            source: 1,
            target: RelationshipTarget::Index(2),
            kind: RelationshipKind::Caption,
        });
        // A relationship entirely before the splice point must be left untouched.
        document.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Key("unresolved".to_string()),
            kind: RelationshipKind::InternalLink,
        });

        let results = vec![RegionVlmResult {
            page_index: 0,
            markdown: "VLM TABLE".to_string(),
            hint: hint_with_top(600.0),
        }];

        inject_region_results(&mut document, results);

        // The new element was inserted at index 1, so indices >= 1 shift by one.
        assert_eq!(document.elements[1].text, "VLM TABLE");
        assert_eq!(document.elements[2].text, "existing");
        assert_eq!(document.relationships[0].source, 2, "source index 1 must shift to 2");
        assert_eq!(
            document.relationships[0].target,
            RelationshipTarget::Index(3),
            "target index 2 must shift to 3"
        );
        assert_eq!(
            document.relationships[1].source, 0,
            "relationship before the splice point must not shift"
        );
        assert_eq!(
            document.relationships[1].target,
            RelationshipTarget::Key("unresolved".to_string()),
            "unresolved key targets are untouched"
        );
    }
}
