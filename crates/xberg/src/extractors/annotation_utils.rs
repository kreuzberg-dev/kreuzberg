use crate::types::document_structure::TextAnnotation;

/// Adjust annotation byte offsets after trimming whitespace from text.
///
/// Offsets are shifted left by the amount of leading whitespace removed and then
/// **clamped** to the trimmed text's length. An annotation that runs into the
/// trailing whitespace `trim()` removed still covers real words, so clamping it
/// preserves that formatting; the previous `a.end <= trimmed_len` filter dropped
/// the whole span instead, silently losing the emphasis. See #226.
///
/// Annotations that collapse to an empty range — because they lay entirely
/// inside removed leading or trailing whitespace — are still discarded.
pub(crate) fn adjust_annotations_for_trim(
    annotations: Vec<TextAnnotation>,
    raw_text: &str,
    trimmed_text: &str,
) -> Vec<TextAnnotation> {
    let offset = (raw_text.len() - raw_text.trim_start().len()) as u32;
    let trimmed_len = trimmed_text.len() as u32;
    annotations
        .into_iter()
        .filter_map(|mut a| {
            a.start = a.start.saturating_sub(offset).min(trimmed_len);
            a.end = a.end.saturating_sub(offset).min(trimmed_len);
            (a.start < a.end).then_some(a)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::document_structure::AnnotationKind;

    fn ann(start: u32, end: u32) -> TextAnnotation {
        TextAnnotation {
            start,
            end,
            kind: AnnotationKind::Bold,
        }
    }

    #[test]
    fn no_trimming_needed() {
        let raw = "hello world";
        let trimmed = "hello world";
        let annotations = vec![ann(0, 5)];
        let result = adjust_annotations_for_trim(annotations, raw, trimmed);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 5);
    }

    #[test]
    fn leading_whitespace_trim() {
        let raw = "   hello world";
        let trimmed = "hello world";
        let annotations = vec![ann(3, 8)];
        let result = adjust_annotations_for_trim(annotations, raw, trimmed);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 5);
    }

    #[test]
    fn annotation_fully_in_trimmed_region() {
        let raw = "     hello";
        let trimmed = "hello";
        let annotations = vec![ann(0, 3)];
        let result = adjust_annotations_for_trim(annotations, raw, trimmed);
        assert!(result.is_empty(), "annotation in trimmed region should be removed");
    }

    #[test]
    fn annotation_spanning_trim_boundary() {
        let raw = "  hello";
        let trimmed = "hello";
        let annotations = vec![ann(1, 4)];
        let result = adjust_annotations_for_trim(annotations, raw, trimmed);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 2);
    }

    /// Regression for #226: an annotation whose end lands in trailing whitespace
    /// that `trim()` removed must be clamped to the trimmed length, not dropped.
    /// "hello world   " is 14 bytes; the bold run covers all of it, but only the
    /// first 11 bytes survive trimming.
    #[test]
    fn annotation_ending_in_trailing_whitespace_is_clamped() {
        let raw = "hello world   ";
        let trimmed = "hello world";
        let result = adjust_annotations_for_trim(vec![ann(0, 14)], raw, trimmed);
        assert_eq!(result.len(), 1, "annotation must be clamped, not dropped");
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 11);
    }

    /// Leading and trailing whitespace removed together: the shift and the clamp
    /// must both apply. "  hello world  " is 15 bytes, 2 leading, 2 trailing.
    #[test]
    fn annotation_spanning_both_trim_boundaries_is_shifted_and_clamped() {
        let raw = "  hello world  ";
        let trimmed = "hello world";
        let result = adjust_annotations_for_trim(vec![ann(2, 15)], raw, trimmed);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 0);
        assert_eq!(result[0].end, 11);
    }

    /// An annotation lying entirely inside removed trailing whitespace collapses
    /// to an empty range and is still discarded.
    #[test]
    fn annotation_entirely_in_trailing_whitespace_is_removed() {
        let raw = "hello   ";
        let trimmed = "hello";
        let result = adjust_annotations_for_trim(vec![ann(6, 8)], raw, trimmed);
        assert!(
            result.is_empty(),
            "annotation wholly inside removed trailing whitespace should be removed"
        );
    }

    /// A partial run touching the trailing whitespace keeps its real start.
    #[test]
    fn partial_annotation_touching_trailing_whitespace_keeps_start() {
        let raw = "hello world  ";
        let trimmed = "hello world";
        let result = adjust_annotations_for_trim(vec![ann(6, 13)], raw, trimmed);
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].start, 6);
        assert_eq!(result[0].end, 11);
    }

    #[test]
    fn empty_annotation_removed() {
        let raw = "hello";
        let trimmed = "hello";
        let annotations = vec![ann(2, 2)];
        let result = adjust_annotations_for_trim(annotations, raw, trimmed);
        assert!(result.is_empty(), "empty annotation (start==end) should be removed");
    }
}
