//! Annotation extraction using the pdf_oxide backend.
//!
//! Maps pdf_oxide's `Annotation` types to Xberg's `PdfAnnotation` model,
//! extracting content text, bounding boxes, and link URIs.

use super::OxideDocument;
use crate::types::{BoundingBox, PdfAnnotation, PdfAnnotationType, ProcessingWarning};

/// Extract annotations from all pages of a PDF document using pdf_oxide.
///
/// Iterates over every page and every annotation on each page, mapping
/// pdf_oxide annotation subtypes to [`PdfAnnotationType`] and collecting
/// content text and bounding boxes where available.
///
/// Widget (form field) and Popup annotations are skipped as they are not
/// user-facing content annotations.
///
/// # Arguments
///
/// * `doc` - Mutable reference to the oxide document
///
/// # Returns
///
/// A `Vec<PdfAnnotation>` containing all successfully extracted annotations, and a
/// `Vec<ProcessingWarning>` describing any pages whose annotations could not be
/// read (issue #72). When the document's page count itself cannot be determined,
/// annotations are empty and a single warning is returned.
pub(crate) fn extract_annotations(doc: &mut OxideDocument) -> (Vec<PdfAnnotation>, Vec<ProcessingWarning>) {
    let page_count = match doc.doc.page_count() {
        Ok(count) => count,
        Err(e) => {
            tracing::debug!("pdf_oxide: failed to get page count for annotations: {e}");
            return (Vec::new(), vec![page_count_failure_warning(&e)]);
        }
    };

    let mut annotations = Vec::new();
    let mut warnings = Vec::new();

    for page_index in 0..page_count {
        let page_number = (page_index + 1) as u32;

        let page_annotations = match doc.doc.get_annotations(page_index) {
            Ok(annots) => annots,
            Err(e) => {
                tracing::debug!(page = page_index, "pdf_oxide: failed to get annotations: {e}");
                warnings.push(page_annotations_failure_warning(page_number, &e));
                continue;
            }
        };

        for annot in page_annotations {
            if matches!(
                annot.subtype_enum,
                pdf_oxide::AnnotationSubtype::Widget | pdf_oxide::AnnotationSubtype::Popup
            ) {
                continue;
            }

            let annotation_type = map_annotation_subtype(annot.subtype_enum);

            let content = extract_annotation_content(&annot);

            let bounding_box = annot.rect.map(|rect| BoundingBox {
                x0: rect[0],
                y0: rect[1],
                x1: rect[2],
                y1: rect[3],
            });

            let quad_points: Option<Vec<BoundingBox>> = annot
                .quad_points
                .as_ref()
                .map(|quads| quads.iter().map(quad_to_bounding_box).collect());

            let marked_text = if is_text_markup(annot.subtype_enum) {
                quad_points
                    .as_deref()
                    .and_then(|boxes| extract_marked_text(&doc.doc, page_index, boxes))
            } else {
                None
            };

            annotations.push(PdfAnnotation {
                annotation_type,
                content,
                page_number,
                bounding_box,
                author: annot.author.clone().filter(|s| !s.is_empty()),
                modified: annot.modification_date.clone().filter(|s| !s.is_empty()),
                color: annot.color.as_deref().and_then(color_to_hex),
                subject: annot.subject.clone().filter(|s| !s.is_empty()),
                quad_points,
                marked_text,
            });
        }
    }

    (annotations, warnings)
}

/// Whether an annotation subtype marks up existing page text via `/QuadPoints`
/// (Highlight, Underline, StrikeOut, Squiggly).
fn is_text_markup(subtype: pdf_oxide::AnnotationSubtype) -> bool {
    matches!(
        subtype,
        pdf_oxide::AnnotationSubtype::Highlight
            | pdf_oxide::AnnotationSubtype::Underline
            | pdf_oxide::AnnotationSubtype::StrikeOut
            | pdf_oxide::AnnotationSubtype::Squiggly
    )
}

