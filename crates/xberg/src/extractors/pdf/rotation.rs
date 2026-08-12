//! Rotated text-matrix run assembly for PDF pages.
//!
//! pdf_oxide reports each span's text-matrix rotation
//! ([`TextSpan::rotation_degrees`]) and bakes a rotated run's word gaps into
//! that run's *own* baseline rather than into page-x. Concatenating span text
//! in page order therefore both glues adjacent words together and can read a
//! rotated run's fragments out of order — GH#1358, where sideways spec tables
//! extract word-reversed and glued.
//!
//! This module carries the repair for that, and nothing else. It is
//! deliberately free of any layout-hint dependency: rotation comes straight
//! off the text matrix, so [`repair_rotated_page_text`] needs no layout
//! detection, no ONNX inference, and no feature gate. Its sibling
//! `super::reading_order` — which reconstructs columns and cross-region
//! reading order — does need hints, and stays gated behind
//! `layout-detection`; it imports the primitives below rather than
//! duplicating them.
//!
//! This does not fully close GH#1358. What is repaired here is assembly
//! *within* each same-rotation run: word order along the run's own advance
//! axis, and the word gaps between fragments. Reading order *between* regions
//! on a rotated page — which rotated block precedes which — is layout-hint
//! work and lives in `super::reading_order`, unchanged.

/// A text span with bounding box information.
///
/// `x`/`y`/`width`/`height` are always the page-space bbox pdf_oxide reports:
/// for a rotated run the origin is in page coordinates but `width`/`height`
/// are flattened onto the run's own (rotated) axis — see
/// [`upright_reading_origin`] for why ordering must account for this.
#[derive(Debug, Clone)]
pub struct TextSpan {
    pub text: String,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    /// Text-matrix rotation in degrees, as reported by pdf_oxide
    /// (`TextSpan::rotation_degrees`). Zero for the overwhelming majority of
    /// (unrotated) spans.
    pub rotation_degrees: f32,
}

/// The `rotation_degrees` an unrotated span carries. Spans on the page's own
/// upright axis take [`append_run`]'s verbatim legacy path.
const UNROTATED_DEGREES: f32 = 0.0;
const ROTATION_TOLERANCE_DEGREES: f32 = 0.001;

/// Maximum same-baseline gap that still represents a kerning-run split.
///
/// Shared with `super::reading_order`'s segment-level fragment reconciliation:
/// the cutoff between kerning and a real word boundary is one decision, so it
/// has one home.
pub(crate) const ATOMIC_FRAGMENT_GAP_RATIO: f32 = 0.15;

/// Cross-axis spans within this fraction of the taller span's `height` are
/// treated as the same rotated-frame "line" rather than a new row. Mirrors the
/// baseline-clustering role `FALSE_PICTURE_BASELINE_TOLERANCE_RATIO` plays in
/// `super::reading_order`, but expressed on the rotated upright frame
/// [`upright_reading_origin`] produces.
const ROTATED_LINE_CROSS_TOLERANCE_RATIO: f32 = 0.5;

/// Rotate a span's page-space origin into its own upright reading frame.
///
/// Mirrors [`crate::pdf::oxide::span_geometry::upright_origin`] (which
/// operates on `pdf_oxide::layout::TextSpan`) for the simpler geometry this
/// module works with. Returns `(advance, cross)`: `advance` is the position
/// along the span's own reading direction and `cross` is the position along
/// the axis lines stack on. For unrotated spans (`rotation_degrees == 0`,
/// the overwhelming majority) this is the identity `(x, y)`.
pub(crate) fn upright_reading_origin(span: &TextSpan) -> (f32, f32) {
    if is_unrotated(span.rotation_degrees) {
        return (span.x, span.y);
    }
    let (sin, cos) = (-span.rotation_degrees).to_radians().sin_cos();
    (span.x * cos - span.y * sin, span.x * sin + span.y * cos)
}

