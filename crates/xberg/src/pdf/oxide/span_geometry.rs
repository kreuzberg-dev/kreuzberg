//! Span orientation predicates and upright-frame geometry shared by the
//! pdf_oxide span pipeline.
//!
//! # Why rotated spans need their own frame
//!
//! pdf_oxide reports the content-stream text-matrix rotation of every run in
//! [`TextSpan::rotation_degrees`]. For such a run:
//!
//! * `bbox.x` / `bbox.y` are **page-space** coordinates of the run origin, and
//! * `bbox.width` / `bbox.height` are **flattened onto the run's own axis** —
//!   `width` is the sum of the glyph advances along the rotated baseline and
//!   `height` is the font extent perpendicular to it.
//!
//! pdf_oxide states this itself (`document.rs`: "a rotated run's glyphs advance
//! along a rotated axis, but the span bbox flattens them onto the x-axis
//! (width = Σ glyph advances, height = font)") and relies on it in
//! `order_rotated_blocks`, which rotates each origin by `-rotation_degrees`
//! before applying the ordinary row-aware comparator.
//!
//! Consequently any gap/overlap arithmetic that assumes an upright axis is
//! wrong for rotated runs: for 90-degree text the reading direction is a step
//! along page-y and the "next line" is a step along page-x. Rotating only the
//! **origin** back into an upright frame — the width/height are already
//! expressed there — makes every existing upright heuristic correct again
//! without transforming the page as a whole.
//!
//! For unrotated spans every function here is the identity on `bbox`, so
//! rotation-0 pages (the overwhelming majority) are byte-identical.

use pdf_oxide::layout::TextSpan;

/// True when the span is drawn with an upright (unrotated) text matrix.
pub(crate) fn is_unrotated(span: &TextSpan) -> bool {
    span.rotation_degrees.abs() <= f32::EPSILON
}

/// True when two spans are drawn with the same text-matrix rotation.
///
/// Only same-rotation spans may be compared geometrically: their bboxes are
/// flattened onto different axes otherwise.
pub(crate) fn has_same_rotation(first: &TextSpan, second: &TextSpan) -> bool {
    (first.rotation_degrees - second.rotation_degrees).abs() <= f32::EPSILON
}

/// True when the span's *writing mode* is horizontal left-to-right.
///
/// This deliberately says nothing about `rotation_degrees`. A rotated table
/// header is still horizontal LTR text — it is merely painted along a rotated
/// baseline — so joining, spacing, and row-reset decisions between two spans of
/// **equal** rotation remain valid. Callers whose arithmetic genuinely assumes
/// the page axis should use [`is_horizontal_ltr`] instead.
pub(crate) fn is_ltr_writing_mode(span: &TextSpan) -> bool {
    span.wmode == 0 && !span.rtl_draw_logical
}

/// True when the span is horizontal LTR **and** painted on the page axis.
///
/// Use this only where the surrounding arithmetic is expressed in raw page
/// coordinates and cannot be lifted into the span's upright frame.
pub(crate) fn is_horizontal_ltr(span: &TextSpan) -> bool {
    is_ltr_writing_mode(span) && is_unrotated(span)
}

/// The span origin rotated back into the span's own upright reading frame.
///
/// Returns `(advance_axis, cross_axis)`. Mirrors the `-rotation_degrees`
/// rotation pdf_oxide's `order_rotated_blocks` applies before sorting, so
/// ordering and separator decisions agree with the order spans arrive in.
pub(crate) fn upright_origin(span: &TextSpan) -> (f32, f32) {
    if is_unrotated(span) {
        return (span.bbox.x, span.bbox.y);
    }
    let (sin, cos) = (-span.rotation_degrees).to_radians().sin_cos();
    (
        span.bbox.x * cos - span.bbox.y * sin,
        span.bbox.x * sin + span.bbox.y * cos,
    )
}

/// `(start, end)` of the span along its own advance axis.
///
/// Equivalent to `(bbox.x, bbox.x + bbox.width)` for unrotated spans.
pub(crate) fn upright_advance_extent(span: &TextSpan) -> (f32, f32) {
    let (start, _) = upright_origin(span);
    (start, start + span.bbox.width)
}

/// `(low, high)` of the span on its own cross axis (the axis lines stack along).
///
/// Equivalent to `(bbox.y, bbox.y + bbox.height)` for unrotated spans.
pub(crate) fn upright_cross_extent(span: &TextSpan) -> (f32, f32) {
    let (_, low) = upright_origin(span);
    (low, low + span.bbox.height)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pdf_oxide::geometry::Rect;

    fn rotated_span(x: f32, y: f32, width: f32, height: f32, rotation_degrees: f32) -> TextSpan {
        TextSpan {
            text: "x".to_string(),
            bbox: Rect { x, y, width, height },
            font_size: height,
            rotation_degrees,
            ..TextSpan::default()
        }
    }

    #[test]
    fn should_return_raw_bbox_when_span_is_unrotated() {
        let span = rotated_span(100.0, 700.0, 40.0, 10.0, 0.0);

        assert_eq!(upright_origin(&span), (100.0, 700.0));
        assert_eq!(upright_advance_extent(&span), (100.0, 140.0));
        assert_eq!(upright_cross_extent(&span), (700.0, 710.0));
    }

    #[test]
    fn should_swap_axes_when_span_is_rotated_ninety_degrees() {
        let span = rotated_span(100.0, 700.0, 40.0, 10.0, 90.0);

        let (advance, cross) = upright_origin(&span);
        // -90 degrees: (x, y) -> (y, -x).
        assert!((advance - 700.0).abs() < 1e-3, "advance axis was {advance}");
        assert!((cross - -100.0).abs() < 1e-3, "cross axis was {cross}");
        let (start, end) = upright_advance_extent(&span);
        assert!((start - 700.0).abs() < 1e-3 && (end - 740.0).abs() < 1e-3);
    }

    #[test]
    fn should_treat_rotated_span_as_ltr_writing_mode_but_not_horizontal_ltr() {
        let span = rotated_span(100.0, 700.0, 40.0, 10.0, 90.0);

        assert!(is_ltr_writing_mode(&span), "rotated text is still horizontal LTR text");
        assert!(
            !is_horizontal_ltr(&span),
            "rotated text is not painted on the page axis"
        );
        assert!(!is_unrotated(&span));
    }

    #[test]
    fn should_reject_vertical_and_rtl_writing_modes_regardless_of_rotation() {
        let mut vertical = rotated_span(100.0, 700.0, 40.0, 10.0, 90.0);
        vertical.wmode = 1;
        let mut right_to_left = rotated_span(100.0, 700.0, 40.0, 10.0, 0.0);
        right_to_left.rtl_draw_logical = true;

        assert!(!is_ltr_writing_mode(&vertical));
        assert!(!is_ltr_writing_mode(&right_to_left));
        assert!(!is_horizontal_ltr(&right_to_left));
    }

    #[test]
    fn should_match_rotation_only_between_equally_rotated_spans() {
        let upright = rotated_span(0.0, 0.0, 10.0, 10.0, 0.0);
        let rotated = rotated_span(0.0, 0.0, 10.0, 10.0, 90.0);
        let also_rotated = rotated_span(50.0, 0.0, 10.0, 10.0, 90.0);

        assert!(has_same_rotation(&rotated, &also_rotated));
        assert!(!has_same_rotation(&upright, &rotated));
        assert!(has_same_rotation(&upright, &upright.clone()));
    }
}