/// Reduce a `/QuadPoints` quad (`x1,y1, x2,y2, x3,y3, x4,y4`) to its axis-aligned
/// bounding box. The PDF spec does not fix the winding order of the four
/// points, so the min/max of each axis is taken rather than assuming a
/// particular corner order.
fn quad_to_bounding_box(quad: &[f64; 8]) -> BoundingBox {
    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];
    BoundingBox {
        x0: xs.iter().copied().fold(f64::INFINITY, f64::min),
        y0: ys.iter().copied().fold(f64::INFINITY, f64::min),
        x1: xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
        y1: ys.iter().copied().fold(f64::NEG_INFINITY, f64::max),
    }
}

/// Recover the page text underneath a text markup annotation's `/QuadPoints`
/// boxes, one per marked line/run, joined with a single space.
///
/// Returns `None` when no box yields any text (e.g. the region only covers
/// whitespace, or `pdf_oxide` fails to extract text for every box).
fn extract_marked_text(doc: &pdf_oxide::PdfDocument, page_index: usize, boxes: &[BoundingBox]) -> Option<String> {
    let mut pieces = Vec::with_capacity(boxes.len());

    for bbox in boxes {
        let region =
            pdf_oxide::geometry::Rect::from_points(bbox.x0 as f32, bbox.y0 as f32, bbox.x1 as f32, bbox.y1 as f32);

        match doc.extract_text_in_rect(page_index, region, pdf_oxide::layout::RectFilterMode::Intersects) {
            Ok(text) => {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    pieces.push(trimmed.to_string());
                }
            }
            Err(e) => {
                tracing::debug!(
                    page = page_index,
                    "pdf_oxide: failed to extract marked text for annotation: {e}"
                );
            }
        }
    }

    if pieces.is_empty() {
        None
    } else {
        Some(pieces.join(" "))
    }
}

/// Convert a PDF `/C` colour array (DeviceGray, DeviceRGB, or DeviceCMYK) to a
/// CSS-compatible `#rrggbb` hex string.
fn color_to_hex(components: &[f64]) -> Option<String> {
    let to_u8 = |v: f64| -> u8 { (v.clamp(0.0, 1.0) * 255.0).round() as u8 };

    match components {
        [gray] => {
            let g = to_u8(*gray);
            Some(format!("#{g:02x}{g:02x}{g:02x}"))
        }
        [r, g, b] => Some(format!("#{:02x}{:02x}{:02x}", to_u8(*r), to_u8(*g), to_u8(*b))),
        [c, m, y, k] => {
            let r = to_u8((1.0 - c) * (1.0 - k));
            let g = to_u8((1.0 - m) * (1.0 - k));
            let b = to_u8((1.0 - y) * (1.0 - k));
            Some(format!("#{r:02x}{g:02x}{b:02x}"))
        }
        _ => None,
    }
}

/// Build the warning for issue #72's document-wide failure mode: the page count
/// itself could not be determined, so no page could even be attempted.
fn page_count_failure_warning(error: &pdf_oxide::Error) -> ProcessingWarning {
    ProcessingWarning {
        source: std::borrow::Cow::Borrowed("pdf_annotations"),
        message: std::borrow::Cow::Owned(format!(
            "annotation extraction failed: could not determine page count ({error}); no annotations were extracted"
        )),
    }
}

/// Build the warning for issue #72's per-page failure mode: annotations on one
/// page could not be read, but the rest of the document is still processed.
fn page_annotations_failure_warning(page_number: u32, error: &pdf_oxide::Error) -> ProcessingWarning {
    ProcessingWarning {
        source: std::borrow::Cow::Borrowed("pdf_annotations"),
        message: std::borrow::Cow::Owned(format!(
            "annotation extraction failed for page {page_number}: {error}; annotations on this page were skipped"
        )),
    }
}