/// Assemble spans — already row-ordered by `super::reading_order`'s
/// `reorder_spans_by_layout` (or any other producer of a span-index order) —
/// into page text.
///
/// Plain concatenation of `spans[i].text` in index order is correct only when
/// every span shares the page's own upright axis: pdf_oxide bakes word gaps
/// into a rotated run's *own* baseline, not into page-x, so naive
/// concatenation of a 90/180/270-degree-rotated run's fragments both glues
/// adjacent words together (no separator survives reordering) and can read
/// the fragments out of order (a rotated run's local word order is only
/// well-defined along its own advance axis, [`upright_reading_origin`], not
/// along whatever order pdf_oxide happened to emit fragments in).
///
/// This groups `order` into maximal same-rotation runs first — a mixed page
/// (rotated body text beside an upright footer, for example) must not have
/// one frame forced across the boundary — then, for a rotated run, further
/// splits it into "lines" using cross-axis proximity (spans within
/// [`ROTATED_LINE_CROSS_TOLERANCE_RATIO`] of the run's own font extent count
/// as the same line), sorts each line strictly by advance-axis position, and
/// inserts a space wherever the advance-axis gap between consecutive spans
/// exceeds the [`ATOMIC_FRAGMENT_GAP_RATIO`] kerning cutoff.
///
/// Unrotated spans take the old code path verbatim (back-to-back
/// concatenation, no reordering, no inserted separators), so this function is
/// a byte-identical no-op whenever every span in `order` has
/// `rotation_degrees == 0.0` — the overwhelming majority of pages.
pub(crate) fn assemble_reading_order_text(spans: &[TextSpan], order: &[usize]) -> String {
    let mut text = String::new();
    let mut run_start = 0;
    while run_start < order.len() {
        let rotation = span_rotation(spans, order[run_start]);
        let mut run_end = run_start + 1;
        while run_end < order.len() && same_rotation(span_rotation(spans, order[run_end]), rotation) {
            run_end += 1;
        }
        if run_start > 0 && !text.is_empty() && !text.ends_with(char::is_whitespace) {
            text.push(' ');
        }
        append_run(&mut text, spans, &order[run_start..run_end], rotation);
        run_start = run_end;
    }
    text
}

fn span_rotation(spans: &[TextSpan], index: usize) -> f32 {
    spans.get(index).map_or(UNROTATED_DEGREES, |span| span.rotation_degrees)
}

fn is_unrotated(rotation: f32) -> bool {
    rotation.is_finite() && (rotation - UNROTATED_DEGREES).abs() <= ROTATION_TOLERANCE_DEGREES
}

fn same_rotation(left: f32, right: f32) -> bool {
    left.is_finite() && right.is_finite() && (left - right).abs() <= ROTATION_TOLERANCE_DEGREES
}

/// Append one maximal same-rotation run to `text`. `rotation == 0.0` is the
/// exact legacy path; any other rotation goes through line-clustering,
/// advance-axis sorting, and gap-based space insertion.
fn append_run(text: &mut String, spans: &[TextSpan], indices: &[usize], rotation: f32) {
    if is_unrotated(rotation) {
        for &index in indices {
            if let Some(span) = spans.get(index) {
                text.push_str(&span.text);
            }
        }
        return;
    }

    let mut line_start = 0;
    let mut first_line = true;
    while line_start < indices.len() {
        let Some(anchor) = spans.get(indices[line_start]) else {
            line_start += 1;
            continue;
        };
        let (_, anchor_cross) = upright_reading_origin(anchor);
        let mut line_end = line_start + 1;
        while line_end < indices.len() {
            let Some(candidate) = spans.get(indices[line_end]) else {
                break;
            };
            let (_, candidate_cross) = upright_reading_origin(candidate);
            let tolerance = anchor.height.max(candidate.height).max(f32::EPSILON) * ROTATED_LINE_CROSS_TOLERANCE_RATIO;
            if (candidate_cross - anchor_cross).abs() > tolerance {
                break;
            }
            line_end += 1;
        }

        if !first_line && !text.is_empty() && !text.ends_with(char::is_whitespace) {
            text.push(' ');
        }
        first_line = false;
        append_rotated_line(text, spans, &indices[line_start..line_end]);
        line_start = line_end;
    }
}