/// Map a pdf_oxide annotation subtype to Xberg's `PdfAnnotationType`.
fn map_annotation_subtype(subtype: pdf_oxide::AnnotationSubtype) -> PdfAnnotationType {
    match subtype {
        pdf_oxide::AnnotationSubtype::Text | pdf_oxide::AnnotationSubtype::FreeText => PdfAnnotationType::Text,
        pdf_oxide::AnnotationSubtype::Highlight => PdfAnnotationType::Highlight,
        pdf_oxide::AnnotationSubtype::Link => PdfAnnotationType::Link,
        pdf_oxide::AnnotationSubtype::Stamp => PdfAnnotationType::Stamp,
        pdf_oxide::AnnotationSubtype::Underline => PdfAnnotationType::Underline,
        pdf_oxide::AnnotationSubtype::StrikeOut => PdfAnnotationType::StrikeOut,
        pdf_oxide::AnnotationSubtype::Squiggly => PdfAnnotationType::Squiggly,
        pdf_oxide::AnnotationSubtype::Ink => PdfAnnotationType::Ink,
        pdf_oxide::AnnotationSubtype::Square => PdfAnnotationType::Square,
        pdf_oxide::AnnotationSubtype::Circle => PdfAnnotationType::Circle,
        pdf_oxide::AnnotationSubtype::Polygon => PdfAnnotationType::Polygon,
        pdf_oxide::AnnotationSubtype::PolyLine => PdfAnnotationType::PolyLine,
        pdf_oxide::AnnotationSubtype::Line => PdfAnnotationType::Line,
        pdf_oxide::AnnotationSubtype::Caret => PdfAnnotationType::Caret,
        pdf_oxide::AnnotationSubtype::FileAttachment => PdfAnnotationType::FileAttachment,
        pdf_oxide::AnnotationSubtype::Sound => PdfAnnotationType::Sound,
        pdf_oxide::AnnotationSubtype::Movie => PdfAnnotationType::Movie,
        _ => PdfAnnotationType::Other,
    }
}

/// Extract content text from a pdf_oxide annotation.
///
/// For Link annotations, attempts to retrieve the URI from the associated
/// action. Falls back to the generic `contents` field for all types.
fn extract_annotation_content(annot: &pdf_oxide::Annotation) -> Option<String> {
    if annot.subtype_enum == pdf_oxide::AnnotationSubtype::Link
        && let Some(ref action) = annot.action
    {
        match action {
            pdf_oxide::LinkAction::Uri(uri) if !uri.is_empty() => {
                return Some(uri.clone());
            }
            _ => {}
        }
    }

    annot.contents.as_ref().filter(|s| !s.is_empty()).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #72: a document-wide page-count failure must produce a
    /// `ProcessingWarning` (not just a `tracing::debug!` line the caller can
    /// never see) naming the root cause.
    #[test]
    fn test_page_count_failure_warning_names_root_cause() {
        let error = pdf_oxide::Error::InvalidPdf("corrupt xref".to_string());
        let warning = page_count_failure_warning(&error);

        assert_eq!(warning.source.as_ref(), "pdf_annotations");
        assert_eq!(
            warning.message.as_ref(),
            "annotation extraction failed: could not determine page count (Invalid PDF: corrupt xref); \
             no annotations were extracted"
        );
    }

    /// Issue #72: a single page's annotation-read failure must produce a
    /// `ProcessingWarning` naming that page, while extraction continues.
    #[test]
    fn test_page_annotations_failure_warning_names_page() {
        let error = pdf_oxide::Error::InvalidPdf("malformed /Annots array".to_string());
        let warning = page_annotations_failure_warning(3, &error);

        assert_eq!(warning.source.as_ref(), "pdf_annotations");
        assert_eq!(
            warning.message.as_ref(),
            "annotation extraction failed for page 3: Invalid PDF: malformed /Annots array; \
             annotations on this page were skipped"
        );
    }

    #[test]
    fn test_map_annotation_subtype_text() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Text),
            PdfAnnotationType::Text
        );
    }

    #[test]
    fn test_map_annotation_subtype_free_text() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::FreeText),
            PdfAnnotationType::Text
        );
    }

    #[test]
    fn test_map_annotation_subtype_highlight() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Highlight),
            PdfAnnotationType::Highlight
        );
    }

    #[test]
    fn test_map_annotation_subtype_link() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Link),
            PdfAnnotationType::Link
        );
    }

    #[test]
    fn test_map_annotation_subtype_stamp() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Stamp),
            PdfAnnotationType::Stamp
        );
    }

    #[test]
    fn test_map_annotation_subtype_underline() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Underline),
            PdfAnnotationType::Underline
        );
    }

    #[test]
    fn test_map_annotation_subtype_strikeout() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::StrikeOut),
            PdfAnnotationType::StrikeOut
        );
    }

    #[test]
    fn test_map_annotation_subtype_other() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Screen),
            PdfAnnotationType::Other
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Watermark),
            PdfAnnotationType::Other
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Redact),
            PdfAnnotationType::Other
        );
    }

    /// Issue #63: subtypes that were previously collapsed into `Other` now each
    /// round-trip as their own distinct variant.
    #[test]
    fn test_map_annotation_subtype_previously_collapsed_variants() {
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Ink),
            PdfAnnotationType::Ink
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Square),
            PdfAnnotationType::Square
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Circle),
            PdfAnnotationType::Circle
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Polygon),
            PdfAnnotationType::Polygon
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::PolyLine),
            PdfAnnotationType::PolyLine
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Squiggly),
            PdfAnnotationType::Squiggly
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Caret),
            PdfAnnotationType::Caret
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::FileAttachment),
            PdfAnnotationType::FileAttachment
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Sound),
            PdfAnnotationType::Sound
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Movie),
            PdfAnnotationType::Movie
        );
        assert_eq!(
            map_annotation_subtype(pdf_oxide::AnnotationSubtype::Line),
            PdfAnnotationType::Line
        );
    }

    #[test]
    fn test_color_to_hex_gray() {
        assert_eq!(color_to_hex(&[0.5]), Some("#808080".to_string()));
    }

    #[test]
    fn test_color_to_hex_rgb() {
        assert_eq!(color_to_hex(&[1.0, 1.0, 0.0]), Some("#ffff00".to_string()));
    }

    #[test]
    fn test_color_to_hex_cmyk() {
        // Pure black in CMYK (K=1) converts to RGB black.
        assert_eq!(color_to_hex(&[0.0, 0.0, 0.0, 1.0]), Some("#000000".to_string()));
    }

    #[test]
    fn test_color_to_hex_invalid_length_returns_none() {
        assert_eq!(color_to_hex(&[0.1, 0.2]), None);
        assert_eq!(color_to_hex(&[]), None);
    }

    #[test]
    fn test_quad_to_bounding_box_computes_min_max() {
        let quad = [10.0, 20.0, 30.0, 20.0, 10.0, 40.0, 30.0, 40.0];
        let bbox = quad_to_bounding_box(&quad);
        assert_eq!(bbox.x0, 10.0);
        assert_eq!(bbox.y0, 20.0);
        assert_eq!(bbox.x1, 30.0);
        assert_eq!(bbox.y1, 40.0);
    }

    #[test]
    fn test_extract_annotation_content_uri() {
        let annot = pdf_oxide::Annotation {
            annotation_type: "Annot".to_string(),
            subtype: Some("Link".to_string()),
            subtype_enum: pdf_oxide::AnnotationSubtype::Link,
            contents: None,
            rect: None,
            author: None,
            creation_date: None,
            modification_date: None,
            subject: None,
            destination: None,
            action: Some(pdf_oxide::LinkAction::Uri("https://example.com".to_string())),
            quad_points: None,
            color: None,
            opacity: None,
            flags: pdf_oxide::AnnotationFlags::empty(),
            border: None,
            interior_color: None,
            field_type: None,
            field_name: None,
            field_value: None,
            default_value: None,
            field_flags: None,
            options: None,
            appearance_state: None,
            raw_dict: None,
        };

        let content = extract_annotation_content(&annot);
        assert_eq!(content, Some("https://example.com".to_string()));
    }

    #[test]
    fn test_extract_annotation_content_fallback() {
        let annot = pdf_oxide::Annotation {
            annotation_type: "Annot".to_string(),
            subtype: Some("Text".to_string()),
            subtype_enum: pdf_oxide::AnnotationSubtype::Text,
            contents: Some("A note".to_string()),
            rect: None,
            author: None,
            creation_date: None,
            modification_date: None,
            subject: None,
            destination: None,
            action: None,
            quad_points: None,
            color: None,
            opacity: None,
            flags: pdf_oxide::AnnotationFlags::empty(),
            border: None,
            interior_color: None,
            field_type: None,
            field_name: None,
            field_value: None,
            default_value: None,
            field_flags: None,
            options: None,
            appearance_state: None,
            raw_dict: None,
        };

        let content = extract_annotation_content(&annot);
        assert_eq!(content, Some("A note".to_string()));
    }
}