/// Sort one rotated-frame line by advance-axis position and join it,
/// inserting a space wherever the advance-axis gap between consecutive spans
/// looks like a real word boundary rather than kerning.
fn append_rotated_line(text: &mut String, spans: &[TextSpan], indices: &[usize]) {
    let mut ordered: Vec<usize> = indices.to_vec();
    ordered.sort_by(|&a, &b| {
        let advance_a = spans.get(a).map_or(0.0, |span| upright_reading_origin(span).0);
        let advance_b = spans.get(b).map_or(0.0, |span| upright_reading_origin(span).0);
        advance_a.total_cmp(&advance_b)
    });

    let mut previous_advance_end: Option<f32> = None;
    for index in ordered {
        let Some(span) = spans.get(index) else { continue };
        let (advance_start, _) = upright_reading_origin(span);
        if let Some(previous_end) = previous_advance_end {
            let gap = advance_start - previous_end;
            let kerning_limit = span.height.max(f32::EPSILON) * ATOMIC_FRAGMENT_GAP_RATIO;
            if gap > kerning_limit && !text.is_empty() && !text.ends_with(char::is_whitespace) {
                text.push(' ');
            }
        }
        text.push_str(&span.text);
        previous_advance_end = Some(advance_start + span.width);
    }
}

/// True when any span on the page carries a non-zero text-matrix rotation.
///
/// Rotation is read straight off the PDF text matrix, so this question is
/// answerable without layout detection, ONNX inference, or any hint at all.
/// It exists as the cheap gate in front of [`repair_rotated_page_text`]: the
/// overwhelming majority of pages are entirely upright and must be able to
/// skip the repair on a single linear scan.
///
/// This says only "is there rotation at all" — see [`rotation_is_dominant`]
/// for the second gate that decides whether the full-page repair is worth
/// what it costs the rest of the page.
pub(crate) fn page_has_rotated_spans(spans: &[TextSpan]) -> bool {
    spans.iter().any(|span| !is_unrotated(span.rotation_degrees))
}

/// Minimum share of a page's span text, by character count, that must carry
/// non-zero rotation before [`repair_rotated_page_text`] rewrites the page.
///
/// [`repair_rotated_page_text`] does not repair only the rotated run — it
/// replaces the *entire page's* text via [`assemble_reading_order_text`],
/// whose unrotated path is a legacy verbatim concatenation with no inserted
/// separators at all (see [`append_run`]'s unrotated branch), unlike
/// `assemble_page_text` in `pdf::oxide::text` (paragraph/line-break
/// detection, RTL handling, glyph-fragmentation repair for #962). A page
/// where rotation is a tiny minority — one rotated caption, axis label, or
/// section-tab digit sitting on an otherwise entirely upright page — pays
/// that cost across the whole page to fix a few words, and loses far more
/// than it gains.
///
/// This is not hypothetical: three corpus regressions traced to exactly this
/// shape. `issue-90-example` and `nougat_004` each carry a single rotated
/// caption/label on an otherwise upright page; `pdfa_042` carries three-
/// character rotated section-tab labels (`"2.1"`, `"3.1"`, `"3.2"`) beside
/// full pages of upright table and prose text — well under 5% of the page's
/// characters in every case. `MIN_ROTATED_TEXT_SHARE` requires rotation to be
/// a substantial share of the page — the shape of a genuinely sideways table
/// or page, GH#1358's actual target — before the whole-page rewrite fires.
const MIN_ROTATED_TEXT_SHARE: f32 = 0.2;

/// True when rotated spans make up at least [`MIN_ROTATED_TEXT_SHARE`] of the
/// page's total span text, measured by character count.
///
/// Character count (not span count) is the right unit: a page can carry many
/// short unrotated fragments (kerning splits, table cells) alongside one
/// short rotated label, and span-count alone would over-weight fragmentation
/// on either side. An empty page has no dominant rotation by definition.
fn rotation_is_dominant(spans: &[TextSpan]) -> bool {
    let mut rotated_chars = 0usize;
    let mut total_chars = 0usize;
    for span in spans {
        let chars = span.text.chars().count();
        total_chars += chars;
        if !is_unrotated(span.rotation_degrees) {
            rotated_chars += chars;
        }
    }
    total_chars > 0 && (rotated_chars as f32 / total_chars as f32) >= MIN_ROTATED_TEXT_SHARE
}

/// Repair a page whose text contains rotated runs, without layout hints.
///
/// This is the hint-free half of the GH#1358 fix: sideways tables and rotated
/// captions read word-reversed and glued, and [`assemble_reading_order_text`]
/// needs only `rotation_degrees` to undo it. Ordering is deliberately left at
/// identity — the span order the caller already has — because deciding a
/// *better* cross-region order is the part that genuinely needs layout hints,
/// and `super::reading_order`'s hint-free geometric fallback is rotation-blind
/// by design and so cannot supply it.
///
/// Returns `None` when the page has no rotated spans, so the caller can leave
/// that page's existing text completely untouched. That is the safety
/// property the whole change rests on: an upright page is never rewritten and
/// its output cannot drift. It also returns `None` when rotation exists but
/// is not [`rotation_is_dominant`]: replacing the whole page's assembly to
/// fix a minority of rotated characters costs the upright majority its
/// paragraph structure, which is a worse trade than leaving the minority
/// rotated run exactly as unrepaired as it was before GH#1358.
pub(crate) fn repair_rotated_page_text(spans: &[TextSpan]) -> Option<String> {
    if !page_has_rotated_spans(spans) || !rotation_is_dominant(spans) {
        return None;
    }

    let identity_order: Vec<usize> = (0..spans.len()).collect();
    Some(assemble_reading_order_text(spans, &identity_order))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Regression tests for issue #292/#293 (GH#1358): `reorder_spans_by_layout`
    // already emits the correct span-index order, but the caller
    // (`apply_reading_order_reordering` in extraction.rs) used to concatenate
    // `spans[index].text` back-to-back with no separator at all. For a
    // 90-degree-rotated run pdf_oxide bakes word gaps into the run's own
    // (rotated) baseline, not into page-x, so naive concatenation glued
    // adjacent words together. `assemble_reading_order_text` fixes this by
    // grouping same-rotation runs, sorting each rotated line by its own
    // advance axis, and inserting a space wherever the advance-axis gap looks
    // like a real word boundary rather than kerning.
    mod issue_292_span_assembly {
        use super::*;

        fn rotated_word(text: &str, y: f32, width: f32) -> TextSpan {
            TextSpan {
                text: text.to_string(),
                x: 100.0,
                y,
                width,
                height: 10.0,
                rotation_degrees: 90.0,
            }
        }

        /// Six words of a rotated run, built so the correct reading order
        /// ("Engine oil need only meet the") reads along ascending
        /// advance-axis (page-y for a 90-degree run), with real 3pt word gaps
        /// between them (well above the kerning cutoff for a 10pt run). Fed
        /// to `assemble_reading_order_text` in the exact reverse order to
        /// reproduce the observed defect (#292: "the meet only need oil
        /// Engine").
        fn scrambled_rotated_sentence() -> Vec<TextSpan> {
            vec![
                rotated_word("Engine", 0.0, 36.0),
                rotated_word("oil", 39.0, 18.0),
                rotated_word("need", 60.0, 24.0),
                rotated_word("only", 87.0, 24.0),
                rotated_word("meet", 114.0, 24.0),
                rotated_word("the", 141.0, 18.0),
            ]
        }

        #[test]
        fn should_reassemble_rotated_run_in_advance_order_with_word_gaps() {
            let spans = scrambled_rotated_sentence();
            // Fed in reverse: exactly the garbled order observed on the real
            // fixture, where the run's fragments arrive back-to-front.
            let order = vec![5, 4, 3, 2, 1, 0];

            let text = assemble_reading_order_text(&spans, &order);

            assert_eq!(
                text, "Engine oil need only meet the",
                "rotated-run assembly must read along the advance axis and space real word gaps"
            );
        }

        #[test]
        fn should_not_insert_space_for_kerning_tight_rotated_fragments() {
            // "Eng" and "ine" are two fragments of one word pdf_oxide split,
            // 0.5pt apart on a 10pt run — well under the kerning cutoff
            // (10.0 * ATOMIC_FRAGMENT_GAP_RATIO = 1.5), so no space belongs
            // between them.
            let spans = vec![rotated_word("Eng", 0.0, 18.0), rotated_word("ine", 18.5, 18.0)];
            let order = vec![0, 1];

            let text = assemble_reading_order_text(&spans, &order);

            assert_eq!(
                text, "Engine",
                "kerning-tight rotated fragments must glue, not space, together"
            );
        }

        #[test]
        fn should_not_force_one_frame_across_a_rotation_boundary() {
            // An upright footer line sandwiched between two rotated-body
            // fragments: the rotation boundary must isolate the footer from
            // the rotated run's advance-axis sort, and the footer itself must
            // take the exact legacy pass-through (no reordering, no inserted
            // separators) regardless of position.
            let spans = vec![
                rotated_word("oil", 39.0, 18.0),   // 0: rotated body, second word
                rotated_word("Engine", 0.0, 36.0), // 1: rotated body, first word
                TextSpan {
                    text: "Page 264".to_string(),
                    x: 500.0,
                    y: 10.0,
                    width: 40.0,
                    height: 8.0,
                    rotation_degrees: 0.0,
                },
            ];
            // Rotated run first (fed reversed, like the fixture), then the
            // upright footer as its own trailing run.
            let order = vec![0, 1, 2];

            let text = assemble_reading_order_text(&spans, &order);

            assert_eq!(
                text, "Engine oil Page 264",
                "the rotated run resolves to advance order internally; the upright run after it \
                 is untouched pass-through text separated from the rotated frame"
            );
        }

        /// Over-fire guard: an entirely unrotated span list must come out
        /// byte-identical to plain `order`-indexed concatenation, with no
        /// sorting or gap-based spacing applied — even when the given order
        /// does not match left-to-right page position, proving the rotated
        /// path never fires for `rotation_degrees == 0.0`.
        #[test]
        fn should_leave_unrotated_spans_byte_identical_to_plain_concatenation() {
            let spans = vec![
                TextSpan {
                    text: "second".to_string(),
                    x: 300.0,
                    y: 0.0,
                    width: 40.0,
                    height: 10.0,
                    rotation_degrees: 0.0,
                },
                TextSpan {
                    text: "first".to_string(),
                    x: 0.0,
                    y: 0.0,
                    width: 40.0,
                    height: 10.0,
                    rotation_degrees: 0.0,
                },
            ];
            // Deliberately positionally "wrong" order (right-hand span first)
            // — the unrotated path must reproduce it verbatim, not repair it.
            let order = vec![0, 1];

            let text = assemble_reading_order_text(&spans, &order);

            assert_eq!(
                text, "secondfirst",
                "unrotated spans must pass through in the given order with no separators, unchanged"
            );
        }
    }

    // GH#1358: the rotated-run repair above is correct but used to be
    // reachable only through layout hints, in the feature-gated
    // `super::reading_order`. `repair_rotated_page_text` exposes it on the
    // hint-free path, gated by `page_has_rotated_spans` so an upright page is
    // never rewritten. These tests compile and run on a default build — no
    // `layout-detection` feature — which is the whole point of the split. ~keep
    mod rotation_repair_without_layout_hints {
        use super::*;

        /// One word of a 90-degree-rotated run. For a 90-degree run the
        /// advance axis is page-y, so `y` is the word's reading position and
        /// the shared `x` puts every word on one rotated line.
        fn rotated_word(text: &str, y: f32, width: f32) -> TextSpan {
            TextSpan {
                text: text.to_string(),
                x: 100.0,
                y,
                width,
                height: 10.0,
                rotation_degrees: 90.0,
            }
        }

        fn upright_word(text: &str, x: f32, width: f32) -> TextSpan {
            TextSpan {
                text: text.to_string(),
                x,
                y: 0.0,
                width,
                height: 10.0,
                rotation_degrees: 0.0,
            }
        }

        /// A rotated run in the scrambled order pdf_oxide emits it: fragments
        /// arrive back-to-front, with real 3pt word gaps (well above the
        /// kerning cutoff of 10.0 * ATOMIC_FRAGMENT_GAP_RATIO = 1.5).
        fn scrambled_rotated_page() -> Vec<TextSpan> {
            vec![
                rotated_word("the", 141.0, 18.0),
                rotated_word("meet", 114.0, 24.0),
                rotated_word("only", 87.0, 24.0),
                rotated_word("need", 60.0, 24.0),
                rotated_word("oil", 39.0, 18.0),
                rotated_word("Engine", 0.0, 36.0),
            ]
        }

        #[test]
        fn should_report_no_rotated_spans_when_every_span_is_upright() {
            let spans = vec![upright_word("first", 0.0, 40.0), upright_word("second", 50.0, 40.0)];

            assert!(
                !page_has_rotated_spans(&spans),
                "an all-upright page must not be flagged as rotated"
            );
        }

        #[test]
        fn should_report_rotated_spans_when_any_single_span_is_rotated() {
            let spans = vec![
                upright_word("first", 0.0, 40.0),
                rotated_word("Engine", 0.0, 36.0),
                upright_word("second", 50.0, 40.0),
            ];

            assert!(
                page_has_rotated_spans(&spans),
                "one rotated span anywhere on the page is enough to need the repair"
            );
        }

        #[test]
        fn should_return_none_for_an_unrotated_page() {
            let spans = vec![upright_word("second", 300.0, 40.0), upright_word("first", 0.0, 40.0)];

            assert!(
                repair_rotated_page_text(&spans).is_none(),
                "a page with no rotated span must be left untouched, not re-assembled \
                 into positional order"
            );
        }

        #[test]
        fn should_repair_a_ninety_degree_page_from_identity_span_order() {
            let spans = scrambled_rotated_page();
            let naive: String = spans.iter().map(|span| span.text.as_str()).collect();
            assert_eq!(
                naive, "themeetonlyneedoilEngine",
                "guard: the unrepaired page really is word-reversed and glued"
            );

            let repaired = repair_rotated_page_text(&spans);

            assert_eq!(
                repaired.as_deref(),
                Some("Engine oil need only meet the"),
                "the rotated run must be re-read along its own advance axis with word gaps restored"
            );
        }

        #[test]
        fn should_keep_the_unrotated_run_in_legacy_order_on_a_mixed_page() {
            // Rotated body fragments (reversed, as emitted) followed by two
            // upright spans in a deliberately non-positional order: only the
            // rotated run may be re-read, the upright run stays verbatim.
            let spans = vec![
                rotated_word("oil", 39.0, 18.0),
                rotated_word("Engine", 0.0, 36.0),
                upright_word("second", 300.0, 40.0),
                upright_word("first", 0.0, 40.0),
            ];

            let repaired = repair_rotated_page_text(&spans);

            assert_eq!(
                repaired.as_deref(),
                Some("Engine oil secondfirst"),
                "the rotated run resolves to advance order; the upright run keeps its given \
                 order and remains separated from the rotated frame"
            );
        }

        // Regression coverage for the three corpus documents (issue-90-example,
        // nougat_004, pdfa_042) that #1358's original all-or-nothing gate broke:
        // each carries a small genuinely-rotated island on an otherwise entirely
        // upright page, and the old gate replaced the whole page's text —
        // including the untouched upright majority — with the legacy
        // no-separator concatenation path.
        mod rotation_dominance_gate {
            use super::*;

            #[test]
            fn should_not_repair_when_rotated_text_is_a_small_minority_of_the_page() {
                // Mirrors pdfa_042: a three-character rotated section-tab label
                // ("2.1") beside a page of long upright sentences. Rotated share
                // is 3 / (3 + 100+) — far under MIN_ROTATED_TEXT_SHARE.
                let spans = vec![
                    upright_word(
                        "This upright paragraph carries the overwhelming majority of the page text",
                        0.0,
                        400.0,
                    ),
                    upright_word(
                        "and must keep its normal paragraph and line-break assembly untouched",
                        50.0,
                        400.0,
                    ),
                    rotated_word("2.1", 500.0, 12.0),
                ];

                assert!(
                    page_has_rotated_spans(&spans),
                    "guard: the page does carry a rotated span"
                );
                assert_eq!(
                    repair_rotated_page_text(&spans),
                    None,
                    "a minority rotated label must not trigger a full-page rewrite that costs \
                     the upright majority its own assembly"
                );
            }

            #[test]
            fn should_repair_when_rotated_text_dominates_the_page() {
                // The inverse: a page that is mostly the rotated run, matching
                // GH#1358's actual target (a sideways spec table taking up most
                // of the page). Rotation is clearly dominant here, so the
                // full-page repair must still fire.
                let spans = scrambled_rotated_page();

                assert!(
                    repair_rotated_page_text(&spans).is_some(),
                    "an entirely-rotated page must still be repaired"
                );
            }
        }
    }
}
