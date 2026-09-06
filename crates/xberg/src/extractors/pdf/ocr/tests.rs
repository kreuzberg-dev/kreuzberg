// ~keep Nested (rather than flattened into this file directly) so that
// `direct_image_decode_api_calls_are_audited` (extraction/image_decode.rs), which recognizes
// the test exemption by an inline module's ident rather than by file path, still exempts this
// module's direct decoder calls exactly as it did when this file's content was a `mod tests {
// ... }` block nested inside the old monolithic ocr.rs. The name match with the containing
// file-module is therefore deliberate, not an oversight.
#[allow(clippy::module_inception)]
mod tests {
    use super::super::document::*;
    use super::super::pipeline::*;
    use super::super::rendering::*;
    use super::super::scoring::*;
    use crate::core::config::ExtractionConfig;
    use crate::core::config::OcrQualityThresholds;
    use std::borrow::Cow;

    #[cfg(feature = "ocr")]
    fn t() -> OcrQualityThresholds {
        OcrQualityThresholds::default()
    }

    /// Deliberately non-square (`processed_width` 200 != `processed_height` 100):
    /// a square raster cannot distinguish a width/height mix-up from correct code,
    /// since the two dimensions are interchangeable in that case. This is exactly
    /// the shape that let the pre-fix 90/270 arms of `undo_auto_rotate_point` ship
    /// with `processed_width` and `processed_height` swapped relative to what the
    /// inverse rotation requires.
    ///
    /// Against the unfixed 90 arm (`(y, processed_height - x)`), this call returns
    /// `(40.0, 70.0)` instead of the `(40.0, 170.0)` asserted below.
    #[cfg(feature = "ocr")]
    #[test]
    fn undo_auto_rotate_point_90_arm_uses_processed_width_for_the_second_coordinate() {
        let (x, y) = undo_auto_rotate_point(30.0, 40.0, 90, 200.0, 100.0);
        assert_eq!((x, y), (40.0, 170.0));
    }

    /// Companion to the 90-arm test above, same non-square dimensions.
    ///
    /// Against the unfixed 270 arm (`(processed_width - y, x)`), this call returns
    /// `(160.0, 30.0)` instead of the `(60.0, 30.0)` asserted below.
    #[cfg(feature = "ocr")]
    #[test]
    fn undo_auto_rotate_point_270_arm_uses_processed_height_for_the_first_coordinate() {
        let (x, y) = undo_auto_rotate_point(30.0, 40.0, 270, 200.0, 100.0);
        assert_eq!((x, y), (60.0, 30.0));
    }

    /// Control: the 180 arm was already correct before and after the 90/270 fix
    /// (a half-turn does not swap width and height, so there is no dimension to mix
    /// up). Same non-square dimensions and inputs as the 90/270 tests above; this
    /// must stay green across the fix.
    #[cfg(feature = "ocr")]
    #[test]
    fn undo_auto_rotate_point_180_arm_is_unaffected_by_the_90_270_fix() {
        let (x, y) = undo_auto_rotate_point(30.0, 40.0, 180, 200.0, 100.0);
        assert_eq!((x, y), (170.0, 60.0));
    }

    /// GLM paired mode pushes the SAME `region_bbox` into both `formulas[].bbox` and
    /// `table_bboxes` (`candle_ocr/glm_ocr_backend.rs`), but
    /// `undo_upright_raster_correction` corrected only `ocr_internal_document` elements
    /// and `tables`. On a `/Rotate != 0` page the formula box therefore stayed in the
    /// upright raster's frame while its table twin was mapped back, and
    /// `formula_bbox_to_page_points` then rescaled it against MediaBox-raster page
    /// dimensions.
    ///
    /// Against unfixed code the formula assertion below fails with the untouched input
    /// box (10, 20, 30, 40) instead of the corrected one.
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn undo_upright_raster_correction_maps_a_formula_bbox_like_its_table_twin() {
        let region = crate::types::extraction::BoundingBox {
            x0: 10.0,
            y0: 20.0,
            x1: 30.0,
            y1: 40.0,
        };
        let mut result = crate::types::ExtractedDocument {
            tables: vec![crate::types::Table {
                bounding_box: Some(region),
                ..Default::default()
            }],
            formulas: vec![crate::types::Formula {
                latex: "x^2".to_string(),
                bbox: Some(region),
                page: Some(1),
            }],
            ..Default::default()
        };

        undo_upright_raster_correction(&mut result, 90, 200, 100);

        let table_bbox = result.tables[0]
            .bounding_box
            .expect("the table bbox must survive the correction");
        let formula_bbox = result.formulas[0]
            .bbox
            .expect("the formula bbox must survive the correction");

        assert_ne!(
            table_bbox, region,
            "a 90 degree correction must actually move the table bbox, or this test proves nothing"
        );
        assert_eq!(
            formula_bbox, table_bbox,
            "a formula bbox holding the same region as a table bbox must be mapped identically"
        );
    }

    /// `undo_upright_raster_correction` corrected `ocr_internal_document`, `tables`, and
    /// `formulas` but skipped `ocr_elements` entirely — the word/line/block boxes
    /// `attach_page_ocr_payload` copies straight onto the assembled document with no further
    /// pixel-space transform. On a `RequiresUpright` backend (sceptre, PaddleOCR-VL, the VLM
    /// backend, GLM-OCR, DeepSeek-OCR, TrOCR, Tesseract-WASM) run against a `/Rotate != 0`
    /// page, every word/line box stayed in the upright raster's frame forever.
    ///
    /// Non-square `upright_width`/`upright_height` (200x100), matching the Bug-A tests, so a
    /// square raster cannot hide a width/height mix-up in the geometry conversion either.
    ///
    /// Against unfixed code neither assertion holds: `ocr_elements` is not read at all inside
    /// `undo_upright_raster_correction`, so the rectangle stays `{left: 10, top: 20, width: 15,
    /// height: 10}` and the quadrilateral stays `[(0,0), (50,0), (50,20), (0,20)]` — identical
    /// to the untouched input asserted against below.
    #[cfg(all(feature = "ocr", feature = "pdf"))]
    #[test]
    fn undo_upright_raster_correction_maps_ocr_element_geometry() {
        use crate::types::ocr_elements::{OcrBoundingGeometry, OcrElement};

        let mut result = crate::types::ExtractedDocument {
            ocr_elements: Some(vec![
                OcrElement {
                    geometry: OcrBoundingGeometry::Rectangle {
                        left: 10,
                        top: 20,
                        width: 15,
                        height: 10,
                    },
                    ..Default::default()
                },
                OcrElement {
                    geometry: OcrBoundingGeometry::Quadrilateral {
                        points: [(0, 0), (50, 0), (50, 20), (0, 20)]
                            .into_iter()
                            .map(Into::into)
                            .collect(),
                    },
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };

        undo_upright_raster_correction(&mut result, 90, 200, 100);

        let elements = result.ocr_elements.expect("ocr_elements must survive the correction");
        assert_eq!(
            elements[0].geometry,
            OcrBoundingGeometry::Rectangle {
                left: 20,
                top: 175,
                width: 10,
                height: 15,
            },
            "the rectangle's corners must be mapped through the fixed 90-degree arm, not left in the upright raster"
        );
        assert_eq!(
            elements[1].geometry,
            OcrBoundingGeometry::Quadrilateral {
                points: [(0, 200), (0, 150), (20, 150), (20, 200)]
                    .into_iter()
                    .map(Into::into)
                    .collect(),
            },
            "the quad's points must be mapped through the fixed 90-degree arm, not left in the upright raster"
        );
    }

    #[cfg(feature = "ocr")]
    fn boundary(page_number: u32, byte_start: usize, byte_end: usize) -> crate::types::PageBoundary {
        crate::types::PageBoundary {
            byte_start,
            byte_end,
            page_number,
        }
    }

    /// A page whose OCR came back effectively blank must NOT overwrite that page's native
    /// text. Reported against a 16-page scanned ordinance whose page renders were empty:
    /// tesseract returned a couple of noise characters per page, every one cleared the old
    /// `!text.trim().is_empty()` bar, and the accepted replacements deleted the native text
    /// -- so enabling OCR returned FEWER characters than not enabling it (174 vs 185).
    #[test]
    #[cfg(feature = "ocr")]
    fn should_reject_blank_ocr_page_rather_than_overwrite_native_text() {
        let native = "Page one has real native content here.\nPage two also has real content.";
        let split = native.find('\n').expect("newline") + 1;
        let boundaries = vec![boundary(1, 0, split), boundary(2, split, native.len())];

        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        // What a blank page render actually produces: a scrap of noise.
        ocr_results.insert(1, "  a \n".to_string());

        let accepted =
            accepted_ocr_page_replacements(native, &boundaries, &ocr_results, &OcrQualityThresholds::default());
        assert!(
            accepted.is_empty(),
            "blank OCR output must not be accepted as a replacement, got {accepted:?}"
        );

        let merged = apply_ocr_page_replacements(native, &boundaries, &accepted);
        assert_eq!(merged, native, "native text must survive a blank OCR page untouched");
        assert!(
            merged.chars().filter(|c| !c.is_whitespace()).count()
                >= native.chars().filter(|c| !c.is_whitespace()).count(),
            "OCR must never reduce the non-whitespace character count below native"
        );
    }

    /// The complement: real OCR text still replaces the page, so the guard above cannot be
    /// satisfied by simply refusing everything.
    #[test]
    #[cfg(feature = "ocr")]
    fn should_still_accept_ocr_page_with_real_recovered_text() {
        let native = "garbled\nPage two native.";
        let split = native.find('\n').expect("newline") + 1;
        let boundaries = vec![boundary(1, 0, split), boundary(2, split, native.len())];

        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(1, "ORDINANCE NO. 2197\n".to_string());

        let accepted =
            accepted_ocr_page_replacements(native, &boundaries, &ocr_results, &OcrQualityThresholds::default());
        assert_eq!(accepted.len(), 1, "a page with real OCR text must be accepted");

        let merged = apply_ocr_page_replacements(native, &boundaries, &accepted);
        assert!(merged.contains("ORDINANCE NO. 2197"), "OCR text must reach the output");
        assert!(merged.contains("Page two native."), "untouched pages must be preserved");
    }

    #[test]
    #[cfg(feature = "ocr")]
    fn should_keep_rich_native_text_when_ocr_recovers_less_content() {
        let native = "বাংলা ভাষায় লেখা এই অনুসন্ধানযোগ্য অনুচ্ছেদে নির্ভরযোগ্য তথ্য এবং পরিমাপ রয়েছে। ".repeat(24);
        let boundaries = vec![boundary(1, 0, native.len())];
        let ocr_results = ahash::AHashMap::from_iter([(
            1,
            "A short English OCR fragment with much less recovered information.".to_string(),
        )]);

        let accepted =
            accepted_ocr_page_replacements(&native, &boundaries, &ocr_results, &OcrQualityThresholds::default());
        assert!(
            accepted.is_empty(),
            "lower-information OCR must not overwrite a strong native text layer"
        );

        let merged = apply_ocr_page_replacements(&native, &boundaries, &accepted);
        assert_eq!(merged, native);
    }

    /// Replacing a page with OCR text of a different length shifts every later offset, so
    /// the native boundaries no longer describe the merged text. Anything mapping a byte
    /// offset back to a page -- including page tagging, which is what `doc.pages` is built
    /// from -- reads the wrong page without this re-map.
    #[test]
    #[cfg(feature = "ocr")]
    fn should_remap_page_boundaries_onto_the_merged_text() {
        let native = "AAA\nBBB\nCCC";
        let boundaries = vec![boundary(1, 0, 4), boundary(2, 4, 8), boundary(3, 8, native.len())];

        let mut accepted: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        accepted.insert(2, "LONGER PAGE TWO\n".to_string());

        let merged = apply_ocr_page_replacements(native, &boundaries, &accepted);
        let remapped = boundaries_after_replacements(&boundaries, &accepted);

        assert_eq!(remapped.len(), 3);
        // Page 1 is before the replacement: unchanged.
        assert_eq!((remapped[0].byte_start, remapped[0].byte_end), (0, 4));
        // Page 2 grew to the replacement's length.
        assert_eq!(remapped[1].byte_start, 4);
        assert_eq!(remapped[1].byte_end, 4 + "LONGER PAGE TWO\n".len());
        // Page 3 shifted by the delta, and must still address its own text.
        assert_eq!(remapped[2].byte_end, merged.len());
        assert_eq!(
            &merged[remapped[2].byte_start..remapped[2].byte_end],
            "CCC",
            "page 3 must still resolve to its own content after the shift"
        );
        assert_eq!(
            &merged[remapped[1].byte_start..remapped[1].byte_end],
            "LONGER PAGE TWO\n",
            "page 2 must resolve to the OCR text that replaced it"
        );
    }

    /// Issue #181: TATR tables recognized during full-document OCR must carry a
    /// deterministic `table_id`, `columns`, and `bounding_box` derived from
    /// `detection_bbox` — not `..Default::default()` blanks.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn recognized_table_to_public_table_assigns_id_columns_and_bounding_box() {
        let recognized = crate::RecognizedTable {
            detection_bbox: crate::layout::BBox::new(10.0, 20.0, 110.0, 220.0),
            cells: vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            markdown: "| Name | Age |\n|---|---|\n| Alice | 30 |".to_string(),
        };

        let table = recognized_table_to_public_table(&recognized, 3, 1);

        assert_eq!(table.page_number, 3);
        assert_eq!(table.table_id.as_deref(), Some("table-2"));
        assert_eq!(table.columns, Some(vec!["Name".to_string(), "Age".to_string()]));
        let bbox = table.bounding_box.expect("bounding box must be populated");
        assert_eq!(bbox.x0, 10.0);
        assert_eq!(bbox.y0, 20.0);
        assert_eq!(bbox.x1, 110.0);
        assert_eq!(bbox.y1, 220.0);
    }

    /// Minimal `PdfParagraph` fixture: same shape `ocr_doc_to_layout_paragraphs`
    /// produces (empty `lines`, no layout region/caption), just enough for
    /// `apply_ocr_text_list_fallback`. Not layout-gated: `apply_ocr_text_list_fallback`
    /// itself no longer requires `layout-detection` (#713), and this fixture is reused
    /// by the non-layout-route tests below.
    #[cfg(feature = "ocr")]
    fn ocr_paragraph(text: &str) -> crate::pdf::structure::types::PdfParagraph {
        crate::pdf::structure::types::PdfParagraph {
            text: text.to_string(),
            lines: Vec::new(),
            dominant_font_size: 12.0,
            heading_level: None,
            is_bold: false,
            is_list_item: false,
            is_code_block: false,
            is_formula: false,
            is_page_furniture: false,
            layout_class: None,
            layout_region_path: None,
            caption_for: None,
            block_bbox: None,
            word_count: text.split_whitespace().count(),
        }
    }

    #[cfg(feature = "pdf")]
    fn disabled_page_margins() -> crate::pdf::native::text::PageMarginFractions {
        crate::pdf::native::text::PageMarginFractions { top: 0.0, bottom: 0.0 }
    }

    #[cfg(feature = "pdf")]
    fn pdf_config_with_disabled_page_margins() -> crate::core::config::PdfConfig {
        crate::core::config::PdfConfig {
            top_margin_fraction: Some(0.0),
            bottom_margin_fraction: Some(0.0),
            ..Default::default()
        }
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[test]
    fn should_filter_positioned_ocr_paragraphs_by_pdf_page_margins() {
        let mut header = ocr_paragraph("header");
        header.block_bbox = Some((20.0, 950.0, 300.0, 970.0));
        let mut body = ocr_paragraph("body");
        body.block_bbox = Some((20.0, 400.0, 300.0, 420.0));
        let mut footer = ocr_paragraph("footer");
        footer.block_bbox = Some((20.0, 20.0, 300.0, 40.0));
        let mut paragraphs = vec![header, body, footer];

        let outcome = filter_ocr_paragraphs_by_page_margins(
            &mut paragraphs,
            1000.0,
            crate::pdf::native::text::PageMarginFractions {
                top: 0.10,
                bottom: 0.10,
            },
        );

        assert!(outcome.removed);
        assert!(!outcome.missing_geometry);
        assert_eq!(ocr_paragraphs_plain_text(&paragraphs), "body");
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[test]
    fn should_preserve_unpositioned_ocr_text_and_report_missing_geometry() {
        let mut paragraphs = vec![ocr_paragraph("text-only backend output")];

        let outcome = filter_ocr_paragraphs_by_page_margins(
            &mut paragraphs,
            1000.0,
            crate::pdf::native::text::PageMarginFractions {
                top: 0.10,
                bottom: 0.10,
            },
        );

        assert!(!outcome.removed);
        assert!(outcome.missing_geometry);
        assert_eq!(ocr_paragraphs_plain_text(&paragraphs), "text-only backend output");
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[test]
    fn should_treat_all_positioned_ocr_paragraphs_removed_as_complete_filtering() {
        let mut header = ocr_paragraph("header only");
        header.block_bbox = Some((20.0, 950.0, 300.0, 970.0));
        let mut paragraphs = vec![header];

        let outcome = filter_ocr_paragraphs_by_page_margins(
            &mut paragraphs,
            1000.0,
            crate::pdf::native::text::PageMarginFractions { top: 0.10, bottom: 0.0 },
        );

        assert!(outcome.removed);
        assert!(!outcome.missing_geometry);
        assert!(paragraphs.is_empty());
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[test]
    fn should_report_missing_geometry_when_structured_ocr_paragraphs_are_empty() {
        let mut paragraphs = Vec::new();

        let outcome = filter_ocr_paragraphs_by_page_margins(
            &mut paragraphs,
            1000.0,
            crate::pdf::native::text::PageMarginFractions {
                top: 0.10,
                bottom: 0.10,
            },
        );

        assert!(!outcome.removed);
        assert!(outcome.missing_geometry);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn should_preserve_layout_header_when_ocr_content_filter_includes_headers() {
        let mut header = ocr_paragraph("header");
        header.is_page_furniture = true;
        header.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::PageHeader);
        let mut paragraphs = vec![header];
        let config = ExtractionConfig {
            content_filter: Some(crate::core::config::ContentFilterConfig {
                include_headers: true,
                ..crate::core::config::ContentFilterConfig::default()
            }),
            ..ExtractionConfig::default()
        };

        apply_ocr_layout_content_filter(&mut paragraphs, &config);

        assert!(!paragraphs[0].is_page_furniture);
    }

    /// Issue #695: on the OCR + `--layout` route, `ocr_doc_to_layout_paragraphs`
    /// only sets `is_list_item` from a high-confidence `ListItem` layout hint and
    /// never from the paragraph's own text, unlike the native-PDF assembler's
    /// `looks_like_list_item`. When the layout model fails to emit (or mislabels)
    /// a `ListItem` box for a numbered/bulleted line, the paragraph is built with
    /// `is_list_item: false` and stays that way -- exactly what a plain
    /// `ocr_doc_to_layout_paragraphs` call (without the fallback this test
    /// exercises) would leave behind.
    ///
    /// Without `apply_ocr_text_list_fallback`, every assertion below except the
    /// first (heading is left alone, which was already true before this change)
    /// fails: a fresh paragraph carries `is_list_item: false` by construction, and
    /// nothing in this call path would ever flip it.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_classifies_unclassified_numbered_and_bulleted_paragraphs() {
        let mut paragraphs = vec![
            ocr_paragraph("1. As shown on Exhibit B-1, the PD encompasses 0.7906 acre."),
            ocr_paragraph("2. Land Use: Live/Work Townhomes"),
            ocr_paragraph("- a dash-bulleted item"),
            ocr_paragraph("• a bullet-marked item"),
        ];

        apply_ocr_text_list_fallback(&mut paragraphs);

        assert!(paragraphs[0].is_list_item, "numbered marker '1.' must be recovered");
        assert!(paragraphs[1].is_list_item, "numbered marker '2.' must be recovered");
        assert!(paragraphs[2].is_list_item, "dash marker '- ' must be recovered");
        assert!(paragraphs[3].is_list_item, "bullet marker must be recovered");
    }

    /// A GENUINE numbered-section heading is left untouched even though it shares
    /// the "digit, separator, text" shape a list marker has: "1. INTRODUCTION" is
    /// excluded by `looks_like_list_item` itself (via `is_numbered_section_heading`,
    /// its ALL-CAPS-remainder branch), so this paragraph never reaches the override
    /// branch at all. This assertion passes with or without the fix -- it pins the
    /// "a real heading is never reclassified" half of the contract that the next
    /// two tests exercise from the other direction (a heading whose text is NOT a
    /// genuine section heading).
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_never_overrides_a_genuine_numbered_section_heading() {
        let mut paragraph = ocr_paragraph("1. INTRODUCTION");
        paragraph.heading_level = Some(2);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert_eq!(
            paragraph.heading_level,
            Some(2),
            "an existing heading must survive untouched"
        );
        assert!(
            !paragraph.is_list_item,
            "a paragraph already classified as a heading must not also become a list item"
        );
    }

    /// Regression for the `--ocr-scanned-pages --layout` list-F1 collapse (task
    /// #665): on `ordinance_2197_scanned.pdf` the layout model classified
    /// `"8. Maximum height of structures: 50'"` as a `Title` heading rather than a
    /// `ListItem`. Its text is NOT a genuine numbered section heading -- unlike
    /// "1. INTRODUCTION" above, the remainder is mixed-case prose, so
    /// `is_numbered_section_heading` (and therefore the exclusion inside
    /// `looks_like_list_item`) does not fire -- so the text-marker signal must win.
    ///
    /// Against the pre-fix code (which `continue`s whenever `heading_level.is_some()`,
    /// unconditionally), both assertions fail: `is_list_item` stays `false` and
    /// `heading_level` stays `Some(2)`.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_overrides_a_misclassified_title_heading() {
        let mut paragraph = ocr_paragraph("8. Maximum height of structures: 50'");
        paragraph.heading_level = Some(2);
        paragraph.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::Title);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            paragraph.is_list_item,
            "an unambiguous list marker must win over a Title heading hint"
        );
        assert_eq!(
            paragraph.heading_level, None,
            "heading_level must be cleared: assembly.rs checks heading_level BEFORE is_list_item, \
             so leaving it set would silently render this as a heading regardless of is_list_item"
        );
    }

    /// Same override, exercised through a `SectionHeader` hint instead of `Title`
    /// (the two layout classes `should_promote_logo_followed_by_title` /
    /// `apply_hint_to_paragraph` can assign `heading_level` from), and at a
    /// different heading level, to show the override is not level- or
    /// hint-class-specific.
    ///
    /// Against the pre-fix code, both assertions fail for the same reason as above.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_overrides_a_misclassified_section_header_heading() {
        let mut paragraph = ocr_paragraph("(2) Second item continues the numbered run");
        paragraph.heading_level = Some(3);
        paragraph.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::SectionHeader);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            paragraph.is_list_item,
            "an unambiguous list marker must win over a SectionHeader heading hint"
        );
        assert_eq!(
            paragraph.heading_level, None,
            "heading_level must be cleared alongside setting is_list_item"
        );
    }

    // ~keep
    // Pinned against `apply_ocr_text_list_fallback_overrides_a_misclassified_section_header_heading`
    // above: that test proves `"(2) Second item …"` (capital `S`) is an unambiguous list marker
    // that must override a `SectionHeader` heading hint. This test proves the mirror case is
    // deliberately NOT a list marker: `pdf::structure::pipeline::is_inline_parenthesized_quantity`
    // treats a same-line, lowercase continuation after a parenthesized numeric marker
    // (`"(2) second item …"`) as the shape of a spelled-out quantity clarification ("Two (2)
    // additional…") that happens to start a wrapped line, not a genuine enumerated item -- a
    // genuine item is a new sentence and so starts with a capital letter.
    //
    // The test that used to occupy this string (`ba76d1b497`, 2026-08-19) asserted the opposite
    // -- that lowercase "(2) second item …" WAS a list marker -- and went undetected as wrong
    // once `is_inline_parenthesized_quantity` landed in `75e50e4156` (2026-08-21) without touching
    // this file: the fixture and the production rule silently diverged. Keeping both the capital-
    // and lowercase-marker cases side by side means the next change to either
    // `looks_like_list_item` or `is_inline_parenthesized_quantity` cannot repeat that collision
    // without failing one of these two tests.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_leaves_a_lowercase_parenthesized_quantity_continuation_unclassified() {
        let mut paragraph = ocr_paragraph("(2) second item continues the numbered run");
        paragraph.heading_level = Some(3);
        paragraph.layout_class = Some(crate::pdf::structure::types::LayoutHintClass::SectionHeader);

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            !paragraph.is_list_item,
            "a lowercase continuation after a parenthesized numeric marker reads as a spelled-out \
             quantity clarification, not a genuine enumerated item, so it must not be swept into \
             the list classification"
        );
        assert_eq!(
            paragraph.heading_level,
            Some(3),
            "the fallback only fills gaps for unambiguous list markers; an ambiguous shape must \
             leave the existing SectionHeader heading hint untouched"
        );
    }

    /// A paragraph already marked as a list item by a high-confidence layout hint
    /// must be left exactly as-is: the fallback only fills gaps, it never
    /// re-derives a classification the layout path already made.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_leaves_already_classified_list_items_untouched() {
        let mut paragraph = ocr_paragraph("plain prose with no marker at all");
        paragraph.is_list_item = true;

        apply_ocr_text_list_fallback(std::slice::from_mut(&mut paragraph));

        assert!(
            paragraph.is_list_item,
            "an existing list classification must survive untouched"
        );
    }

    /// Plain prose -- the overwhelming majority of any document's paragraphs --
    /// must never be swept into the list classification. Without this guard the
    /// fallback would be a net regression, not a fix.
    ///
    /// Asserted through `apply_ocr_text_list_fallback` rather than the marker
    /// predicate directly: the predicate is now
    /// `pdf::structure::pipeline::looks_like_list_item`, which owns its own tests,
    /// and what matters here is that the OCR route consults it and acts on the
    /// answer. The author-byline case ("A. Smith, B. Jones") is included because
    /// it is exactly what an OCR-route-local re-implementation of the marker rules
    /// would get wrong -- it only passes because the shared predicate is used.
    #[cfg(all(feature = "ocr", feature = "layout-detection"))]
    #[test]
    fn apply_ocr_text_list_fallback_leaves_ordinary_prose_and_non_list_shapes_unclassified() {
        let texts = [
            "This is an ordinary sentence with no marker.",
            "1000. Four-digit identifiers are not list markers.",
            "2024. A total of 3 trucks were used.",
            "1. INTRODUCTION",
            "A. Smith, B. Jones, Journal of Irreproducible Results",
            "",
        ];
        let mut paragraphs: Vec<_> = texts.iter().map(|text| ocr_paragraph(text)).collect();

        apply_ocr_text_list_fallback(&mut paragraphs);

        for (paragraph, text) in paragraphs.iter().zip(texts) {
            assert!(!paragraph.is_list_item, "must not classify as a list item: {text:?}");
        }
    }

    /// #713: on the non-layout OCR route, `apply_ocr_text_list_fallback` was gated on
    /// `feature = "layout-detection"` even though it does no layout classification at
    /// all -- it is a pure text-marker pass. This is deliberately compiled and run
    /// under `feature = "ocr"` alone (no `layout-detection` in this test's own `cfg`)
    /// to prove it no longer requires that feature.
    ///
    /// Against unfixed code this does not compile in an `ocr`-only build (the function
    /// itself carries `#[cfg(all(feature = "ocr", feature = "layout-detection"))]`); in
    /// a build where `layout-detection` happens to be on too (the common CI default),
    /// it still documents the intended availability contract this fix changes.
    #[cfg(feature = "ocr")]
    #[test]
    fn apply_ocr_text_list_fallback_is_available_without_layout_detection() {
        let mut paragraphs = vec![ocr_paragraph("1. Item without any ML layout classification")];

        apply_ocr_text_list_fallback(&mut paragraphs);

        assert!(paragraphs[0].is_list_item);
    }

    /// #713 end-to-end for the non-layout OCR route: a single hOCR block spanning
    /// three numbered items must, after both fixes (`ocr_doc_to_paragraphs`'s
    /// marker-splitting and `apply_ocr_text_list_fallback`'s text-marker
    /// classification), come out as three separately classified list items -- the
    /// same outcome the ML-layout route already got via `push_body_group` +
    /// `apply_ocr_text_list_fallback`.
    ///
    /// Against unfixed code this asserts `paragraphs.len() == 3` and
    /// `paragraphs[i].is_list_item` for each; today `ocr_doc_to_paragraphs` returns a
    /// single unsplit, unclassified paragraph (`len() == 1`, `is_list_item == false`).
    #[cfg(feature = "ocr")]
    #[test]
    fn non_layout_route_recovers_list_items_from_a_multi_marker_ocr_block() {
        let mut doc = crate::types::internal::InternalDocument::new("test");
        let mut elem = crate::types::internal::InternalElement::text(
            crate::types::internal::ElementKind::OcrText {
                level: crate::types::ocr_elements::OcrElementLevel::Block,
            },
            "1. First item\n2. Second item\n3. Third item",
            0,
        );
        elem.bbox = Some(crate::types::extraction::BoundingBox {
            x0: 10.0,
            y0: 10.0,
            x1: 200.0,
            y1: 100.0,
        });
        doc.push_element(elem);

        let mut paragraphs = crate::pdf::structure::adapters::ocr_doc_to_paragraphs(
            &doc,
            1000,
            crate::pdf::structure::adapters::OcrFontSizeScale::uniform(1.0),
        );
        apply_ocr_text_list_fallback(&mut paragraphs);

        assert_eq!(
            paragraphs.len(),
            3,
            "each marker-opening line must become its own paragraph"
        );
        assert!(
            paragraphs.iter().all(|paragraph| paragraph.is_list_item),
            "every split item must be classified"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_empty_text_triggers_fallback() {
        let decision = evaluate_native_text_for_ocr("", Some(1), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_replacement_chars_trigger_fallback() {
        let text = "The \u{FFFD}\u{FFFD}\u{FFFD} quick \u{FFFD}\u{FFFD}\u{FFFD} brown fox";
        let stats = NativeTextStats::from(text);
        assert_eq!(stats.garbage_char_count, 6);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_fragmented_words_trigger_fallback() {
        let text = "T h e q u i c k b r o w n f o x j u m p s";
        let stats = NativeTextStats::from(text);
        assert!(stats.fragmented_word_ratio > 0.8);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_good_text_no_fallback() {
        let text = "This is a normal paragraph with meaningful words and proper structure. \
                    It contains multiple sentences that form a coherent text block.";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(!decision.fallback);
    }

    /// Builds a PUA-heavy string simulating an undecodable glyph-index text layer:
    /// a font whose CID/glyph indices resolve into the Private Use Area rather than
    /// real Unicode (issue #1254).
    #[cfg(feature = "ocr")]
    fn pua_garbage_text() -> String {
        (0..200)
            .map(|i| char::from_u32(0xE000 + (i % 400)).expect("valid PUA codepoint"))
            .collect::<String>()
            .chars()
            .collect::<Vec<char>>()
            .chunks(6)
            .map(|chunk| chunk.iter().collect::<String>())
            .collect::<Vec<String>>()
            .join(" ")
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_ratio_helper_flags_pua_heavy_text() {
        let garbage = pua_garbage_text();
        let stats = NativeTextStats::from(&garbage);
        assert!(
            stats.undecodable_ratio >= 0.99,
            "expected near-total undecodable ratio for all-PUA text, got {}",
            stats.undecodable_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_ratio_helper_ignores_occasional_symbols() {
        let text = "This is a normal paragraph with meaningful words \u{2022} and one bullet symbol, \
                    plus a trademark\u{2122} and a section sign \u{00A7} sprinkled in for good measure.";
        let stats = NativeTextStats::from(text);
        assert!(
            stats.undecodable_ratio < 0.05,
            "expected a near-zero undecodable ratio for normal prose with a few symbols, got {}",
            stats.undecodable_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_ratio_helper_excludes_cjk_kana_hangul_emoji() {
        let text = "\u{65E5}\u{672C}\u{8A9E} \u{D55C}\u{AD6D}\u{C5B4} \u{4E2D}\u{6587} \
                    \u{3072}\u{3089}\u{304C}\u{306A} \u{30AB}\u{30BF}\u{30AB}\u{30CA} \
                    with latin words and emoji \u{1F600}\u{1F680}";
        let stats = NativeTextStats::from(text);
        assert_eq!(
            stats.undecodable_ratio, 0.0,
            "CJK/Kana/Hangul/emoji must not count as undecodable, got {}",
            stats.undecodable_ratio
        );
    }

    /// A text layer that decodes almost entirely into the Unicode Private Use Area — the
    /// signature of a `Type0`/`Identity-H` font with `CIDToGIDMap /Identity`, no
    /// `/ToUnicode` CMap, and an embedded subset with neither `cmap` nor `post` — must be
    /// routed to OCR exactly like a scanned page, even though it has a full, visible,
    /// glyph-rich text layer (issue #1254).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_undecodable_text_layer_routes_to_ocr() {
        let garbage = pua_garbage_text();
        let decision = evaluate_native_text_for_ocr(&garbage, Some(1), &t());
        assert!(
            decision.fallback,
            "a page whose text layer is mostly undecodable glyph indices must trigger OCR fallback"
        );
    }

    /// Normal prose that happens to contain a handful of real symbols (bullets, trademark
    /// signs, section marks) must NOT be misclassified as an undecodable text layer.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_normal_text_with_symbols_does_not_route_to_ocr() {
        let text = "This is a normal paragraph with meaningful words \u{2022} and one bullet symbol, \
                    plus a trademark\u{2122} and a section sign \u{00A7} sprinkled in for good measure. \
                    It contains multiple sentences that form a coherent, legible text block.";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(
            !decision.fallback,
            "normal prose with a few symbols must not trigger OCR fallback via the undecodable-ratio signal"
        );
    }

    /// Builds a gate decision with explicit fallback / whole-document-failure
    /// flags and otherwise-empty stats, for exercising `evaluate_ocr_skip_gate`
    /// independently of the native-text heuristics.
    #[cfg(feature = "ocr")]
    fn gate_decision(fallback: bool, whole_doc_failure: bool) -> OcrFallbackDecision {
        OcrFallbackDecision {
            stats: NativeTextStats::from(""),
            avg_non_whitespace: 0.0,
            avg_alnum: 0.0,
            fallback,
            failing_pages: Vec::new(),
            whole_doc_failure,
        }
    }

    /// A scanned page with a garbage/undecodable text layer produces a
    /// pre-rendered structured doc plus enough low-alphanumeric characters to
    /// look "non-textual", but the per-document check flags the whole document.
    /// The whole-document failure must win over the non-text skip and route to
    /// OCR, otherwise a scanner PDF is silently returned as empty native text
    /// (issue #1338).
    #[cfg(feature = "ocr")]
    #[test]
    fn test_whole_doc_failure_overrides_non_text_skip() {
        let thresholds = t();
        let outcome = evaluate_ocr_skip_gate(
            true, // pre-rendered structured doc present
            50,
            0.1, // < alnum_ws_ratio_threshold (0.4): looks non-textual
            &gate_decision(true, true),
            &thresholds,
        );
        assert_eq!(
            outcome,
            OcrGateOutcome::RunFallback,
            "a whole-document quality failure must route to OCR, not SkipNonText"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_fallback_overrides_non_text_skip() {
        let thresholds = t();
        let mut decision = gate_decision(true, false);
        decision.failing_pages = vec![3];

        let outcome = evaluate_ocr_skip_gate(true, thresholds.non_text_min_chars, 0.1, &decision, &thresholds);

        assert_eq!(outcome, OcrGateOutcome::RunFallbackOnPages(vec![3]));
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_dot_leader_toc_fallback_overrides_non_text_ratio() {
        let thresholds = t();
        let text = "Introduction....................1\nMethods.................7";
        let total_chars = text.chars().count();
        let alnum_ws_chars = text
            .chars()
            .filter(|character| character.is_alphanumeric() || character.is_whitespace())
            .count();
        let alnum_ws_ratio = alnum_ws_chars as f64 / total_chars as f64;
        let mut decision = gate_decision(true, false);
        decision.failing_pages = vec![1];

        assert!((alnum_ws_ratio - 0.374).abs() < 0.002);
        assert!(alnum_ws_ratio < thresholds.alnum_ws_ratio_threshold);
        assert!(total_chars >= thresholds.non_text_min_chars);
        assert_eq!(
            evaluate_ocr_skip_gate(true, total_chars, alnum_ws_ratio, &decision, &thresholds),
            OcrGateOutcome::RunFallbackOnPages(vec![1])
        );
    }

    /// A genuinely non-textual *structured* document (a rendered diagram whose
    /// stray label characters are mostly punctuation) that still passes the
    /// per-document quality check must keep skipping OCR — the guard must not
    /// over-trigger and OCR every diagram.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_non_text_structured_doc_still_skips_ocr() {
        let thresholds = t();
        let outcome = evaluate_ocr_skip_gate(true, 50, 0.1, &gate_decision(false, false), &thresholds);
        assert_eq!(
            outcome,
            OcrGateOutcome::SkipNonText,
            "a non-textual structured doc that passes the quality check must still skip OCR"
        );
    }

    /// A genuinely scanned page (no native text layer at all) must still route to OCR,
    /// preserving pre-existing behavior alongside the new undecodable-text-layer trigger.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_scanned_empty_page_still_routes_to_ocr() {
        let decision = evaluate_native_text_for_ocr("   \n\t  ", Some(1), &t());
        assert!(decision.fallback, "an empty/scanned page must still route to OCR");
        assert_eq!(decision.stats.undecodable_ratio, 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_single_bad_page_triggers() {
        use crate::types::PageBoundary;

        let text = "Good text on page one with meaningful content.\x00\x00\x00";
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 46,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 46,
                byte_end: text.len(),
            },
        ];
        let decision = evaluate_per_page_ocr(text, Some(&boundaries), Some(2), &t());
        assert!(decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_multi_page_garbage_threshold_routes_only_failing_page() {
        use crate::types::PageBoundary;

        let thresholds = t();
        let garbage = "\u{FFFD}".repeat(thresholds.min_garbage_chars);
        let first_page = format!(
            "This first page contains reliable searchable measurements and explanatory text. \
             The native layer remains useful despite a few replacement characters {garbage}."
        );
        let second_page = "This second page contains reliable searchable measurements and explanatory text. \
                           Its native text layer should remain untouched by OCR.";
        let text = format!("{first_page}{second_page}");
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: first_page.len(),
            },
            PageBoundary {
                page_number: 2,
                byte_start: first_page.len(),
                byte_end: text.len(),
            },
        ];

        let decision = evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &thresholds);

        assert!(
            decision.fallback,
            "the page at the configured threshold must still route to OCR"
        );
        assert_eq!(decision.failing_pages, vec![1]);
        assert!(
            !decision.whole_doc_failure,
            "aggregate replacement characters must not force whole-document OCR"
        );
        assert_eq!(
            evaluate_ocr_skip_gate(false, text.len(), 0.9, &decision, &thresholds),
            OcrGateOutcome::RunFallbackOnPages(vec![1])
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_multi_page_garbage_threshold_without_boundaries_uses_aggregate_fallback() {
        let thresholds = t();
        let garbage = "\u{FFFD}".repeat(thresholds.min_garbage_chars);
        let text = format!(
            "This multi-page document contains reliable searchable measurements and explanatory text. \
             Its replacement characters still require aggregate fallback without page boundaries {garbage}."
        );

        let decision = evaluate_per_page_ocr(&text, None, Some(2), &thresholds);

        assert!(decision.fallback);
        assert!(decision.whole_doc_failure);
        assert!(decision.failing_pages.is_empty());
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_multi_page_garbage_threshold_with_invalid_boundaries_uses_aggregate_fallback() {
        use crate::types::PageBoundary;

        let thresholds = t();
        let garbage = "\u{FFFD}".repeat(thresholds.min_garbage_chars);
        let text = format!(
            "This multi-page document contains reliable searchable measurements and explanatory text. \
             Its replacement characters still require aggregate fallback with stale boundaries {garbage}."
        );
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: text.len() + 1,
                byte_end: text.len() + 2,
            },
            PageBoundary {
                page_number: 2,
                byte_start: text.len() + 2,
                byte_end: text.len() + 3,
            },
        ];

        let decision = evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &thresholds);

        assert!(decision.fallback);
        assert!(decision.whole_doc_failure);
        assert!(decision.failing_pages.is_empty());
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_multi_page_garbage_threshold_outside_boundaries_uses_aggregate_fallback() {
        use crate::types::PageBoundary;

        let thresholds = t();
        let first_page = "This first page contains reliable searchable measurements and explanatory text.";
        let garbage = "\u{FFFD}".repeat(thresholds.min_garbage_chars);
        let second_page = "This second page also contains reliable searchable measurements and explanatory text.";
        let text = format!("{first_page}{garbage}{second_page}");
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: first_page.len(),
            },
            PageBoundary {
                page_number: 2,
                byte_start: first_page.len() + garbage.len(),
                byte_end: text.len(),
            },
        ];

        let decision = evaluate_per_page_ocr(&text, Some(&boundaries), Some(2), &thresholds);

        assert!(decision.fallback);
        assert!(decision.whole_doc_failure);
        assert!(decision.failing_pages.is_empty());
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_merge_empty_ocr_result_keeps_native_text() {
        use crate::types::PageBoundary;

        let native = "PAGE ONE NATIVE\nPAGE TWO NATIVE";
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 16,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 16,
                byte_end: native.len(),
            },
        ];
        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(2, String::new());

        let merged = merge_ocr_pages_into_native(native, &boundaries, &ocr_results);
        assert_eq!(
            merged, native,
            "an empty OCR result must not overwrite the page's native text"
        );
        assert!(merged.contains("PAGE TWO NATIVE"));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_merge_nonempty_ocr_result_replaces_native_text() {
        use crate::types::PageBoundary;

        let native = "PAGE ONE NATIVE\ngarbage page two";
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 16,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 16,
                byte_end: native.len(),
            },
        ];
        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(2, "CLEAN OCR PAGE TWO".to_string());

        let merged = merge_ocr_pages_into_native(native, &boundaries, &ocr_results);
        assert!(merged.contains("PAGE ONE NATIVE"));
        assert!(merged.contains("CLEAN OCR PAGE TWO"));
        assert!(!merged.contains("garbage page two"));
    }

    #[test]
    fn test_accepted_replacements_reject_empty_missing_duplicate_overlap_and_invalid_utf8() {
        use crate::types::PageBoundary;

        let native = "A•BCDE";
        let bullet = native.find('•').unwrap();
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: native.len(),
                byte_end: native.len(),
            },
            PageBoundary {
                page_number: 3,
                byte_start: 0,
                byte_end: 1,
            },
            PageBoundary {
                page_number: 3,
                byte_start: 1,
                byte_end: 1,
            },
            PageBoundary {
                page_number: 4,
                byte_start: bullet + 1,
                byte_end: native.len(),
            },
            PageBoundary {
                page_number: 5,
                byte_start: 0,
                byte_end: native.len(),
            },
            PageBoundary {
                page_number: 6,
                byte_start: 1,
                byte_end: native.len(),
            },
        ];
        let mut raw = ahash::AHashMap::new();
        raw.insert(1, "accepted".to_string());
        raw.insert(2, "missing boundary".to_string());
        raw.insert(3, "duplicate boundary".to_string());
        raw.insert(4, "invalid UTF-8 offset".to_string());
        raw.insert(5, "overlap one".to_string());
        raw.insert(6, "overlap two".to_string());
        raw.insert(7, "   ".to_string());

        let accepted = accepted_ocr_page_replacements(native, &boundaries, &raw, &OcrQualityThresholds::default());

        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted.get(&1).map(String::as_str), Some("accepted"));
    }

    #[test]
    fn test_zero_width_consecutive_replacements_have_deterministic_page_order() {
        use crate::types::PageBoundary;

        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: 0,
            },
            PageBoundary {
                page_number: 2,
                byte_start: 0,
                byte_end: 0,
            },
        ];
        let raw = ahash::AHashMap::from_iter([(2, "page two".to_string()), (1, "page one|".to_string())]);

        let accepted = accepted_ocr_page_replacements("", &boundaries, &raw, &OcrQualityThresholds::default());
        let merged = apply_ocr_page_replacements("", &boundaries, &accepted);

        assert_eq!(merged, "page one|page two");
    }

    #[test]
    fn test_structured_mixed_merge_preserves_assets_and_remaps_relationships() {
        use crate::types::internal::{
            ElementKind, InternalDocument, InternalElement, Relationship, RelationshipKind, RelationshipTarget,
        };

        let mut doc = InternalDocument::new("pdf");
        doc.tables.push(crate::types::Table {
            cells: vec![vec!["kept".to_string()]],
            markdown: "| kept |".to_string(),
            page_number: 2,
            bounding_box: None,
            ..Default::default()
        });
        doc.images.push(crate::types::ExtractedImage {
            image_index: 0,
            page_number: Some(2),
            ocr_result: Some(Box::new(crate::types::ExtractedDocument {
                content: "DUPLICATE INLINE OCR".to_string(),
                ..Default::default()
            })),
            ..Default::default()
        });
        let mut push = |kind, text: &str, page| {
            let mut element = InternalElement::text(kind, text, 0);
            element.page = page;
            doc.push_element(element);
        };
        push(ElementKind::Paragraph, "native page one", Some(1));
        push(ElementKind::PageBreak, "", None);
        push(ElementKind::ListStart { ordered: false }, "", None);
        push(ElementKind::ListItem { ordered: false }, "stale page two", Some(2));
        push(ElementKind::Table { table_index: 0 }, "", Some(2));
        push(ElementKind::Image { image_index: 0 }, "", Some(2));
        push(ElementKind::ListEnd, "", None);
        push(ElementKind::PageBreak, "", None);
        push(ElementKind::Paragraph, "native page three", Some(3));
        doc.elements[3].anchor = Some("removed-target".to_string());
        doc.elements[8].anchor = Some("retained-target".to_string());
        doc.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Index(5),
            kind: RelationshipKind::Caption,
        });
        doc.relationships.push(Relationship {
            source: 3,
            target: RelationshipTarget::Index(8),
            kind: RelationshipKind::Caption,
        });
        doc.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Key("retained-target".to_string()),
            kind: RelationshipKind::InternalLink,
        });
        doc.relationships.push(Relationship {
            source: 0,
            target: RelationshipTarget::Key("removed-target".to_string()),
            kind: RelationshipKind::InternalLink,
        });

        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2, "DUPLICATE INLINE OCR\n\nOCR paragraph two".to_string());
        merge_ocr_pages_into_internal_document(&mut doc, &ocr_results);

        let kinds: Vec<ElementKind> = doc.elements.iter().map(|element| element.kind).collect();
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| matches!(kind, ElementKind::PageBreak))
                .count(),
            2
        );
        assert!(!kinds.iter().any(|kind| matches!(kind, ElementKind::Table { .. })));
        assert_eq!(
            kinds
                .iter()
                .filter(|kind| matches!(kind, ElementKind::Image { .. }))
                .count(),
            1
        );
        assert!(
            !doc.elements
                .iter()
                .any(|element| element.text.contains("stale page two"))
        );
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::OcrText { .. }))
                .map(|element| element.text.as_str())
                .collect::<Vec<_>>(),
            vec!["DUPLICATE INLINE OCR", "OCR paragraph two"]
        );
        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.images.len(), 1);
        assert!(
            doc.images[0].ocr_result.is_some(),
            "public nested OCR data must be preserved"
        );
        doc.append_ocr_text = true;
        for rendered in [
            crate::rendering::render_plain(&doc),
            crate::rendering::render_markdown(&doc),
            crate::rendering::render_djot(&doc),
        ] {
            assert_eq!(
                rendered.matches("DUPLICATE INLINE OCR").count(),
                1,
                "whole-page OCR must suppress duplicate nested image OCR rendering: {rendered}"
            );
        }
        let derived = crate::extraction::derive::derive_extraction_result(
            doc.clone(),
            true,
            crate::core::config::OutputFormat::Plain,
        );
        let document = serde_json::to_string(derived.document.as_ref().expect("document structure must exist"))
            .expect("document structure must serialize");
        assert!(
            !document.contains("xberg:internal"),
            "internal renderer flags must not be public"
        );
        assert_eq!(doc.relationships.len(), 2);
        let RelationshipTarget::Index(target) = doc.relationships[0].target else {
            panic!("retained indexed relationship must stay resolved");
        };
        assert!(matches!(doc.elements[target as usize].kind, ElementKind::Image { .. }));
        assert!(matches!(doc.relationships[1].target, RelationshipTarget::Key(ref key) if key == "retained-target"));
        let ids: std::collections::HashSet<&str> = doc.elements.iter().map(|element| element.id.as_ref()).collect();
        assert_eq!(ids.len(), doc.elements.len(), "rebuilt element IDs must be unique");
    }

    #[test]
    fn test_structured_mixed_merge_inserts_missing_page_in_order() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "page one", 0).with_page(1));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "page three", 0).with_page(3));
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2, "new page two".to_string());

        merge_ocr_pages_into_internal_document(&mut doc, &ocr_results);

        let texts: Vec<&str> = doc
            .elements
            .iter()
            .filter(|element| !element.text.is_empty())
            .map(|element| element.text.as_str())
            .collect();
        assert_eq!(texts, vec!["page one", "new page two", "page three"]);
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::PageBreak))
                .count(),
            2
        );
    }

    #[test]
    fn test_structured_mixed_merge_prefers_page_document_and_keeps_text_fallback() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut native = InternalDocument::new("pdf");
        native.push_element(InternalElement::text(ElementKind::Paragraph, "native one", 0).with_page(1));
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale two", 0).with_page(2));
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale three", 0).with_page(3));

        let mut structured_page = InternalDocument::new("pdf");
        structured_page.push_element(
            InternalElement::text(ElementKind::Heading { level: 2 }, "Structured OCR heading", 0).with_page(1),
        );
        let empty_structured_page = InternalDocument::new("pdf");
        let structured_pages = ahash::AHashMap::from_iter([(2, structured_page), (3, empty_structured_page)]);
        let replacements =
            ahash::AHashMap::from_iter([(2, "flat OCR two".to_string()), (3, "fallback OCR three".to_string())]);

        merge_structured_ocr_pages_into_internal_document(&mut native, &replacements, &structured_pages);

        assert!(native.elements.iter().any(|element| {
            element.text == "Structured OCR heading"
                && element.page == Some(2)
                && matches!(element.kind, ElementKind::Heading { level: 2 })
        }));
        assert!(!native.elements.iter().any(|element| element.text == "flat OCR two"));
        assert!(native.elements.iter().any(|element| {
            element.text == "fallback OCR three"
                && element.page == Some(3)
                && matches!(element.kind, ElementKind::OcrText { .. })
        }));
    }

    /// A structured OCR page carrying assets is merged structurally, not flattened
    /// back to raw text (#57/#59). This previously asserted the opposite: the flat
    /// fallback ran and the page's table was lost.
    #[test]
    fn test_structured_mixed_merge_reindexes_pages_with_assets() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut native = InternalDocument::new("pdf");
        native.push_element(InternalElement::text(ElementKind::Paragraph, "stale page", 0).with_page(2));

        let mut structured_page = InternalDocument::new("pdf");
        structured_page.push_element(
            InternalElement::text(ElementKind::Heading { level: 2 }, "heading before table", 0).with_page(2),
        );
        structured_page.push_element(InternalElement::text(ElementKind::Table { table_index: 0 }, "", 0).with_page(2));
        structured_page.tables.push(crate::types::Table {
            markdown: "| value |\n| --- |\n| retained |".to_string(),
            page_number: 2,
            ..Default::default()
        });

        let structured_pages = ahash::AHashMap::from_iter([(2, structured_page)]);
        let replacements = ahash::AHashMap::from_iter([(
            2,
            "heading before table\n\n| value |\n| --- |\n| retained |".to_string(),
        )]);

        merge_structured_ocr_pages_into_internal_document(&mut native, &replacements, &structured_pages);

        assert_eq!(
            native.tables.len(),
            1,
            "the page's table must be merged into the parent"
        );
        assert_eq!(native.tables[0].markdown, "| value |\n| --- |\n| retained |");
        assert_eq!(native.tables[0].page_number, 2);
        assert!(
            native.elements.iter().any(|element| {
                element.text == "heading before table" && matches!(element.kind, ElementKind::Heading { level: 2 })
            }),
            "the structured heading must survive instead of being flattened to OCR text"
        );
        assert!(
            native
                .elements
                .iter()
                .any(|element| matches!(element.kind, ElementKind::Table { table_index: 0 })),
            "the table reference must be rebased onto the parent's collection"
        );
        assert!(
            !native
                .elements
                .iter()
                .any(|element| matches!(element.kind, ElementKind::OcrText { .. })),
            "the raw-text fallback must not run for a structurally merged page"
        );
        assert!(!native.elements.iter().any(|element| element.text == "stale page"));
    }

    #[test]
    fn test_empty_structured_page_keeps_recovered_flat_ocr_text() {
        let mut pages = vec![Some(Vec::new())];
        let page_texts = vec!["Recovered embedded image text".to_string()];

        fill_unstructured_ocr_pages(&mut pages, &page_texts);

        let paragraphs = pages[0].as_ref().expect("recovered page must be represented");
        assert_eq!(paragraphs.len(), 1);
        assert_eq!(paragraphs[0].text, "Recovered embedded image text");
    }

    #[test]
    fn test_structured_merge_handles_first_last_consecutive_and_textless_pages() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        for page in 1..=4 {
            doc.push_element(
                InternalElement::text(ElementKind::Paragraph, format!("native {page}"), 0).with_page(page),
            );
        }
        let replacements = ahash::AHashMap::from_iter([
            (1, "same OCR".to_string()),
            (2, "same OCR".to_string()),
            (4, "last OCR".to_string()),
            (5, "textless OCR".to_string()),
        ]);

        merge_ocr_pages_into_internal_document(&mut doc, &replacements);

        let texts: Vec<&str> = doc
            .elements
            .iter()
            .filter(|element| !element.text.is_empty())
            .map(|element| element.text.as_str())
            .collect();
        assert_eq!(
            texts,
            vec!["same OCR", "same OCR", "native 3", "last OCR", "textless OCR"]
        );
        let ids: std::collections::HashSet<&str> = doc.elements.iter().map(|element| element.id.as_ref()).collect();
        assert_eq!(
            ids.len(),
            doc.elements.len(),
            "repeated OCR text still needs unique IDs"
        );
        assert_eq!(
            doc.elements
                .iter()
                .filter(|element| matches!(element.kind, ElementKind::PageBreak))
                .count(),
            4
        );
    }

    #[test]
    fn test_container_analysis_keeps_only_balanced_same_page_markers() {
        use crate::types::internal::{ElementKind, InternalElement};

        let element = |kind, page| {
            let mut element = InternalElement::text(kind, "", 0);
            element.page = page;
            element
        };
        let elements = vec![
            element(ElementKind::ListStart { ordered: false }, None),
            element(ElementKind::GroupStart, Some(1)),
            element(ElementKind::Paragraph, Some(1)),
            element(ElementKind::GroupEnd, None),
            element(ElementKind::ListEnd, None),
            element(ElementKind::QuoteStart, None),
            element(ElementKind::Paragraph, Some(1)),
            element(ElementKind::Paragraph, Some(2)),
            element(ElementKind::QuoteEnd, None),
            element(ElementKind::ListEnd, None),
            element(ElementKind::GroupStart, None),
            element(ElementKind::ListStart { ordered: true }, Some(1)),
            element(ElementKind::QuoteStart, Some(1)),
            element(ElementKind::ListEnd, None),
            element(ElementKind::QuoteEnd, None),
        ];

        let analysis = analyze_container_markers(&elements);

        for index in [0, 1, 3, 4] {
            assert!(!analysis.drop_marker[index], "valid nested marker {index} must survive");
            assert_eq!(analysis.inferred_pages[index], Some(1));
        }
        for index in [5, 8, 9, 10, 11, 13] {
            assert!(analysis.drop_marker[index], "invalid marker {index} must be flattened");
        }
        assert!(
            !analysis.drop_marker[12],
            "independently balanced inner quote must survive"
        );
        assert!(
            !analysis.drop_marker[14],
            "independently balanced inner quote must survive"
        );
    }

    /// Boundaries can go stale when the text they index is rebuilt (e.g.
    /// reading-order reordering). A stale offset landing inside a multibyte
    /// character must be skipped, not panic the page.
    #[cfg(feature = "ocr")]
    #[test]
    fn test_per_page_ocr_non_char_boundary_offsets_skipped() {
        use crate::types::PageBoundary;

        let text = "This is a normal paragraph with meaningful words and proper structure. \
                    It contains multiple sentences • that form a coherent text block.";
        let mid_bullet = text.find('•').unwrap() + 1;
        assert!(!text.is_char_boundary(mid_bullet));
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: mid_bullet,
            },
            PageBoundary {
                page_number: 2,
                byte_start: mid_bullet,
                byte_end: text.len(),
            },
        ];
        let decision = evaluate_per_page_ocr(text, Some(&boundaries), Some(2), &t());
        assert!(
            decision.failing_pages.is_empty(),
            "stale non-char-boundary offsets must be skipped, not evaluated"
        );
    }

    /// Same staleness in the mixed OCR/native merge: a boundary that does not
    /// land on char boundaries must leave the native text untouched.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_merge_non_char_boundary_offsets_skipped() {
        use crate::types::PageBoundary;

        let native = "PAGE ONE • NATIVE\nPAGE TWO NATIVE";
        let mid_bullet = native.find('•').unwrap() + 1;
        assert!(!native.is_char_boundary(mid_bullet));
        let boundaries = vec![
            PageBoundary {
                page_number: 1,
                byte_start: 0,
                byte_end: mid_bullet,
            },
            PageBoundary {
                page_number: 2,
                byte_start: mid_bullet,
                byte_end: native.len(),
            },
        ];
        let mut ocr_results: ahash::AHashMap<u32, String> = ahash::AHashMap::new();
        ocr_results.insert(1, "OCR PAGE ONE".to_string());
        ocr_results.insert(2, "OCR PAGE TWO".to_string());

        let merged = merge_ocr_pages_into_native(native, &boundaries, &ocr_results);
        assert_eq!(
            merged, native,
            "stale non-char-boundary offsets must not be spliced into the native text"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_few_replacement_chars_no_fallback() {
        let text = "The quick \u{FFFD} brown fox jumps over the lazy dog repeatedly.";
        let stats = NativeTextStats::from(text);
        assert_eq!(stats.garbage_char_count, 1);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_consecutive_repeat_high_with_substantial_content_no_ocr() {
        let defaults = t();
        let mut words = Vec::new();
        for _ in 0..10 {
            words.extend_from_slice(&[
                "TALK", "TALK", "of", "of", "the", "the", "TOWN", "TOWN", "London", "London",
            ]);
        }
        let text = words.join(" ");
        let stats = NativeTextStats::from(&text);
        assert!(
            stats.consecutive_repeat_ratio >= defaults.min_consecutive_repeat_ratio,
            "ratio {} should be >= {}",
            stats.consecutive_repeat_ratio,
            defaults.min_consecutive_repeat_ratio
        );
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &defaults);

        assert!(
            !decision.fallback,
            "Substantial content should NOT trigger OCR even with high repeat ratio. \
             Stats: non_ws={}, avg_non_ws={:.2}",
            stats.non_whitespace, decision.avg_non_whitespace
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normal_text_no_consecutive_repeat_false_positive() {
        let defaults = t();
        let text = "The quick brown fox jumps over the lazy dog. This is a completely normal \
                    paragraph of text that forms coherent sentences. It contains multiple \
                    meaningful words and no unusual patterns of repetition. The text continues \
                    with more content that demonstrates typical English prose structure and \
                    vocabulary distribution across several sentences of varying length.";
        let stats = NativeTextStats::from(text);
        assert!(
            stats.consecutive_repeat_ratio < defaults.min_consecutive_repeat_ratio,
            "Normal text ratio {} should be < {}",
            stats.consecutive_repeat_ratio,
            defaults.min_consecutive_repeat_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_critical_fragmentation_triggers_fallback() {
        let defaults = t();
        let mut words: Vec<&str> = vec!["A"; 90];
        words.extend(vec!["document"; 10]);
        let text = words.join(" ");
        let stats = NativeTextStats::from(&text);
        assert!(
            stats.fragmented_word_ratio >= defaults.critical_fragmented_word_ratio,
            "fragmented ratio {} should be >= {}",
            stats.fragmented_word_ratio,
            defaults.critical_fragmented_word_ratio
        );
        assert!(stats.meaningful_words >= defaults.min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &defaults);
        assert!(
            decision.fallback,
            "Critical fragmentation should trigger OCR even with meaningful words"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_low_avg_word_length_triggers_fallback() {
        let defaults = t();
        let mut words: Vec<&str> = vec!["x"; 55];
        words.push("hello");
        words.push("world");
        words.push("testing");
        let text = words.join(" ");
        let stats = NativeTextStats::from(&text);
        assert!(stats.avg_word_length < defaults.min_avg_word_length);
        assert!(stats.word_count >= defaults.min_words_for_avg_length_check);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &defaults);
        assert!(decision.fallback, "Low avg word length should trigger OCR fallback");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normal_text_with_articles_no_false_positive() {
        let defaults = t();
        let text = "I am a fan of it. It is an old or new idea. A to do list is on my desk. \
                    He is in on it. We do go to it. I am at it. Is it so? He or I do it. \
                    The paragraph contains meaningful content with proper structure and sentences.";
        let stats = NativeTextStats::from(text);
        assert!(stats.meaningful_words >= defaults.min_meaningful_words);
        assert!(
            stats.fragmented_word_ratio < defaults.critical_fragmented_word_ratio,
            "Normal text fragmentation {} should be < {}",
            stats.fragmented_word_ratio,
            defaults.critical_fragmented_word_ratio
        );
        let decision = evaluate_native_text_for_ocr(text, Some(1), &defaults);
        assert!(
            !decision.fallback,
            "Normal text with short words should not trigger OCR"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_short_words_in_normal_text_no_false_positive() {
        let text = "I am a fan of this document. He is on to something here. \
                    We do have meaningful words like paragraph and structure throughout.";
        let stats = NativeTextStats::from(text);
        assert!(stats.meaningful_words >= t().min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(!decision.fallback);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_good_text() {
        let text = "This is a normal paragraph with meaningful words and proper structure. \
                    It contains multiple sentences that form a coherent text block.";
        let score = compute_quality_score(text, &t());
        assert!(score > 0.7, "Good text should score > 0.7, got {score}");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_empty_text() {
        assert_eq!(compute_quality_score("", &t()), 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_garbled_text() {
        let text = "x y z a b c d e f g h i j k l m n o p q r s t u v w";
        let score = compute_quality_score(text, &t());
        let good_score = compute_quality_score("This is a well-formed sentence with proper words and structure.", &t());
        assert!(
            score < good_score,
            "Garbled text ({score}) should score lower than good text ({good_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_zero_min_meaningful_words_no_panic() {
        let mut thresholds = t();
        thresholds.min_meaningful_words = 0;
        let score = compute_quality_score("hello world", &thresholds);
        assert!(score > 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_zero_min_consecutive_repeat_ratio_no_panic() {
        let mut thresholds = t();
        thresholds.min_consecutive_repeat_ratio = 0.0;
        let score = compute_quality_score("hello hello world world", &thresholds);
        assert!(score > 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_zero_min_garbage_chars_no_panic() {
        let mut thresholds = t();
        thresholds.min_garbage_chars = 0;
        let score = compute_quality_score("hello world testing", &thresholds);
        assert!(score > 0.0);
        let score_with_garbage = compute_quality_score("hello \u{FFFD} world", &thresholds);
        assert!(score > score_with_garbage);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_meaningful_words_not_capped() {
        let words: Vec<&str> = vec!["programming"; 50];
        let text = words.join(" ");
        let score = compute_quality_score(&text, &t());
        let stats = NativeTextStats::compute(&text, &t());
        assert_eq!(stats.meaningful_words, 50);
        let meaningful_score = (stats.meaningful_words as f64 / t().min_meaningful_words as f64).min(1.0);
        assert!(
            (meaningful_score - 1.0).abs() < f64::EPSILON,
            "meaningful_score should be 1.0 with 50 meaningful words, got {meaningful_score}"
        );
        assert!(
            score > 0.7,
            "Score with many meaningful words should be high, got {score}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_repeat_threshold_relative_normalization() {
        let thresholds = t();
        let text = "The quick brown fox jumps over the lazy dog near the stream. \
                    The quick brown fox jumps over the lazy dog near the stream. \
                    The quick brown fox jumps over the lazy dog near the stream.";
        let stats = NativeTextStats::compute(text, &thresholds);
        if stats.consecutive_repeat_ratio > 0.0
            && stats.consecutive_repeat_ratio < thresholds.min_consecutive_repeat_ratio
        {
            let expected_repeat_score =
                1.0 - (stats.consecutive_repeat_ratio / thresholds.min_consecutive_repeat_ratio).min(1.0);
            let _ = expected_repeat_score;
        }
        let half_ratio = thresholds.min_consecutive_repeat_ratio / 2.0;
        let expected = 1.0 - (half_ratio / thresholds.min_consecutive_repeat_ratio).min(1.0);
        assert!(
            (expected - 0.5).abs() < f64::EPSILON,
            "repeat_score at half threshold should be 0.5, got {expected}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_strictly_monotonic() {
        let thresholds = t();

        let perfect_text = "This document contains comprehensive analysis of market trends \
                           and provides detailed recommendations for future investment strategies. \
                           The methodology involves rigorous statistical examination of historical \
                           data patterns across multiple economic sectors and geographical regions.";

        let good_text = "This is a normal paragraph with meaningful words and proper structure. \
                        It contains multiple sentences that form a coherent text block.";

        let mediocre_text = "ok so um the uh thing is that we like need to uh figure out what \
                            to do about the um situation or whatever it is that happened here today";

        let garbled_text = "x y z a b c d e f g h i j k l m n o p q r s t u v w x y z a b";

        let empty_text = "";

        let perfect_score = compute_quality_score(perfect_text, &thresholds);
        let good_score = compute_quality_score(good_text, &thresholds);
        let mediocre_score = compute_quality_score(mediocre_text, &thresholds);
        let garbled_score = compute_quality_score(garbled_text, &thresholds);
        let empty_score = compute_quality_score(empty_text, &thresholds);

        assert!(
            perfect_score > good_score,
            "perfect ({perfect_score}) > good ({good_score})"
        );
        assert!(
            good_score > mediocre_score,
            "good ({good_score}) > mediocre ({mediocre_score})"
        );
        assert!(
            mediocre_score > garbled_score,
            "mediocre ({mediocre_score}) > garbled ({garbled_score})"
        );
        assert!(
            garbled_score > empty_score,
            "garbled ({garbled_score}) > empty ({empty_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normalize_markdown_for_scoring_strips_structure() {
        let input = "# Heading\n\n\
                     | Col A | Col B |\n| --- | --- |\n| one | two |\n\n\
                     - bullet item\n\
                     ```\ncode fence body\n```\n\
                     **bold** and _italic_ words";
        let out = normalize_markdown_for_scoring(input);
        assert!(!out.contains('|'), "table pipes removed: {out:?}");
        assert!(!out.contains('#'), "heading hashes removed: {out:?}");
        assert!(!out.contains('*') && !out.contains('_'), "emphasis removed: {out:?}");
        assert!(!out.contains("```"), "code fence markers removed: {out:?}");
        assert!(!out.contains("---"), "table separator row removed: {out:?}");
        assert!(out.contains("Heading"), "heading text kept: {out:?}");
        assert!(out.contains("bullet item"), "list text kept: {out:?}");
        assert!(
            out.contains("bold") && out.contains("italic"),
            "emphasized words kept: {out:?}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_normalize_markdown_for_scoring_strips_ordered_list_markers() {
        let input = "1. First item\n2) Second item\n12. Twelfth item\n10) Tenth item";
        let out = normalize_markdown_for_scoring(input);
        assert!(!out.contains("1."), "single-digit dot marker removed: {out:?}");
        assert!(!out.contains("2)"), "single-digit paren marker removed: {out:?}");
        assert!(!out.contains("12."), "multi-digit dot marker removed: {out:?}");
        assert!(!out.contains("10)"), "multi-digit paren marker removed: {out:?}");
        assert!(out.contains("First item"), "first item text kept: {out:?}");
        assert!(out.contains("Second item"), "second item text kept: {out:?}");
        assert!(out.contains("Twelfth item"), "twelfth item text kept: {out:?}");
        assert!(out.contains("Tenth item"), "tenth item text kept: {out:?}");
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_highest_score_keeps_max() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();

        // No current best: always replace.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            None,
            None,
            "some text",
            0.1,
            &thresholds
        ));
        // Strictly higher score replaces.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            Some(0.4),
            Some("prior text"),
            "better text",
            0.5,
            &thresholds
        ));
        // Equal or lower score does not replace.
        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            Some(0.5),
            Some("prior text"),
            "equal text",
            0.5,
            &thresholds
        ));
        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::HighestScore,
            Some(0.9),
            Some("prior text"),
            "worse text",
            0.2,
            &thresholds
        ));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_overrides_lower_score() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();

        // A later, non-empty, lower-scoring stage still replaces a higher-scoring
        // earlier stage under `PreferLastNonEmpty` (#1341: a correct-but-lower-score
        // VLM transcription must win over a higher-scoring but garbled classical
        // result). The incumbent is a merged-word artifact -- a single long token,
        // so its meaningful-word density is low despite the (unrealistically) high
        // score -- so the F46 guard has nothing dense worth protecting and does not
        // block the override.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.9),
            Some("mergedwordsallsquashedtogetherintooneunreadabletoken"),
            "correct vlm transcription of this document page",
            0.3,
            &thresholds
        ));
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_keeps_prior_on_empty_candidate() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();

        // An empty later-stage result (e.g. a VLM that declined a destroyed page)
        // never overwrites an existing non-empty best.
        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.4),
            Some("prior page content"),
            "   ",
            0.0,
            &thresholds
        ));
        // But an empty candidate still becomes the best when there is no prior best.
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            None,
            None,
            "",
            0.0,
            &thresholds
        ));
    }

    /// F46: a `PreferLastNonEmpty` candidate that is materially worse than a dense
    /// incumbent must not replace it, even though the policy otherwise always
    /// prefers the later, non-empty stage.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_rejects_degraded_candidate() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        // Dense, real-word incumbent: well above the density floor.
        let incumbent = "This document describes the quarterly financial results for the \
                          corporation including revenue growth expenses and forecasts for the \
                          next fiscal year across every operating region";
        assert!(
            meaningful_word_density_per_1000_chars(incumbent, &thresholds)
                .expect("fixture must carry enough tokens to judge")
                >= MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS,
            "test fixture must actually be dense enough to be worth protecting"
        );
        // Damaged candidate: non-empty, but recognition noise -- short garbled tokens
        // with no words at or above `min_meaningful_word_len`.
        let candidate = "xk 9z pq 1a bb cc dd ee ff gg hh ii jj kk ll mm nn oo";
        assert!(
            meaningful_word_density_per_1000_chars(candidate, &thresholds)
                .expect("fixture must carry enough tokens to judge")
                < MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS,
            "test fixture must actually be degraded"
        );

        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.9),
            Some(incumbent),
            candidate,
            0.95, // even a higher raw score does not override the density guard
            &thresholds
        ));
    }

    /// F46's guard is one-sided: an incumbent that was never dense in the first place
    /// has nothing worth protecting, so the later non-empty stage still wins by
    /// default even when its own density also falls under the floor.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_low_density_incumbent_still_replaced() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        let sparse_incumbent = "xk 9z pq 1a bb cc dd ee ff gg";
        let also_sparse_candidate = "zz 8y qw 2b cc dd ee ff gg hh";
        assert!(
            meaningful_word_density_per_1000_chars(sparse_incumbent, &thresholds)
                .expect("fixture must carry enough tokens to judge")
                < MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS
        );

        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.4),
            Some(sparse_incumbent),
            also_sparse_candidate,
            0.1,
            &thresholds
        ));
    }

    /// A VLM stage that emits Markdown must be judged on its prose, not on its scaffolding.
    /// The density guard scores the same normalized input as `compute_quality_score`; scoring
    /// raw text instead let a runaway table-separator row (a known LLM repetition-loop failure)
    /// inflate the non-whitespace denominator while the meaningful-word count stayed flat,
    /// vetoing a candidate whose actual content was fine.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn a_markdown_candidates_table_scaffolding_does_not_count_against_its_density() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        let incumbent = "This document describes the quarterly financial results for the \
                          corporation including revenue growth expenses and forecasts for the \
                          next fiscal year across every operating region";
        let separator = format!("|{}|", "-".repeat(4000));
        let candidate = format!(
            "This report contains detailed regional revenue growth analysis across every operating department.\n{separator}\n"
        );

        let raw_stats = NativeTextStats::compute(&candidate, &thresholds);
        assert!(
            raw_stats.meaningful_words as f64 / raw_stats.non_whitespace as f64 * 1000.0
                < MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS,
            "fixture must be one that an un-normalized density would wrongly reject"
        );
        assert!(
            meaningful_word_density_per_1000_chars(&candidate, &thresholds)
                .expect("fixture must carry enough tokens to judge")
                >= MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS,
            "normalized density must see through the separator row"
        );

        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.9),
            Some(incumbent),
            &candidate,
            0.5,
            &thresholds
        ));
    }

    /// Density is a ratio and so is blind to amount: a bare `"Page 12"` header scores far above
    /// the floor. An incumbent that short is not established content worth protecting, matching
    /// the token-floor convention `NativeTextStats::compute` already applies to its own ratios.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn a_too_short_incumbent_is_not_treated_as_dense_enough_to_protect() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        let stub_incumbent = "Page 12";
        let stats = NativeTextStats::compute(stub_incumbent, &thresholds);
        assert!(
            stats.meaningful_words as f64 / stats.non_whitespace as f64 * 1000.0
                >= MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS,
            "fixture must be one a raw ratio would wrongly call dense"
        );
        assert_eq!(
            meaningful_word_density_per_1000_chars(stub_incumbent, &thresholds),
            None,
            "too few tokens to judge"
        );

        let candidate = "This document describes the quarterly financial results for the \
                          corporation including revenue growth expenses and forecasts";
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.9),
            Some(stub_incumbent),
            candidate,
            0.1,
            &thresholds
        ));
    }

    /// The mirror case: an unjudgeably short candidate is not evidence of a better result, so a
    /// dense incumbent is kept rather than traded for a stub that a raw ratio would score high.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn a_too_short_candidate_does_not_replace_a_dense_incumbent() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        let incumbent = "This document describes the quarterly financial results for the \
                          corporation including revenue growth expenses and forecasts for the \
                          next fiscal year across every operating region";
        let stub_candidate = "Page 12";
        assert_eq!(
            meaningful_word_density_per_1000_chars(stub_candidate, &thresholds),
            None,
            "too few tokens to judge"
        );

        assert!(!should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.4),
            Some(incumbent),
            stub_candidate,
            0.95,
            &thresholds
        ));
    }

    /// F46's guard must not engage when there is no incumbent text to compare against
    /// (the first stage to produce anything becomes the best-effort result).
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn test_should_replace_best_effort_result_prefer_last_non_empty_no_incumbent_always_replaces() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        let candidate = "xk 9z pq 1a bb cc dd ee ff gg"; // low density, but there is nothing to protect
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            None,
            None,
            candidate,
            0.05,
            &thresholds
        ));
    }

    /// F46's density guard tokenizes on `split_whitespace`, which is a false premise for
    /// scripts that do not delimit words with spaces. A genuinely correct, dense CJK page
    /// (OCR line breaks are the only whitespace present; each line is one token no matter
    /// how many ideographs it holds) collapses to a handful of tokens while `non_whitespace`
    /// still counts every character, so density craters even though the text is dense and
    /// correct. The guard must not treat that as a materially degraded replacement.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn a_correct_dense_cjk_candidate_is_not_treated_as_degraded() {
        use crate::core::config::OcrPipelineSelection;

        let thresholds = OcrQualityThresholds::default();
        let incumbent = "This document describes the quarterly financial results for the \
                          corporation including revenue growth expenses and forecasts for the \
                          next fiscal year across every operating region";
        assert!(
            meaningful_word_density_per_1000_chars(incumbent, &thresholds)
                .expect("fixture must carry enough tokens to judge")
                >= MIN_VLM_OVERRIDE_WORD_DENSITY_PER_1000_CHARS,
            "test fixture must actually be dense enough to be worth protecting"
        );

        // Twelve lines of genuine, grammatical Mandarin describing the same quarterly
        // report as `incumbent`. Each line is a single whitespace-delimited token, so the
        // space-counting metric sees only 12 "words" against 343 characters.
        let cjk_candidate = "这是一份重要的季度财务报告文件，详细说明了公司本季度的整体经营状况\n\
                              报告详细说明了公司本季度的收入情况，也介绍了各项运营费用的支出记录\n\
                              此外还包含对下一财政年度的详细预测，以及未来发展方向的整体规划\n\
                              公司的营业利润呈现稳步上升的趋势，显示出良好的盈利能力和增长潜力\n\
                              现金流量状况良好并且负债水平保持稳定，财务结构整体健康稳健\n\
                              管理层认为这种增长趋势将持续到明年，并计划继续扩大市场份额\n\
                              许多分析师都看好公司未来的市场前景，纷纷上调了目标股价\n\
                              公司计划在新兴市场加大投资的力度，以寻求新的增长动力\n\
                              预计明年的销售额将会实现两位数增长，超过行业平均增长水平\n\
                              董事会已经批准了新的资本支出计划，用于扩建生产设施\n\
                              全体员工都对公司的发展充满信心，齐心协力迎接新的挑战\n\
                              公司将继续秉持稳健经营的原则，为股东创造长期价值";

        assert!(
            !candidate_is_materially_degraded(cjk_candidate, incumbent, &thresholds),
            "a correct, dense CJK transcription must not be flagged as a degraded replacement"
        );
        assert!(should_replace_best_effort_result(
            OcrPipelineSelection::PreferLastNonEmpty,
            Some(0.9),
            Some(incumbent),
            cjk_candidate,
            0.5,
            &thresholds
        ));
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_markdown_not_penalized() {
        // A VLM that emits correct, structured Markdown must not score materially below
        // the same prose without structure, or pipeline selection discards the richer
        // result in favor of a classical backend (#1341).
        let thresholds = t();
        let plain = "Quarterly revenue rose across every region this year. The northern \
                     division led growth while the southern division held steady and the \
                     eastern division recovered from the prior downturn this fiscal period.";
        let markdown = "## Quarterly revenue\n\n\
                        Quarterly revenue rose across every region this year.\n\n\
                        | Region | Trend |\n| --- | --- |\n| Northern | led growth |\n\
                        | Southern | held steady |\n| Eastern | recovered |\n\n\
                        - The northern division led growth this fiscal period\n\
                        - The southern division held steady while the eastern recovered";
        let plain_score = compute_quality_score(plain, &thresholds);
        let markdown_score = compute_quality_score(markdown, &thresholds);
        assert!(
            markdown_score >= plain_score - 0.05,
            "structured markdown ({markdown_score}) must not be heavily penalized vs plain prose ({plain_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_high_garbage_chars() {
        let thresholds = t();
        let text = format!("Hello world testing {} more words here", "\u{FFFD}".repeat(20));
        let score = compute_quality_score(&text, &thresholds);
        let clean_score = compute_quality_score("Hello world testing more words here", &thresholds);
        assert!(
            score < clean_score,
            "Text with garbage chars ({score}) should score lower than clean text ({clean_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_quality_score_high_consecutive_repetition() {
        let thresholds = t();
        let mut words = Vec::new();
        for _ in 0..30 {
            words.push("word");
            words.push("word");
        }
        let text = words.join(" ");
        let score = compute_quality_score(&text, &thresholds);
        let normal_score = compute_quality_score(
            "The quick brown fox jumps over the lazy dog repeatedly in various ways throughout the day",
            &thresholds,
        );
        assert!(
            score < normal_score,
            "Highly repetitive text ({score}) should score lower than normal text ({normal_score})"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_all_zeros() {
        let text = "... --- !!! @@@ ### $$$ %%% ^^^ &&& *** ((( )))";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &t());
        assert!(decision.fallback, "All non-alnum text should trigger fallback");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_garbage_at_threshold() {
        let thresholds = t();
        let garbage = "\u{FFFD}".repeat(thresholds.min_garbage_chars);
        let text = format!("Some normal text with garbage {garbage} embedded here");
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Text with garbage chars at threshold should trigger fallback"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_fragmented_few_meaningful() {
        let thresholds = t();
        let text = "I a b c d e f g h j k l m n o p q r s u";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert!(stats.fragmented_word_ratio >= thresholds.max_fragmented_word_ratio);
        assert!(stats.meaningful_words < thresholds.min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Fragmented + few meaningful words should trigger fallback"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_critical_fragmentation_with_meaningful_words() {
        let thresholds = t();
        let mut words: Vec<&str> = vec!["A"; 90];
        words.extend(vec!["document"; 10]);
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert!(stats.fragmented_word_ratio >= thresholds.critical_fragmented_word_ratio);
        assert!(stats.meaningful_words >= thresholds.min_meaningful_words);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Critical fragmentation triggers fallback even with meaningful words"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_low_avg_word_length() {
        let thresholds = t();
        let mut words: Vec<&str> = vec!["a"; 55];
        words.push("hello");
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert!(stats.avg_word_length < thresholds.min_avg_word_length);
        assert!(stats.word_count >= thresholds.min_words_for_avg_length_check);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);
        assert!(
            decision.fallback,
            "Low avg word length with enough words should trigger fallback"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_definitive_failure_high_consecutive_repeat_sparse() {
        let thresholds = t();

        let words = vec!["x"; 50];
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);

        assert!(
            stats.word_count >= thresholds.min_words_for_repeat_check,
            "Test setup: need >= {} words for repeat check, got {}",
            thresholds.min_words_for_repeat_check,
            stats.word_count
        );
        assert!(
            stats.consecutive_repeat_ratio >= thresholds.min_consecutive_repeat_ratio,
            "Test setup: should have high repeat ratio >= {}, got {:.2}",
            thresholds.min_consecutive_repeat_ratio,
            stats.consecutive_repeat_ratio
        );
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        if decision.avg_non_whitespace < MIN_AVG_NON_WHITESPACE_TO_TRUST {
            assert!(
                decision.fallback,
                "High consecutive repeat on sparse content should trigger fallback"
            );
        } else {
            eprintln!("Text is borderline sparse: {:.2} chars", decision.avg_non_whitespace);
        }
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_non_definitive_fails_on_alnum_ratio() {
        let thresholds = t();
        let text = "a!@# b%^ c*( d_+";
        let stats = NativeTextStats::compute(text, &thresholds);
        if stats.alnum > 0 && stats.alnum_ratio < thresholds.min_alnum_ratio && stats.non_whitespace != 0 {
            let decision = evaluate_native_text_for_ocr(text, Some(1), &thresholds);
            assert!(
                decision.fallback,
                "Low alnum ratio should trigger fallback through non-definitive path"
            );
        }
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_text_passes_all_checks() {
        let thresholds = t();
        let text = "This is a well-structured document containing multiple meaningful sentences. \
                    The content provides detailed information about various topics including \
                    science, technology, engineering, and mathematics. Each paragraph builds \
                    upon the previous one to create a comprehensive narrative that demonstrates \
                    proper text extraction quality from the PDF document format.";
        let decision = evaluate_native_text_for_ocr(text, Some(1), &thresholds);
        assert!(!decision.fallback, "Well-formed text should pass all checks");
        assert!(decision.stats.meaningful_words >= thresholds.min_meaningful_words);
        assert!(decision.stats.alnum_ratio >= thresholds.min_alnum_ratio);
        assert!(decision.stats.garbage_char_count < thresholds.min_garbage_chars);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_meaningful_words_actual_count_not_capped() {
        let thresholds = t();
        let words: Vec<&str> = vec!["programming"; 20];
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert_eq!(
            stats.meaningful_words, 20,
            "meaningful_words should be 20 (not capped), got {}",
            stats.meaningful_words
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_fragmented_word_ratio_calculation() {
        let thresholds = t();
        let text = "I a am b so the one quick brown fox";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert_eq!(stats.word_count, 10);
        let expected_ratio = 5.0 / 10.0;
        assert!(
            (stats.fragmented_word_ratio - expected_ratio).abs() < 0.01,
            "fragmented_word_ratio should be ~{expected_ratio}, got {}",
            stats.fragmented_word_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_fragmented_word_ratio_below_10_words() {
        let thresholds = t();
        let text = "a b c d e f g h i";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert_eq!(stats.word_count, 9);
        assert_eq!(
            stats.fragmented_word_ratio, 0.0,
            "fragmented_word_ratio should be 0.0 with < 10 words"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_consecutive_repeat_ratio_calculation() {
        let thresholds = t();
        let mut words = Vec::new();
        for _ in 0..25 {
            words.push("alpha");
            words.push("beta");
        }
        let text = words.join(" ");
        let stats = NativeTextStats::compute(&text, &thresholds);
        assert_eq!(stats.word_count, 50);
        assert!(
            stats.consecutive_repeat_ratio < 0.01,
            "Alternating words should have ~0 repeat ratio, got {}",
            stats.consecutive_repeat_ratio
        );

        let mut repeat_words = Vec::new();
        for _ in 0..25 {
            repeat_words.push("same");
            repeat_words.push("same");
        }
        let repeat_text = repeat_words.join(" ");
        let repeat_stats = NativeTextStats::compute(&repeat_text, &thresholds);
        assert!(
            repeat_stats.consecutive_repeat_ratio > 0.4,
            "All-same words should have high repeat ratio, got {}",
            repeat_stats.consecutive_repeat_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_consecutive_repeat_below_min_words() {
        let thresholds = t();
        let text = "same same same";
        let stats = NativeTextStats::compute(text, &thresholds);
        assert!(stats.word_count < thresholds.min_words_for_repeat_check);
        assert_eq!(
            stats.consecutive_repeat_ratio, 0.0,
            "consecutive_repeat_ratio should be 0.0 below word threshold"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_empty_string() {
        let thresholds = t();
        let stats = NativeTextStats::compute("", &thresholds);
        assert_eq!(stats.non_whitespace, 0);
        assert_eq!(stats.alnum, 0);
        assert_eq!(stats.meaningful_words, 0);
        assert_eq!(stats.alnum_ratio, 0.0);
        assert_eq!(stats.garbage_char_count, 0);
        assert_eq!(stats.fragmented_word_ratio, 0.0);
        assert_eq!(stats.consecutive_repeat_ratio, 0.0);
        assert_eq!(stats.avg_word_length, 0.0);
        assert_eq!(stats.word_count, 0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_single_word() {
        let thresholds = t();
        let stats = NativeTextStats::compute("hello", &thresholds);
        assert_eq!(stats.word_count, 1);
        assert_eq!(stats.non_whitespace, 5);
        assert_eq!(stats.alnum, 5);
        assert_eq!(stats.meaningful_words, 1);
        assert_eq!(stats.avg_word_length, 5.0);
        assert_eq!(stats.fragmented_word_ratio, 0.0);
        assert_eq!(stats.consecutive_repeat_ratio, 0.0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_stats_single_char() {
        let thresholds = t();
        let stats = NativeTextStats::compute("x", &thresholds);
        assert_eq!(stats.word_count, 1);
        assert_eq!(stats.non_whitespace, 1);
        assert_eq!(stats.alnum, 1);
        assert_eq!(stats.meaningful_words, 0);
        assert_eq!(stats.avg_word_length, 1.0);
    }

    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn test_process_document_propagation() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::path::Path;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};

        struct MockBackend {
            called: Arc<AtomicBool>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for MockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                panic!("Should not call process_image");
            }
            fn supports_document_processing(&self) -> bool {
                true
            }
            async fn process_document(&self, path: &Path, _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                assert!(path.to_string_lossy().contains("test.pdf"));
                self.called.store(true, Ordering::SeqCst);
                Ok(ExtractedDocument::default())
            }
        }

        impl Plugin for MockBackend {
            fn name(&self) -> &str {
                "mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let called = Arc::new(AtomicBool::new(false));
        let backend = Arc::new(MockBackend { called: called.clone() });
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "mock".to_string(),
                ..Default::default()
            }),
            content_filter: Some(crate::core::config::ContentFilterConfig {
                include_headers: true,
                include_footers: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::plugins::register_ocr_backend(backend).unwrap();

        let path = Path::new("test.pdf");
        let result = extract_with_ocr(
            None,
            Some(&[]),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            Some(path),
        )
        .await;

        assert!(result.is_ok());
        assert!(called.load(Ordering::SeqCst), "process_document was not called");
        let (_, _, _, _, _, llm_usage, _, _, _, _, _) = result.unwrap();
        assert!(llm_usage.is_empty(), "No LLM usage expected for mock backend");

        crate::plugins::unregister_ocr_backend("mock").unwrap();
    }

    /// GH#1554 regression, scanned-page route: `ExtractionConfig::security_limits` must
    /// reach the `OcrConfig` handed to `OcrBackend::process_image` for a scanned PDF page
    /// OCR'd through [`extract_with_ocr`] -- the actual per-page render+OCR route the issue's
    /// headline case (ordinary 300-600 dpi scans refused) goes through, distinct from the
    /// embedded-image route covered by `extraction_config_security_limits_reach_embedded_
    /// image_ocr_config` in `extraction/image_ocr.rs`. Before this fix `OcrConfig` had no
    /// `security_limits` field reachable from this route at all, so a caller's configured,
    /// possibly higher, limit could never reach a backend here -- every scanned page decoded
    /// under `SecurityLimits::default()` regardless of what the caller configured.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn extraction_config_security_limits_reach_scanned_page_ocr_config() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::{Arc, Mutex};

        const BACKEND_NAME: &str = "scanned-page-security-limits-capture-test-backend";

        struct SecurityLimitsCaptureBackend {
            observed_max_content_size: Arc<Mutex<Option<Option<usize>>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for SecurityLimitsCaptureBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], config: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let observed = config.security_limits.as_ref().map(|limits| limits.max_content_size);
                *self.observed_max_content_size.lock().unwrap() = Some(observed);
                Ok(ExtractedDocument::default())
            }
        }

        impl Plugin for SecurityLimitsCaptureBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let observed_max_content_size = Arc::new(Mutex::new(None));
        crate::plugins::register_ocr_backend(Arc::new(SecurityLimitsCaptureBackend {
            observed_max_content_size: Arc::clone(&observed_max_content_size),
        }))
        .unwrap();
        struct Guard;
        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = crate::plugins::unregister_ocr_backend(BACKEND_NAME);
            }
        }
        let _guard = Guard;

        let configured_limit = 200 * 1024 * 1024;
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            security_limits: Some(crate::extractors::security::SecurityLimits {
                max_content_size: configured_limit,
                ..Default::default()
            }),
            ..Default::default()
        };

        let tiny_png = {
            use image::ImageEncoder;
            use image::codecs::png::PngEncoder;
            use std::io::Cursor;
            let img = image::DynamicImage::new_rgb8(1, 1);
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut buf = Cursor::new(Vec::new());
            PngEncoder::new(&mut buf)
                .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                .unwrap();
            image::load_from_memory(&buf.into_inner()).unwrap()
        };

        let result = extract_with_ocr(
            None,
            Some(&[tiny_png]),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        result.expect("scanned-page OCR must succeed");

        let observed = observed_max_content_size.lock().unwrap();
        assert_eq!(
            *observed,
            Some(Some(configured_limit)),
            "OcrConfig::security_limits must carry the caller's ExtractionConfig::security_limits \
             on the scanned-page OCR route"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn should_filter_public_ocr_elements_by_effective_pdf_margins() {
        use crate::core::config::OcrConfig;
        use crate::types::{OcrBoundingGeometry, OcrElement, OcrElementConfig, OcrPoint};

        let element_at = |text: &str, top: u32| OcrElement {
            text: text.to_string(),
            geometry: OcrBoundingGeometry::Rectangle {
                left: 10,
                top,
                width: 100,
                height: 20,
            },
            ..Default::default()
        };
        let mut elements = vec![
            element_at("header", 10),
            element_at("body", 400),
            OcrElement {
                text: "footer".to_string(),
                geometry: OcrBoundingGeometry::Quadrilateral {
                    points: vec![
                        OcrPoint { x: 10, y: 950 },
                        OcrPoint { x: 110, y: 950 },
                        OcrPoint { x: 110, y: 980 },
                        OcrPoint { x: 10, y: 980 },
                    ],
                },
                ..Default::default()
            },
        ];

        let config = OcrConfig {
            element_config: Some(OcrElementConfig {
                include_elements: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let (elements, outcome) = public_ocr_elements_for_pdf_page(
            &mut elements,
            &config,
            7,
            1000,
            crate::pdf::native::text::PageMarginFractions {
                top: 0.10,
                bottom: 0.10,
            },
        );

        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "body");
        assert_eq!(elements[0].page_number, 7);
        assert!(outcome.removed);
        assert!(!outcome.missing_geometry);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn should_preserve_public_ocr_elements_when_page_height_is_unknown() {
        use crate::core::config::OcrConfig;
        use crate::types::{OcrBoundingGeometry, OcrElement, OcrElementConfig};

        let mut elements = vec![OcrElement {
            text: "unknown position".to_string(),
            geometry: OcrBoundingGeometry::Rectangle {
                left: 10,
                top: 10,
                width: 100,
                height: 20,
            },
            ..Default::default()
        }];
        let config = OcrConfig {
            element_config: Some(OcrElementConfig {
                include_elements: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let (elements, outcome) = public_ocr_elements_for_pdf_page(
            &mut elements,
            &config,
            1,
            0,
            crate::pdf::native::text::PageMarginFractions {
                top: 0.10,
                bottom: 0.10,
            },
        );

        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "unknown position");
        assert!(!outcome.removed);
        assert!(outcome.missing_geometry);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn should_use_per_page_ocr_only_when_effective_margins_are_nonzero() {
        assert!(!should_use_document_processing(
            true,
            true,
            crate::pdf::native::text::PageMarginFractions::default(),
        ));
        assert!(should_use_document_processing(
            true,
            true,
            crate::pdf::native::text::PageMarginFractions { top: 0.0, bottom: 0.0 },
        ));
        assert!(!should_use_document_processing(
            true,
            false,
            crate::pdf::native::text::PageMarginFractions { top: 0.0, bottom: 0.0 },
        ));
    }

    /// Verifies that `llm_usage` entries returned by a VLM OCR backend are
    /// accumulated per-page and returned from `extract_with_ocr`.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn test_llm_usage_propagated_through_extract_with_ocr() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        struct VlmMockBackend;

        #[async_trait::async_trait]
        impl OcrBackend for VlmMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(xobject_test_payload("page text"))
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for VlmMockBackend {
            fn name(&self) -> &str {
                "vlm-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = Arc::new(VlmMockBackend);
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "vlm-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        crate::plugins::register_ocr_backend(backend).unwrap();

        let tiny_png = {
            use image::ImageEncoder;
            use image::codecs::png::PngEncoder;
            use std::io::Cursor;
            let img = image::DynamicImage::new_rgb8(1, 1);
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut buf = Cursor::new(Vec::new());
            PngEncoder::new(&mut buf)
                .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                .unwrap();
            image::load_from_memory(&buf.into_inner()).unwrap()
        };
        let images = vec![tiny_png.clone(), tiny_png];

        let result = extract_with_ocr(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("vlm-mock").unwrap();

        let (_, _, tables, _, _, llm_usage, _, _, formulas, preprocessing, _) =
            result.expect("extract_with_ocr should succeed");
        assert_eq!(
            llm_usage.len(),
            2,
            "should have one LlmUsage entry per page, got {}",
            llm_usage.len()
        );
        assert_eq!(llm_usage[0].model, "recovery-model");
        assert_eq!(llm_usage[0].source, "vlm_ocr");
        assert_eq!(llm_usage[0].total_tokens, Some(150));
        assert_eq!(tables.iter().map(|table| table.page_number).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(
            formulas.iter().map(|formula| formula.page).collect::<Vec<_>>(),
            [Some(1), Some(2)]
        );
        assert_eq!(preprocessing.len(), 2);
        assert_eq!(preprocessing[&1].target_dpi, 321);
        assert_eq!(preprocessing[&2].target_dpi, 321);
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[tokio::test]
    #[serial_test::serial]
    async fn accepted_fallback_retains_prior_stage_diagnostics() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const FAILED_BACKEND: &str = "accepted-fallback-primary-failure";
        const FALLBACK_BACKEND: &str = "accepted-fallback-success";
        const UNAVAILABLE_BACKEND: &str = "accepted-fallback-unavailable";
        const FALLBACK_TEXT: &str =
            "This readable fallback result contains enough natural language words to clear the OCR quality threshold.";

        struct FailedPrimaryBackend;
        struct AcceptedFallbackBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FailedPrimaryBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }

            fn supports_language(&self, _: &str) -> bool {
                true
            }

            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Err(crate::XbergError::Parsing {
                    message: "synthetic primary failure".to_string(),
                    source: None,
                })
            }
        }

        impl Plugin for FailedPrimaryBackend {
            fn name(&self) -> &str {
                FAILED_BACKEND
            }

            fn version(&self) -> String {
                "1.0.0".to_string()
            }

            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }

            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        #[async_trait::async_trait]
        impl OcrBackend for AcceptedFallbackBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }

            fn supports_language(&self, _: &str) -> bool {
                true
            }

            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: FALLBACK_TEXT.to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for AcceptedFallbackBackend {
            fn name(&self) -> &str {
                FALLBACK_BACKEND
            }

            fn version(&self) -> String {
                "1.0.0".to_string()
            }

            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }

            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FailedPrimaryBackend)).unwrap();
        crate::plugins::register_ocr_backend(Arc::new(AcceptedFallbackBackend)).unwrap();

        let pipeline = OcrPipelineConfig {
            stages: vec![
                OcrPipelineStage {
                    backend: FAILED_BACKEND.to_string(),
                    priority: 120,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
                OcrPipelineStage {
                    backend: UNAVAILABLE_BACKEND.to_string(),
                    priority: 110,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
                OcrPipelineStage {
                    backend: FALLBACK_BACKEND.to_string(),
                    priority: 100,
                    language: None,
                    tesseract_config: None,
                    paddle_ocr_config: None,
                    vlm_config: None,
                    backend_options: None,
                },
            ],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.05,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };
        let images = vec![image::DynamicImage::new_rgb8(16, 16)];

        let result = run_ocr_pipeline(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(FAILED_BACKEND).unwrap();
        crate::plugins::unregister_ocr_backend(FALLBACK_BACKEND).unwrap();

        let (text, _, _, doc, _, _, _, _, _, _) = result.expect("fallback stage must be accepted");
        assert_eq!(text, FALLBACK_TEXT);
        let warnings = doc
            .expect("accepted fallback diagnostics require an internal document")
            .processing_warnings;
        assert!(
            warnings
                .iter()
                .any(|warning| warning.message.contains(FAILED_BACKEND) && warning.message.contains("failed")),
            "primary-stage failure must survive accepted fallback: {warnings:?}"
        );
        assert!(
            warnings.iter().any(
                |warning| warning.message.contains(UNAVAILABLE_BACKEND)
                    && warning.message.contains("unavailable")
            ),
            "unavailable requested stage must be surfaced: {warnings:?}"
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_build_page_raster_image_fields() {
        let png_bytes = bytes::Bytes::from_static(b"\x89PNG\r\n\x1a\n");
        let img = build_page_raster_image(0, png_bytes.clone(), 800, 600);

        assert_eq!(img.page_number, Some(1), "page_number must be 1-indexed");
        assert_eq!(img.width, Some(800));
        assert_eq!(img.height, Some(600));
        assert_eq!(img.format.as_ref(), "png");
        assert_eq!(img.image_kind, Some(crate::types::ImageKind::PageRaster));
        assert_eq!(img.colorspace.as_deref(), Some("RGB"));
        assert_eq!(img.bits_per_component, Some(8));
        assert!(!img.is_mask);
        assert!(img.bounding_box.is_none());
        assert!(img.ocr_result.is_none());
        assert_eq!(img.data, png_bytes);
        assert_eq!(img.image_index, 0, "image_index is a placeholder; caller must reindex");
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_build_page_raster_image_page_idx_to_page_number() {
        for page_idx in 0usize..5 {
            let img = build_page_raster_image(page_idx, bytes::Bytes::new(), 100, 100);
            assert_eq!(
                img.page_number,
                Some((page_idx + 1) as u32),
                "page_number must be page_idx + 1"
            );
        }
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v2_unlimited_returns_none() {
        assert_eq!(parse_cgroup_v2("max\n", "12345"), None);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v2_numeric_saturating_subtraction() {
        assert_eq!(parse_cgroup_v2("1000000000\n", "250000000\n"), Some(750_000_000));
        assert_eq!(parse_cgroup_v2("100", "500"), Some(0));
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v2_invalid_returns_none() {
        assert_eq!(parse_cgroup_v2("not-a-number", "0"), None);
        assert_eq!(parse_cgroup_v2("1000", "not-a-number"), None);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v1_unlimited_sentinel_returns_none() {
        let unlimited = usize::MAX.to_string();
        assert_eq!(parse_cgroup_v1(&unlimited, "0"), None);

        let just_under = (isize::MAX as usize - 1).to_string();
        assert!(parse_cgroup_v1(&just_under, "0").is_some());
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_cgroup_v1_numeric_saturating_subtraction() {
        assert_eq!(parse_cgroup_v1("2000000", "500000"), Some(1_500_000));
        assert_eq!(parse_cgroup_v1("100", "500"), Some(0));
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_meminfo_available_extracts_kb_and_converts_to_bytes() {
        let synthetic = "\
MemTotal:        8000000 kB
MemFree:         1000000 kB
MemAvailable:       2048 kB
Buffers:           50000 kB
";
        assert_eq!(parse_meminfo_available(synthetic), 2048 * 1024);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_meminfo_available_missing_field_returns_zero() {
        let synthetic = "MemTotal: 8000000 kB\nMemFree: 1000000 kB\n";
        assert_eq!(parse_meminfo_available(synthetic), 0);
    }

    #[cfg(all(feature = "ocr", target_os = "linux"))]
    #[test]
    fn parse_meminfo_available_handles_unparseable_value_as_zero() {
        let synthetic = "MemAvailable: notanumber kB\n";
        assert_eq!(parse_meminfo_available(synthetic), 0);
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn shared_rendered_pages_preserve_order_content_and_backing_buffers() {
        let first = image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(2, 1, vec![1, 2, 3, 4, 5, 6]).unwrap());
        let second = image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(1, 1, vec![7, 8, 9]).unwrap());
        let first_pixels = first.as_bytes().as_ptr();
        let second_pixels = second.as_bytes().as_ptr();

        let shared = share_rendered_page_images(vec![(4, first), (1, second)]);

        assert_eq!(
            shared.iter().map(|(page_idx, _)| *page_idx).collect::<Vec<_>>(),
            vec![4, 1]
        );
        assert_eq!(shared[0].1.as_bytes(), &[1, 2, 3, 4, 5, 6]);
        assert_eq!(shared[1].1.as_bytes(), &[7, 8, 9]);
        assert_eq!(shared[0].1.as_bytes().as_ptr(), first_pixels);
        assert_eq!(shared[1].1.as_bytes().as_ptr(), second_pixels);

        let task_image = std::sync::Arc::clone(&shared[0].1);
        assert!(std::sync::Arc::ptr_eq(&task_image, &shared[0].1));
    }

    /// Pipeline-level test for the actual bug path in #1078 (force_ocr_pages / mixed
    /// path uses render_selected_pages_for_ocr; full force_ocr uses similar batch
    /// render in extract_with_ocr).
    /// This proves the wide PDF no longer hard-fails through the OCR render path
    /// that was crashing in production.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn test_render_selected_pages_for_ocr_wide_pdf_does_not_fail() {
        let wide_pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(20000.0, 300.0);
        let result = render_selected_pages_for_ocr(&wide_pdf, &[0]);
        assert!(
            result.is_ok(),
            "render_selected_pages_for_ocr on wide page (the #1078 bug path) should succeed via safeguard, got: {:?}",
            result.err()
        );
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn full_pdf_ocr_reuses_open_document_across_bounded_batches() {
        let pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let (doc, page_count, page_rotations) = open_pdf_for_full_ocr(&pdf).unwrap();

        assert_eq!(page_count, 1);
        let first_batch = render_full_pdf_ocr_batch(
            &doc,
            &page_rotations,
            0..1,
            &crate::extractors::security::SecurityLimits::default(),
        )
        .unwrap();
        assert_eq!(first_batch.len(), 1);
        assert_eq!(first_batch[0].0, 0);
        drop(first_batch);

        let second_batch = render_full_pdf_ocr_batch(
            &doc,
            &page_rotations,
            0..1,
            &crate::extractors::security::SecurityLimits::default(),
        )
        .unwrap();
        assert_eq!(second_batch.len(), 1);
        assert_eq!(second_batch[0].0, 0);
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_all_out_of_range_pages_skips_backend_lookup() {
        let pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let mut config = ExtractionConfig::default();
        config.ocr = Some(crate::core::config::OcrConfig {
            backend: "unregistered-test-backend".to_string(),
            ..Default::default()
        });

        let result = extract_mixed_ocr_native("native", &[], &[99], &pdf, &config, None)
            .await
            .unwrap();

        assert_eq!(result.0, "native");
        assert!(result.1.is_empty());
        assert!(result.2.is_empty());
        assert!(result.3.is_empty());
        assert!(result.4.is_none());
        assert!(result.5.is_empty());
        assert!(result.6.is_empty());
    }

    /// Root of the shared `test_documents` fixture corpus, resolved relative to this
    /// crate's manifest dir (mirrors the identically named helper in
    /// `crate::pdf::native::images` test module).
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn test_documents_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_documents")
    }

    /// Minimal 2-page PDF (no content streams, just two bare `/Page` objects) for
    /// tests that need `extract_mixed_ocr_native` to target a specific *later* page.
    /// Mirrors `crate::pdf::render::build_minimal_pdf_with_mediabox`, which already
    /// renders successfully with no content stream.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn build_minimal_two_page_pdf(w: f32, h: f32) -> Vec<u8> {
        let mut buf = Vec::<u8>::new();
        buf.extend_from_slice(b"%PDF-1.4\n");

        let obj1_offset = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");

        let obj2_offset = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<</Type /Pages /Kids [3 0 R 4 0 R] /Count 2>>\nendobj\n");

        let mb = format!("[0 0 {} {}]", w, h);
        let obj3_offset = buf.len();
        buf.extend_from_slice(format!("3 0 obj\n<</Type /Page /MediaBox {} /Parent 2 0 R>>\nendobj\n", mb).as_bytes());
        let obj4_offset = buf.len();
        buf.extend_from_slice(format!("4 0 obj\n<</Type /Page /MediaBox {} /Parent 2 0 R>>\nendobj\n", mb).as_bytes());

        let xref_offset = buf.len();
        buf.extend_from_slice(b"xref\n");
        buf.extend_from_slice(b"0 5\n");
        buf.extend_from_slice(b"0000000000 65535 f \n");
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj3_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj4_offset).as_bytes());

        buf.extend_from_slice(b"trailer\n<</Size 5 /Root 1 0 R>>\n");
        buf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        buf
    }

    /// Regression test (review follow-up to #1341): the nested `run_ocr_pipeline`
    /// call for a single page assembles its aggregate text as if that lone image
    /// were page 1 of the document, so a configured page marker is stamped "PAGE 1"
    /// regardless of which real page is being OCR'd. When only a LATER page (page 2
    /// here) is routed through the pipeline route, the merged output must carry the
    /// raw backend text with no leaked "PAGE 1" marker from the nested call.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_later_page_pipeline_route_does_not_leak_page_one_marker() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, PageConfig};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct FixedTextBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FixedTextBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "OCR PAGE TWO CONTENT".to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FixedTextBackend {
            fn name(&self) -> &str {
                "later-page-marker-test-backend"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FixedTextBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);

        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                // An explicit pipeline (rather than `vlm_fallback`) so the test can
                // name its own mock backend instead of the hardcoded "vlm" name.
                pipeline: Some(OcrPipelineConfig {
                    stages: vec![OcrPipelineStage {
                        backend: "later-page-marker-test-backend".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    }],
                    quality_thresholds: crate::core::config::OcrQualityThresholds::default(),
                }),
                ..Default::default()
            }),
            pages: Some(PageConfig {
                insert_page_markers: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[2], &pdf, &config, None)
            .await
            .unwrap();
        let merged = result.0;

        assert!(
            merged.contains("OCR PAGE TWO CONTENT"),
            "merged output must contain the OCR'd page 2 text: {merged:?}"
        );
        assert!(
            !merged.contains("PAGE 1"),
            "merged output must not leak a page-1 marker from the nested single-image pipeline call: {merged:?}"
        );
        assert!(
            merged.contains(page1_text),
            "page 1's native text must be untouched: {merged:?}"
        );

        crate::plugins::unregister_ocr_backend("later-page-marker-test-backend").unwrap();
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_pipeline_later_page_warning_uses_document_page_number() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct LowQualityBackend;

        #[async_trait::async_trait]
        impl OcrBackend for LowQualityBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "A B C D E F G H I J K L M N O P Q R S T U V W X Y Z A B C D".to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for LowQualityBackend {
            fn name(&self) -> &str {
                "below-threshold-warning-test-backend"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(LowQualityBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(OcrPipelineConfig {
                    stages: vec![OcrPipelineStage {
                        backend: "below-threshold-warning-test-backend".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    }],
                    quality_thresholds: OcrQualityThresholds {
                        pipeline_min_quality: 0.0,
                        discard_suspected_ocr_noise: true,
                        ..Default::default()
                    },
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[2], &pdf, &config, None)
            .await
            .unwrap();
        let warnings = result.8;

        assert_eq!(
            result.0, native_text,
            "destructive opt-in must not replace native text with noise"
        );
        let noise_warnings = warnings
            .iter()
            .filter(|warning| warning.message.contains("suspected OCR recognition noise"))
            .collect::<Vec<_>>();
        assert_eq!(
            noise_warnings.len(),
            1,
            "pipeline noise warning must propagate exactly once"
        );
        assert!(noise_warnings[0].message.contains("discarded"));
        assert!(
            noise_warnings[0].message.contains("Page 2"),
            "warning must name the detached image's document page: {noise_warnings:?}"
        );
        assert!(
            !noise_warnings[0].message.contains("Page 1"),
            "warning must not report the detached image as local page 1: {noise_warnings:?}"
        );

        crate::plugins::unregister_ocr_backend("below-threshold-warning-test-backend").unwrap();
    }

    #[cfg(all(feature = "pdf", feature = "ocr"))]
    #[tokio::test]
    #[serial_test::serial]
    async fn mixed_direct_route_retains_overlapping_noise_signals_with_one_warning() {
        use crate::core::config::{OcrConfig, OcrQualityThresholds};
        use crate::plugins::{ConfidenceSemantics, OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, Metadata, PageBoundary};
        use std::sync::Arc;

        const BACKEND_NAME: &str = "mixed-noise-diagnostic-test-backend";
        const SUSPECTED_NOISE: &str = "A B C D E F G H I J K L M N O P Q R S T U V W X Y Z A B C D";

        struct NoiseDiagnosticBackend;

        #[async_trait::async_trait]
        impl OcrBackend for NoiseDiagnosticBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let mut metadata = Metadata::default();
                metadata
                    .additional
                    .insert("mean_text_conf".into(), serde_json::json!(18.0));
                metadata.additional.insert(
                    crate::ocr_metadata_keys::OCR_TESSERACT_DICT_INVALID_WORD_RATIO_METADATA_KEY.into(),
                    serde_json::json!(0.9),
                );
                Ok(ExtractedDocument {
                    content: SUSPECTED_NOISE.to_string(),
                    metadata,
                    ..Default::default()
                })
            }
            fn confidence_semantics(&self) -> ConfidenceSemantics {
                ConfidenceSemantics::Legibility { scale_max: 100.0 }
            }
        }

        impl Plugin for NoiseDiagnosticBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(NoiseDiagnosticBackend)).unwrap();
        let pdf = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let native_text = "native text";
        let boundaries = [PageBoundary {
            byte_start: 0,
            byte_end: native_text.len(),
            page_number: 1,
        }];
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                quality_thresholds: Some(OcrQualityThresholds {
                    max_ocr_output_dict_invalid_word_ratio: 0.5,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };

        let images = [image::DynamicImage::new_rgb8(100, 100)];
        let full_result = extract_with_ocr(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await
        .unwrap();
        let result = extract_mixed_ocr_native(native_text, &boundaries, &[1], &pdf, &config, None)
            .await
            .unwrap();
        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        assert_eq!(full_result.0, SUSPECTED_NOISE);
        assert_eq!(full_result.6, [SUSPECTED_NOISE]);
        let full_warnings = full_result
            .4
            .as_ref()
            .expect("diagnostic warning must attach to the full OCR document")
            .processing_warnings
            .iter()
            .filter(|warning| warning.message.contains("suspected OCR recognition noise"))
            .collect::<Vec<_>>();
        assert_eq!(full_warnings.len(), 1, "full OCR must report overlapping signals once");
        assert_eq!(result.0, SUSPECTED_NOISE);
        assert_eq!(result.1.get(&1).map(String::as_str), Some(SUSPECTED_NOISE));
        let warnings = result
            .8
            .iter()
            .filter(|warning| warning.message.contains("suspected OCR recognition noise"))
            .collect::<Vec<_>>();
        assert_eq!(warnings.len(), 1, "overlapping signals must produce one warning");
        for reason in ["mean confidence", "1-2 characters", "dictionary-invalid", "retained"] {
            assert!(
                full_warnings[0].message.contains(reason),
                "full OCR warning omitted {reason}: {full_warnings:?}"
            );
            assert!(
                warnings[0].message.contains(reason),
                "warning omitted {reason}: {warnings:?}"
            );
        }
    }

    /// The recognition-noise verdict computed inside `accept_or_reject_ocr_page` used to be
    /// discarded one frame before `run_ocr_pipeline_for_page`'s accept/reject decision --
    /// `extract_with_ocr_for_page` returned only a warning string, never the numbers that
    /// justified it. This proves the numeric verdict now survives the whole call: a page whose
    /// fragmented-word ratio crosses `max_ocr_output_fragmented_word_ratio` surfaces
    /// `fragmented_noise: true` and the exact ratio in the function's return, with the other
    /// two independent signals (`low_confidence`, `dictionary_noise`) correctly reported as
    /// not having fired.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    #[serial_test::serial]
    async fn extract_with_ocr_for_page_surfaces_recognition_noise_verdict() {
        use crate::core::config::{OcrConfig, OcrQualityThresholds};
        use crate::plugins::{ConfidenceSemantics, OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "fragmented-noise-verdict-test-backend";
        // 30 single-letter tokens: every scorable word is <=2 chars, so `fragmented_word_ratio`
        // is exactly 1.0 -- well past the default 0.35 threshold -- and `word_count` (30)
        // clears the default `min_words_for_ocr_output_check` (20). ~keep
        const FRAGMENTED_CONTENT: &str = "A B C D E F G H I J K L M N O P Q R S T U V W X Y Z A B C D";

        struct FragmentedNoiseBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FragmentedNoiseBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: FRAGMENTED_CONTENT.to_string(),
                    ..Default::default()
                })
            }
            fn confidence_semantics(&self) -> ConfidenceSemantics {
                ConfidenceSemantics::Uncalibrated
            }
        }

        impl Plugin for FragmentedNoiseBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FragmentedNoiseBackend)).unwrap();

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                quality_thresholds: Some(OcrQualityThresholds::default()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let images = [image::DynamicImage::new_rgb8(100, 100)];
        let result = extract_with_ocr_for_page(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
            0,
            false,
            None,
            0,
        )
        .await
        .unwrap();

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let verdicts = result.12;
        assert_eq!(
            verdicts.len(),
            1,
            "exactly one page ran OCR and should have produced one verdict: {verdicts:?}"
        );
        let verdict = &verdicts[0];
        assert!(
            verdict.fragmented_noise,
            "fragmented-word ratio must have crossed the threshold: {verdict:?}"
        );
        assert_eq!(
            verdict.fragmented_word_ratio, 1.0,
            "every scorable word is <=2 chars, so the ratio must be exactly 1.0"
        );
        assert_eq!(verdict.word_count, 30);
        assert!(
            !verdict.low_confidence,
            "backend reports no calibrated confidence, so this signal must not fire"
        );
        assert!(
            !verdict.dictionary_noise,
            "backend reports no dictionary-invalid ratio, so this signal must not fire"
        );
    }

    /// A sibling test (or a real caller) invoking the public `clear_ocr_backends()` API empties
    /// the process-global OCR registry, including the built-in `tesseract` backend. Generated
    /// language-binding e2e suites exercise exactly this: a backend-management test calls the
    /// equivalent of `clear_ocr_backends()` in the same process as later OCR-dispatch tests, and
    /// test order across suites/classes is not pinned, so a later test can see an empty registry.
    ///
    /// `ensure_ocr_backends_initialized` (`plugins::ocr::ensure_ocr_backends_initialized`) exists
    /// precisely to heal this -- re-seeding the built-in defaults whenever the registry is
    /// missing one -- and is already wired into a handful of call sites (`extract_with_ocr_for_page`,
    /// the `ocr_inline_images` path in `extractors::pdf::mod.rs`, `doctor::ocr`). It was never
    /// wired into `run_ocr_pipeline_for_page`'s own stage-availability check, which reads the
    /// registry directly: a cleared registry made every stage read as unavailable and the pipeline
    /// fail immediately with "No available OCR backends", regardless of what any individual stage's
    /// own dispatch could otherwise have healed. This proves the pipeline route now self-heals too.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    #[serial_test::serial]
    async fn pipeline_recovers_default_backend_after_global_registry_cleared() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};

        crate::plugins::clear_ocr_backends().expect("clearing the global OCR registry must succeed");

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: "tesseract".to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let images = [image::DynamicImage::new_rgb8(4, 4)];
        let result = run_ocr_pipeline(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        if let Err(ref error) = result {
            let message = error.to_string();
            assert!(
                !message.contains("No available OCR backends"),
                "the built-in `tesseract` backend must be re-seeded before pipeline stage \
                 selection runs, even though the global registry was cleared just before this \
                 call: {message}"
            );
        }
    }

    /// #1444: `run_ocr_pipeline` has no OCR execution loop of its own -- every stage is
    /// delegated to `extract_with_ocr`, which already carries the force_ocr image-XObject
    /// fallback (#1355, lines ~2593-2630). This proves that delegation actually threads
    /// the recovered text and its warning through the pipeline route's return value: a
    /// page whose OCR text comes back blank, but whose page carries a real (decodable)
    /// image XObject, must be retried against the embedded image bytes and the recovered
    /// text must replace the blank result.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_blank_pipeline_page_and_warn_when_embedded_image_exists() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const BACKEND_NAME: &str = "pipeline-blank-recovery-test-backend";
        const RECOVERED_TEXT: &str = "RECOVERED PAGE TEXT";

        // Returns blank text on the first call (the full-page render) and recovered
        // text on every later call (the force_ocr fallback retry over embedded image
        // bytes). The fixture has exactly one page, so the call order is deterministic.
        struct BlankThenRecoverBackend {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for BlankThenRecoverBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let call_number = self.calls.fetch_add(1, Ordering::SeqCst);
                if call_number == 0 {
                    Ok(ExtractedDocument::default())
                } else {
                    Ok(xobject_test_payload(RECOVERED_TEXT))
                }
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for BlankThenRecoverBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        crate::plugins::register_ocr_backend(Arc::new(BlankThenRecoverBackend { calls: calls.clone() })).unwrap();

        // Single page, exactly one embedded (DCT/JPEG) image XObject -- verified via
        // `mutool info -I` and already relied on by
        // `test_page_ocr_fallback_image_bytes_recovers_real_image` in
        // `crate::pdf::native::images`.
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        let pdf_bytes = std::fs::read(&pdf_path).expect("failed to read test PDF fixture");

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            // Accept the first (only) stage unconditionally so the test exercises the
            // fallback recovery in isolation, not the best-effort selection branch.
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            pdf_options: Some(pdf_config_with_disabled_page_margins()),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, tables, _, doc, usage, _, _, formulas, preprocessing, _) =
            result.expect("pipeline run must succeed");
        assert_eq!(
            text, RECOVERED_TEXT,
            "recovered fallback text must replace the blank OCR result in the pipeline route"
        );
        assert_eq!(
            calls.load(Ordering::SeqCst),
            2,
            "expected exactly one full-page OCR call and one fallback OCR call"
        );
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].model, "recovery-model");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].page_number, 1);
        assert_eq!(formulas.len(), 1);
        assert_eq!(formulas[0].page, Some(1));
        assert_eq!(preprocessing[&1].target_dpi, 321);

        let warnings = doc
            .expect("the fallback warning must produce an internal document")
            .processing_warnings;
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one processing warning, got: {warnings:?}"
        );
        assert_eq!(warnings[0].source.as_ref(), "ocr");
        assert_eq!(
            warnings[0].message.as_ref(),
            "Page 1 rendered blank but contains 1 image XObject(s) the PDF rasterizer could not draw; \
             OCR was retried on the embedded image bytes."
        );
    }

    /// #1444: a blank OCR result on a page with no embedded image XObjects has nothing
    /// to recover from. The pipeline route must not fabricate content and must not emit
    /// the force_ocr fallback warning -- `page_ocr_fallback_image_bytes` degrades to an
    /// empty vec, and the reference implementation's `if !fallback_images.is_empty()`
    /// guard is a no-op in that case.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_not_warn_or_fabricate_content_when_blank_pipeline_page_has_no_embedded_images() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "pipeline-no-image-blank-test-backend";

        struct AlwaysBlankBackend;

        #[async_trait::async_trait]
        impl OcrBackend for AlwaysBlankBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument::default())
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for AlwaysBlankBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(AlwaysBlankBackend)).unwrap();

        // No content stream, no resources, no image XObjects.
        let pdf_bytes = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, doc, _, _, _, _, _, _) = result.expect("pipeline run must succeed");
        assert_eq!(text, "", "a page with no recoverable images must not fabricate content");
        assert!(
            doc.is_none(),
            "a page with no recoverable images must not produce an internal document, got: {doc:?}"
        );
    }

    /// #1444: the force_ocr fallback warning is deliberately unconditional on recovery
    /// succeeding (reference implementation, ocr.rs ~2618-2630) -- it fires whenever the
    /// page had image XObjects worth retrying, even if the retry also comes back blank.
    /// The defining property of the original bug is silence, so the warning is the part
    /// that must never be skipped, independent of whether any text was recovered.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_warn_when_pipeline_fallback_ocr_recovers_nothing() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "pipeline-unrecoverable-blank-test-backend";

        struct AlwaysBlankBackend;

        #[async_trait::async_trait]
        impl OcrBackend for AlwaysBlankBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument::default())
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for AlwaysBlankBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(AlwaysBlankBackend)).unwrap();

        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        let pdf_bytes = std::fs::read(&pdf_path).expect("failed to read test PDF fixture");

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, doc, _, _, _, _, _, _) = result.expect("pipeline run must succeed");
        assert_eq!(
            text, "",
            "an unrecovered blank page must not fabricate content even though the warning fires"
        );

        let warnings = doc
            .expect("the fallback warning must produce an internal document even with no recovered text")
            .processing_warnings;
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one processing warning, got: {warnings:?}"
        );
        assert_eq!(warnings[0].source.as_ref(), "ocr");
        assert_eq!(
            warnings[0].message.as_ref(),
            "Page 1 rendered blank but contains 1 image XObject(s) the PDF rasterizer could not draw; \
             OCR was retried on the embedded image bytes."
        );
    }

    /// Verifies that formulas returned by a per-page OCR backend are accumulated and
    /// renumbered to 1-indexed document page numbers by `extract_with_ocr`.
    ///
    /// This exercises the same `formula.page = (page_idx + 1) as u32` accumulation
    /// logic that is now replicated in `extract_mixed_ocr_native` for the mixed-OCR
    /// path. Since `extract_mixed_ocr_native` requires real PDF bytes for rendering,
    /// this test uses `extract_with_ocr` with in-memory images to validate that the
    /// accumulation pattern works correctly end-to-end.
    #[cfg(feature = "ocr")]
    #[tokio::test]
    async fn test_formulas_accumulated_and_renumbered_per_page() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{BoundingBox, ExtractedDocument};
        use std::sync::Arc;

        struct FormulaMockBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FormulaMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "page text".to_string(),
                    formulas: vec![crate::types::Formula {
                        latex: "E = mc^2".to_string(),
                        bbox: Some(BoundingBox {
                            x0: 0.0,
                            y0: 0.0,
                            x1: 100.0,
                            y1: 50.0,
                        }),
                        page: None,
                    }],
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FormulaMockBackend {
            fn name(&self) -> &str {
                "formula-mock-mixed-ocr"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = Arc::new(FormulaMockBackend);
        crate::plugins::register_ocr_backend(backend).unwrap();

        let tiny_image = {
            use image::ImageEncoder;
            use image::codecs::png::PngEncoder;
            use std::io::Cursor;
            let img = image::DynamicImage::new_rgb8(1, 1);
            let rgb = img.to_rgb8();
            let (w, h) = rgb.dimensions();
            let mut buf = Cursor::new(Vec::new());
            PngEncoder::new(&mut buf)
                .write_image(&rgb, w, h, image::ColorType::Rgb8.into())
                .unwrap();
            image::load_from_memory(&buf.into_inner()).unwrap()
        };
        let images = vec![tiny_image.clone(), tiny_image];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "formula-mock-mixed-ocr".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            None,
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("formula-mock-mixed-ocr").unwrap();

        let (_, _, _, _, _, _, _, _, formulas, _, _) = result.expect("extract_with_ocr should succeed");

        assert_eq!(formulas.len(), 2, "one formula per page, got {}", formulas.len());

        let mut pages: Vec<u32> = formulas
            .iter()
            .map(|f| f.page.expect("OCR formulas must have a renumbered page"))
            .collect();
        pages.sort_unstable();
        assert_eq!(
            pages,
            vec![1, 2],
            "formula pages must be renumbered to 1-indexed doc pages"
        );

        assert!(
            formulas.iter().all(|f| f.latex == "E = mc^2"),
            "formula latex must be preserved through accumulation"
        );
    }

    /// Test that inject_layout_config_to_backend handles non-object backend_options
    /// by replacing with a fresh object instead of silently dropping the flag.
    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    #[test]
    fn test_inject_layout_config_handles_non_object_backend_options() {
        use crate::core::config::LayoutDetectionConfig;
        let ocr_config = crate::core::config::OcrConfig {
            backend_options: Some(serde_json::json!("invalid")),
            ..Default::default()
        };

        let extraction_config = ExtractionConfig {
            layout: Some(LayoutDetectionConfig {
                enable_chart_understanding: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = inject_layout_config_to_backend(&ocr_config, &extraction_config);

        assert!(result.backend_options.is_some());
        let opts = result.backend_options.unwrap();
        assert!(opts.is_object());
        assert_eq!(
            opts.get("enable_chart_understanding").and_then(|v| v.as_bool()),
            Some(true),
            "enable_chart_understanding should be injected into the new object"
        );
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    #[test]
    fn layout_ocr_config_should_force_word_elements_for_internal_consumers() {
        let config = crate::core::config::OcrConfig {
            element_config: Some(crate::types::OcrElementConfig {
                include_elements: false,
                min_level: crate::types::OcrElementLevel::Line,
                min_confidence: 0.75,
                build_hierarchy: true,
            }),
            ..Default::default()
        };

        let result = ensure_elements_enabled(&config);
        let element_config = result.element_config.expect("layout OCR must request elements");

        assert!(element_config.include_elements);
        assert_eq!(element_config.min_level, crate::types::OcrElementLevel::Word);
        assert_eq!(element_config.min_confidence, 0.75);
        assert!(element_config.build_hierarchy);
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    #[test]
    fn public_ocr_elements_should_preserve_requested_granularity_and_confidence() {
        let elements = vec![
            test_ocr_element("word", crate::types::OcrElementLevel::Word, 0.9),
            test_ocr_element("weak word", crate::types::OcrElementLevel::Word, 0.4),
            test_ocr_element("line", crate::types::OcrElementLevel::Line, 0.8),
            test_ocr_element("block", crate::types::OcrElementLevel::Block, 0.95),
            test_ocr_element("page", crate::types::OcrElementLevel::Page, 0.95),
        ];
        let no_elements = crate::core::config::OcrConfig::default();
        let line_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Line, 0.5);
        let word_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Word, 0.5);
        let block_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Block, 0.0);
        let page_elements = ocr_config_requesting_elements(crate::types::OcrElementLevel::Page, 0.0);

        assert!(filter_public_ocr_elements(&elements, &no_elements).is_empty());
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &line_elements)),
            vec!["line", "block", "page"]
        );
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &word_elements)),
            vec!["word", "line", "block", "page"]
        );
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &block_elements)),
            vec!["block", "page"]
        );
        assert_eq!(
            element_texts(filter_public_ocr_elements(&elements, &page_elements)),
            vec!["page"]
        );
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    fn test_ocr_element(
        text: &str,
        level: crate::types::OcrElementLevel,
        recognition: f64,
    ) -> crate::types::OcrElement {
        crate::types::OcrElement {
            text: text.to_string(),
            level,
            confidence: crate::types::OcrConfidence {
                detection: None,
                recognition,
            },
            ..Default::default()
        }
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    fn ocr_config_requesting_elements(
        min_level: crate::types::OcrElementLevel,
        min_confidence: f64,
    ) -> crate::core::config::OcrConfig {
        crate::core::config::OcrConfig {
            element_config: Some(crate::types::OcrElementConfig {
                include_elements: true,
                min_level,
                min_confidence,
                build_hierarchy: true,
            }),
            ..Default::default()
        }
    }

    #[cfg(all(feature = "layout-detection", feature = "ocr"))]
    fn element_texts(elements: Vec<crate::types::OcrElement>) -> Vec<String> {
        elements.into_iter().map(|element| element.text).collect()
    }

    /// Simulate NICS background checks table: many short numeric tokens.
    /// Characteristics:
    /// - Substantial non-whitespace content (1000+ chars)
    /// - Many short numeric tokens (1-4 chars, e.g., "0", "100", "500")
    /// - High fragmented_word_ratio (~70%)
    /// - Low avg_word_length (~2.5)
    /// - High consecutive_repeat_ratio (repeated numbers)
    #[cfg(feature = "ocr")]
    fn numeric_table_text() -> String {
        let mut text = String::new();
        for row in 0..20 {
            for col in 0..15 {
                let val = (row * col) % 1000;
                text.push_str(&format!("{} ", val));
            }
            text.push('\n');
        }
        text
    }

    /// Simulate math formula page: mix of words and short tokens.
    /// Real formula pages have "where", "define", "equation", "therefore" mixed with symbols.
    /// Characteristics:
    /// - Mixture of long and short tokens
    /// - Substantial content if multiple equations
    /// - Some fragmentation from mathematical notation
    /// - But not extreme critical fragmentation (< 0.80)
    #[cfg(feature = "ocr")]
    fn formula_text() -> String {
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!(
                "Definition {}: where variable equals expression and function applies therefore x y z\n",
                i
            ));
        }
        text
    }

    /// Simulate sparse form with short tokens: checkboxes, small fields.
    /// Characteristics:
    /// - Few non-whitespace chars (<30 per page, genuinely sparse)
    /// - Short tokens
    /// - Should trigger OCR (legitimately sparse, not just non-prose)
    #[cfg(feature = "ocr")]
    fn sparse_form_text() -> String {
        let text = r#"
[]  Yes
[]  No

Name: ___
"#;
        text.to_string()
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_numeric_table_with_short_tokens_no_ocr() {
        let text = numeric_table_text();
        let thresholds = t();

        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        assert!(
            stats.non_whitespace >= 300,
            "Test setup: numeric table should have 300+ non-whitespace chars, got {}",
            stats.non_whitespace
        );
        assert!(
            decision.avg_non_whitespace >= 100.0,
            "Test setup: numeric table should have avg_non_whitespace >= 100, got {:.2}",
            decision.avg_non_whitespace
        );

        // The fixture is still deliberately hostile-looking -- dense, and dominated by
        // one- and two-character tokens. What changed is that a bare number is no longer
        // counted as a fragmented *word*: there are no words here to fragment, so the
        // ratio abstains at 0.00 instead of reading >0.5. The property this test exists
        // for is unchanged and asserted below: a dense numeric table must not trigger
        // OCR fallback. ~keep
        let tokens: Vec<&str> = text.split_whitespace().collect();
        let short_tokens = tokens.iter().filter(|w| w.len() <= 2).count();
        assert!(
            short_tokens * 2 > tokens.len(),
            "Test setup: numeric table should be dominated by short tokens, got {}/{}",
            short_tokens,
            tokens.len()
        );
        assert_eq!(
            stats.fragmented_word_ratio, 0.0,
            "a numeric table carries no alphabetic words to fragment, so the ratio must abstain"
        );

        assert!(
            !decision.fallback,
            "Numeric table with substantial content should NOT trigger OCR fallback. \
             Stats: non_ws={}, avg_word_len={:.2}, frag_ratio={:.2}",
            stats.non_whitespace, stats.avg_word_length, stats.fragmented_word_ratio
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_formula_page_with_short_tokens_no_ocr() {
        let text = formula_text();
        let thresholds = t();

        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        assert!(
            stats.non_whitespace >= 500,
            "Test setup: formula text should have 500+ non-whitespace chars, got {}",
            stats.non_whitespace
        );

        let would_trigger_old_logic = stats.fragmented_word_ratio >= thresholds.max_fragmented_word_ratio
            && stats.meaningful_words < thresholds.min_meaningful_words;

        assert!(
            !decision.fallback,
            "Formula page with substantial content should NOT trigger OCR fallback. \
             Would trigger old logic: {}, frag={:.2}, meaningful={}",
            would_trigger_old_logic, stats.fragmented_word_ratio, stats.meaningful_words
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_sparse_form_triggers_ocr() {
        let text = sparse_form_text();
        let thresholds = t();

        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        eprintln!(
            "Sparse form stats: non_ws={}, avg_non_ws={:.2}, meaningful_words={}, fallback={}",
            stats.non_whitespace, decision.avg_non_whitespace, stats.meaningful_words, decision.fallback
        );

        assert!(
            stats.non_whitespace < 100,
            "Test setup: sparse form should have <100 non-whitespace chars, got {}",
            stats.non_whitespace
        );

        assert!(
            decision.fallback,
            "Sparse form (legitimately few chars) SHOULD trigger OCR fallback. Stats: non_ws={}, meaningful={}",
            stats.non_whitespace, stats.meaningful_words
        );
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn test_short_token_dense_content_no_ocr() {
        let mut text = String::new();
        for i in 0..20 {
            text.push_str(&format!("Row{} ", i));

            for j in 0..15 {
                let val = (i * 13 + j * 7) % 5000;
                text.push_str(&format!("{} ", val));
            }
            text.push('\n');
        }

        let thresholds = t();
        let stats = NativeTextStats::compute(&text, &thresholds);
        let decision = evaluate_native_text_for_ocr(&text, Some(1), &thresholds);

        assert!(
            decision.avg_non_whitespace >= 100.0,
            "Test setup: should have avg_non_whitespace >= 100, got {:.2}",
            decision.avg_non_whitespace
        );
        assert!(
            stats.fragmented_word_ratio < 0.80,
            "Test setup: should be sub-critical < 0.80, got {:.2}",
            stats.fragmented_word_ratio
        );

        assert!(
            !decision.fallback,
            "Dense numeric table should NOT trigger OCR fallback"
        );
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn ocr_layout_dimensions_use_valid_processed_image_metadata() {
        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(2000),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.into(),
            serde_json::json!(3000),
        );

        assert_eq!(resolved_ocr_layout_dimensions(&metadata, 1000, 1500), (2000, 3000));
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn ocr_layout_dimensions_fall_back_for_incomplete_or_invalid_metadata() {
        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(0),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.into(),
            serde_json::json!(3000),
        );

        assert_eq!(resolved_ocr_layout_dimensions(&metadata, 1000, 1500), (1000, 1500));

        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(2000),
        );
        metadata
            .additional
            .remove(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY);

        assert_eq!(resolved_ocr_layout_dimensions(&metadata, 1000, 1500), (1000, 1500));
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn detection_scaling_targets_ocr_coordinate_space() {
        let detection = crate::layout::DetectionResult {
            page_width: 1000,
            page_height: 1500,
            detections: vec![crate::layout::LayoutDetection {
                class_name: crate::layout::LayoutClass::SectionHeader,
                confidence: 0.9,
                bbox: crate::layout::BBox {
                    x1: 100.0,
                    y1: 200.0,
                    x2: 400.0,
                    y2: 300.0,
                },
            }],
        };

        let scaled = scale_detection_to_dimensions(&detection, 2000, 3000);

        assert_eq!(scaled.page_width, 2000);
        assert_eq!(scaled.page_height, 3000);
        assert_eq!(scaled.detections[0].bbox.x1, 200.0);
        assert_eq!(scaled.detections[0].bbox.y1, 400.0);
        assert_eq!(scaled.detections[0].bbox.x2, 800.0);
        assert_eq!(scaled.detections[0].bbox.y2, 600.0);
    }

    /// #665: the mixed OCR route (`--ocr-scanned-pages` / `force_ocr_pages` / the per-page
    /// fallback) never threaded layout detections down into per-page OCR assembly at all --
    /// `extract_with_ocr_for_page`'s `layout_detections` parameter was hardcoded to `None` on
    /// this route, so `--layout` alone produced byte-identical output with zero layout log
    /// lines. `detection_for_mixed_route_page` is the lookup that closes that gap: given the
    /// whole-document layout pass's per-page `Vec<DetectionResult>`, it returns exactly the
    /// entry for a page's own document-wide 0-based index, unmodified.
    ///
    /// Fails on unfixed code two ways: (1) `detection_for_mixed_route_page` does not exist on
    /// the mixed route at all before this fix, so this fails to compile; (2) if the fix's
    /// per-page alignment regresses to an off-by-one (e.g. reading `page_idx + 1` or
    /// `page_idx - 1`), the middle assertion below fails because it would return page 0's or
    /// page 2's very different `page_width`/`page_height`/bbox values instead of page 1's.
    #[cfg(all(
        feature = "layout-detection",
        any(feature = "ocr", feature = "ocr-pipeline"),
        feature = "pdf"
    ))]
    #[test]
    fn detection_for_mixed_route_page_selects_the_aligned_page_without_transforming_it() {
        let page0 = crate::layout::DetectionResult {
            page_width: 111,
            page_height: 222,
            detections: Vec::new(),
        };
        let page1 = crate::layout::DetectionResult {
            page_width: 333,
            page_height: 444,
            detections: vec![crate::layout::LayoutDetection {
                class_name: crate::layout::LayoutClass::SectionHeader,
                confidence: 0.95,
                bbox: crate::layout::BBox {
                    x1: 10.0,
                    y1: 20.0,
                    x2: 30.0,
                    y2: 40.0,
                },
            }],
        };
        let page2 = crate::layout::DetectionResult {
            page_width: 555,
            page_height: 666,
            detections: Vec::new(),
        };
        let detections = vec![page0, page1, page2];

        let found = detection_for_mixed_route_page(Some(&detections), 1).expect("page index 1 must have a detection");

        // Pinned to page 1's exact pixel-space numbers -- not page 0's or page 2's, and not a
        // rescaled derivative of them. Any coordinate transform belongs downstream, inside
        // `extract_with_ocr_for_page` (`scale_detection_to_dimensions` /
        // `scale_detection_to_ocr_coordinates`), not in this lookup.
        assert_eq!(found.page_width, 333);
        assert_eq!(found.page_height, 444);
        assert_eq!(found.detections.len(), 1);
        assert_eq!(found.detections[0].bbox.x1, 10.0);
        assert_eq!(found.detections[0].bbox.y1, 20.0);
        assert_eq!(found.detections[0].bbox.x2, 30.0);
        assert_eq!(found.detections[0].bbox.y2, 40.0);

        assert!(
            detection_for_mixed_route_page(Some(&detections), 5).is_none(),
            "an out-of-range page index must not silently return a neighboring page's detection"
        );
        assert!(
            detection_for_mixed_route_page(None, 1).is_none(),
            "no whole-document layout pass ran (e.g. layout not configured) must mean no detection for any page"
        );
    }

    /// #665: on the mixed OCR route, layout is only reachable when the single configured
    /// backend is wrapped in a one-stage pipeline and driven through `run_ocr_pipeline_for_page`
    /// -- the only per-page entry point that accepts `layout_detections` at all. Before this
    /// fix, `single_stage_pipeline_for_layout` did not exist, so this fails to compile on
    /// unfixed code. Pins the synthesized stage's fields (particularly `backend` and
    /// `language`, which the fix must copy from the real `OcrConfig` rather than defaulting)
    /// and the derived quality thresholds, since a wrong backend name here would silently
    /// route pages to the wrong OCR engine while still calling it "layout enabled".
    #[cfg(all(
        feature = "layout-detection",
        any(feature = "ocr", feature = "ocr-pipeline"),
        feature = "pdf"
    ))]
    #[test]
    fn single_stage_pipeline_for_layout_wraps_the_configured_backend() {
        let ocr_config = crate::core::config::OcrConfig {
            backend: "paddleocr".to_string(),
            language: vec!["deu".to_string()],
            ..Default::default()
        };

        let pipeline = single_stage_pipeline_for_layout(&ocr_config);

        assert_eq!(
            pipeline.stages.len(),
            1,
            "must wrap the single configured backend in exactly one stage"
        );
        assert_eq!(pipeline.stages[0].backend, "paddleocr");
        assert_eq!(pipeline.stages[0].priority, 100);
        assert_eq!(pipeline.stages[0].language, Some(vec!["deu".to_string()]));
        assert!(
            (pipeline.quality_thresholds.pipeline_min_quality - ocr_config.effective_thresholds().pipeline_min_quality)
                .abs()
                < f64::EPSILON,
            "must use the ocr config's own effective thresholds rather than an arbitrary default"
        );
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    fn rotated_ocr_metadata(final_width: u32, final_height: u32, orientation_degrees: i32) -> crate::types::Metadata {
        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY.into(),
            serde_json::json!(final_width),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY.into(),
            serde_json::json!(final_height),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY.into(),
            serde_json::json!(true),
        );
        metadata.additional.insert(
            crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY.into(),
            serde_json::json!(orientation_degrees),
        );
        metadata
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    fn rotation_test_detection() -> crate::layout::DetectionResult {
        crate::layout::DetectionResult {
            page_width: 100,
            page_height: 200,
            detections: vec![crate::layout::LayoutDetection {
                class_name: crate::layout::LayoutClass::SectionHeader,
                confidence: 0.9,
                bbox: crate::layout::BBox {
                    x1: 10.0,
                    y1: 20.0,
                    x2: 30.0,
                    y2: 60.0,
                },
            }],
        }
    }

    #[cfg(all(feature = "layout-detection", any(feature = "ocr", feature = "ocr-wasm")))]
    #[test]
    fn ocr_detection_rotation_matches_clockwise_90_pixel_transform() {
        let metadata = rotated_ocr_metadata(200, 100, 270);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (200, 100));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (140.0, 10.0, 180.0, 30.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn ocr_detection_rotation_matches_180_pixel_transform() {
        let metadata = rotated_ocr_metadata(100, 200, 180);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (100, 200));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (70.0, 140.0, 90.0, 180.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn ocr_detection_rotation_matches_clockwise_270_pixel_transform() {
        let metadata = rotated_ocr_metadata(200, 100, 90);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (200, 100));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (20.0, 70.0, 60.0, 90.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn invalid_rotation_metadata_preserves_dimension_only_fallback() {
        let metadata = rotated_ocr_metadata(200, 400, 45);
        let scaled = scale_detection_to_ocr_coordinates(&rotation_test_detection(), &metadata, 100, 200);
        let bbox = scaled.detections[0].bbox;

        assert_eq!((scaled.page_width, scaled.page_height), (200, 400));
        assert_eq!((bbox.x1, bbox.y1, bbox.x2, bbox.y2), (20.0, 40.0, 60.0, 120.0));
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn rotated_ocr_elements_transform_back_to_render_coordinates() {
        let cases = [
            (270, (140, 10, 40, 20)),
            (180, (70, 140, 20, 40)),
            (90, (20, 70, 40, 20)),
        ];

        for (orientation, (left, top, width, height)) in cases {
            let metadata = if orientation == 180 {
                rotated_ocr_metadata(100, 200, orientation)
            } else {
                rotated_ocr_metadata(200, 100, orientation)
            };
            let element = crate::types::OcrElement {
                text: "heading".to_string(),
                geometry: crate::types::OcrBoundingGeometry::Rectangle {
                    left,
                    top,
                    width,
                    height,
                },
                ..Default::default()
            };

            let transformed = transform_ocr_elements_to_render_space(&[element], &metadata, 100, 200);

            assert_eq!(
                transformed[0].geometry,
                crate::types::OcrBoundingGeometry::Rectangle {
                    left: 10,
                    top: 20,
                    width: 20,
                    height: 40,
                },
                "orientation {orientation}"
            );
        }
    }

    #[cfg(feature = "layout-detection")]
    #[test]
    fn invalid_ocr_element_metadata_preserves_original_geometry() {
        let metadata = rotated_ocr_metadata(200, 400, 45);
        let element = crate::types::OcrElement {
            text: "heading".to_string(),
            geometry: crate::types::OcrBoundingGeometry::Rectangle {
                left: 20,
                top: 40,
                width: 60,
                height: 80,
            },
            ..Default::default()
        };

        let transformed = transform_ocr_elements_to_render_space(std::slice::from_ref(&element), &metadata, 100, 200);

        assert_eq!(transformed[0].geometry, element.geometry);
    }

    // ---------------------------------------------------------------------
    // #57 / #59 / #60 — the mixed PDF OCR path must not drop what it rebuilds.
    // ---------------------------------------------------------------------

    /// Native two-page document: page 1 native prose, page 2 native prose.
    fn native_two_page_document() -> crate::types::internal::InternalDocument {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        doc.mime_type = "application/pdf".to_string();
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "native page one", 0).with_page(1));
        doc.push_element(InternalElement::text(ElementKind::PageBreak, "", 0));
        doc.push_element(InternalElement::text(ElementKind::Paragraph, "native page two", 0).with_page(2));
        doc
    }

    fn ocr_table(markdown: &str, page_number: u32) -> crate::types::Table {
        crate::types::Table {
            cells: vec![vec!["a".to_string(), "b".to_string()]],
            markdown: markdown.to_string(),
            page_number,
            bounding_box: None,
            ..Default::default()
        }
    }

    /// Structured OCR result for one page: a paragraph plus a table it references.
    fn structured_ocr_page_with_table(page: u32) -> crate::types::internal::InternalDocument {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        doc.push_element(
            InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                "ocr prose",
                0,
            )
            .with_page(page),
        );
        let table_index = doc.push_table(ocr_table("| a | b |", page));
        doc.push_element(InternalElement::text(ElementKind::Table { table_index }, "", 0).with_page(page));
        doc
    }

    /// #57 — a table recognised on an OCR-replaced page must survive the merge into
    /// the parent document, both as a `tables` entry and as a referencing element.
    #[test]
    fn should_keep_ocr_page_tables_when_page_is_replaced_by_ocr() {
        use crate::types::internal::ElementKind;

        let mut doc = native_two_page_document();
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, structured_ocr_page_with_table(2));

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        assert_eq!(doc.tables.len(), 1, "the OCR'd page's table must survive the merge");
        assert_eq!(doc.tables[0].markdown, "| a | b |");
        assert_eq!(doc.tables[0].page_number, 2);

        let table_indices: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|element| match element.kind {
                ElementKind::Table { table_index } => Some(table_index),
                _ => None,
            })
            .collect();
        assert_eq!(
            table_indices,
            vec![0],
            "exactly one table element, re-indexed into the parent's table collection"
        );
        assert!(
            doc.elements.iter().any(|element| element.text == "ocr prose"),
            "the structured OCR text must be used, not the raw-text fallback"
        );
        assert!(
            !doc.elements.iter().any(|element| element.text == "native page two"),
            "the replaced page's native prose must be gone"
        );
    }

    /// #59 — page assets are re-indexed against the parent's collections instead of
    /// falling back to splitting raw text, so the asset-to-page association survives.
    #[test]
    fn should_reindex_ocr_page_assets_against_parent_collections() {
        use crate::types::internal::ElementKind;

        let mut doc = native_two_page_document();
        // Parent already owns one table and one image; the OCR page's assets must be
        // appended after them, and their references rebased accordingly.
        doc.push_table(ocr_table("| pre-existing |", 1));
        doc.push_image(crate::types::ExtractedImage {
            image_index: 0,
            page_number: Some(1),
            ..Default::default()
        });

        let mut page_doc = structured_ocr_page_with_table(2);
        let image_index = page_doc.push_image(crate::types::ExtractedImage {
            image_index: 0,
            page_number: None,
            ..Default::default()
        });
        page_doc.push_element(
            crate::types::internal::InternalElement::text(ElementKind::Image { image_index }, "", 0).with_page(2),
        );

        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, page_doc);

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        assert_eq!(doc.tables.len(), 2);
        assert_eq!(doc.tables[0].markdown, "| pre-existing |");
        assert_eq!(doc.tables[1].markdown, "| a | b |");
        assert_eq!(doc.images.len(), 2);
        assert_eq!(doc.images[1].image_index, 1, "merged image must be re-indexed to 1");
        assert_eq!(
            doc.images[1].page_number,
            Some(2),
            "merged image must stay associated with its OCR page"
        );

        let merged_table_index = doc.elements.iter().find_map(|element| match element.kind {
            ElementKind::Table { table_index } => Some(table_index),
            _ => None,
        });
        let merged_image_index = doc.elements.iter().find_map(|element| match element.kind {
            ElementKind::Image { image_index } => Some(image_index),
            _ => None,
        });
        assert_eq!(
            merged_table_index,
            Some(1),
            "table reference rebased onto parent index 1"
        );
        assert_eq!(
            merged_image_index,
            Some(1),
            "image reference rebased onto parent index 1"
        );
    }

    /// #59 — a page document carrying a table that its own element list never
    /// references still contributes a reference, so the table is reachable.
    #[test]
    fn should_emit_reference_for_unreferenced_ocr_page_table() {
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut page_doc = InternalDocument::new("pdf");
        page_doc.push_element(
            InternalElement::text(
                ElementKind::OcrText {
                    level: OcrElementLevel::Block,
                },
                "ocr prose",
                0,
            )
            .with_page(2),
        );
        page_doc.push_table(ocr_table("| orphan |", 2));

        let mut doc = native_two_page_document();
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, page_doc);

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        assert_eq!(doc.tables.len(), 1);
        assert_eq!(doc.tables[0].markdown, "| orphan |");
        let table_indices: Vec<u32> = doc
            .elements
            .iter()
            .filter_map(|element| match element.kind {
                ElementKind::Table { table_index } => Some(table_index),
                _ => None,
            })
            .collect();
        assert_eq!(table_indices, vec![0]);
    }

    /// #60 — `prebuilt_ocr_elements` carried by an OCR page reach the parent document.
    #[test]
    fn should_carry_ocr_page_elements_into_parent_document() {
        let mut page_doc = structured_ocr_page_with_table(2);
        page_doc.prebuilt_ocr_elements = Some(vec![crate::types::OcrElement {
            text: "word".to_string(),
            page_number: 1,
            ..Default::default()
        }]);

        let mut doc = native_two_page_document();
        let mut ocr_results = ahash::AHashMap::new();
        ocr_results.insert(2u32, "ocr prose".to_string());
        let mut structured = ahash::AHashMap::new();
        structured.insert(2u32, page_doc);

        merge_structured_ocr_pages_into_internal_document(&mut doc, &ocr_results, &structured);

        let elements = doc.prebuilt_ocr_elements.expect("OCR elements must reach the parent");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "word");
        assert_eq!(elements[0].page_number, 2, "element must be renumbered onto its page");
    }

    /// #60 — the single-backend mixed route must carry the backend's tables and OCR
    /// elements onto the page document instead of discarding them.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_collect_backend_tables_and_elements_on_mixed_ocr_page() {
        let mut result = crate::types::ExtractedDocument {
            content: "scanned prose".to_string(),
            tables: vec![ocr_table("| x | y |", 0)],
            ocr_elements: Some(vec![crate::types::OcrElement {
                text: "word".to_string(),
                page_number: 1,
                ..Default::default()
            }]),
            ..Default::default()
        };
        let public_config = crate::core::config::OcrConfig {
            element_config: Some(crate::types::OcrElementConfig {
                include_elements: true,
                ..Default::default()
            }),
            ..Default::default()
        };

        let (page_doc, _paragraphs) = build_mixed_ocr_page_document(
            &mut result,
            &public_config,
            3,
            1000,
            1000,
            1000.0,
            1000.0,
            disabled_page_margins(),
        )
        .expect("a backend result with tables must produce a page document");

        assert_eq!(page_doc.tables.len(), 1, "backend table must be kept");
        assert_eq!(page_doc.tables[0].markdown, "| x | y |");
        assert_eq!(page_doc.tables[0].page_number, 3, "table renumbered onto its page");
        let elements = page_doc
            .prebuilt_ocr_elements
            .expect("backend OCR elements must be kept");
        assert_eq!(elements.len(), 1);
        assert_eq!(elements[0].text, "word");
        assert_eq!(elements[0].page_number, 3);
        assert!(
            result.tables.is_empty() && result.ocr_elements.is_none(),
            "payload is moved, not copied"
        );
    }

    /// #60 — a backend result with nothing structured keeps the previous behaviour:
    /// no page document, so the raw-text replacement path still applies.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_not_fabricate_page_document_when_backend_returns_only_text() {
        let mut result = crate::types::ExtractedDocument {
            content: "scanned prose".to_string(),
            ..Default::default()
        };

        assert!(
            build_mixed_ocr_page_document(
                &mut result,
                &crate::core::config::OcrConfig::default(),
                3,
                1000,
                1000,
                1000.0,
                1000.0,
                disabled_page_margins(),
            )
            .is_none()
        );
    }

    /// #633 — a backend that auto-rotated its input image (`OcrConfig::auto_rotate`)
    /// before OCR reports `ocr_internal_document` bboxes in that ROTATED image's pixel
    /// space, not the original raster the caller rendered and will rescale from
    /// (`render_width`/`render_height`). `undo_auto_rotate_document_bboxes` must map
    /// them back before `rescale_ocr_bboxes_to_page_points` runs, or the pixel->point
    /// scale divides the rotated-space bbox by the wrong (un-swapped) axis.
    ///
    /// This fails against the unfixed code (no call to
    /// `undo_auto_rotate_document_bboxes`): skipping straight to the rescale leaves
    /// the raw bbox {10, 20, 30, 60} scaled 1:1 and unchanged, not the
    /// rotation-corrected {140, 10, 180, 30} asserted below.
    ///
    /// (These expected numbers were previously {40, 10, 80, 30} — the output of
    /// `undo_auto_rotate_point`'s pre-fix 270-degree arm, `(processed_width - y, x)`,
    /// which used `processed_width` where the inverse rotation requires
    /// `processed_height`. Forward-mapping {140, 10, 180, 30} through the actual
    /// rotation PaddleOCR applies for `orientation.degrees == 90`
    /// (`image::imageops::rotate270`, i.e. 90° counter-clockwise) reproduces the
    /// input rectangle {10, 20}-{30, 60} exactly; forward-mapping the old {40, 10,
    /// 80, 30} does not, which is how the dimension-swap bug was confirmed.)
    #[cfg(feature = "pdf")]
    #[test]
    fn should_undo_auto_rotate_bboxes_before_rescaling_to_page_points() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        // The backend rotated a 200x100 raster 90 degrees before OCR, producing a
        // 100x200 processed image (`OCR_ORIENTATION_DEGREES_METADATA_KEY: 90`).
        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "word",
            0,
        );
        // Pixel-space bbox in the ROTATED 100x200 processed image.
        element.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 20.0,
            x1: 30.0,
            y1: 60.0,
        });
        doc.push_element(element);

        let mut metadata = crate::types::Metadata::default();
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_AUTO_ROTATED_METADATA_KEY),
            serde_json::Value::Bool(true),
        );
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_ORIENTATION_DEGREES_METADATA_KEY),
            serde_json::json!(90),
        );
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY),
            serde_json::json!(100),
        );
        metadata.additional.insert(
            std::borrow::Cow::Borrowed(crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY),
            serde_json::json!(200),
        );

        // Original (pre-rotation) 200x100 raster.
        undo_auto_rotate_document_bboxes(&mut doc, &metadata, 200, 100);

        // A 200x100pt page so the pixel->point rescale is 1:1 and does not obscure
        // the rotation fix.
        rescale_ocr_bboxes_to_page_points(Some(&mut doc), &mut [], 200, 100, 200.0, 100.0);

        let bbox = doc.elements[0].bbox.expect("element must keep its bbox");
        assert_eq!(bbox.x0, 140.0);
        assert_eq!(bbox.y0, 10.0);
        assert_eq!(bbox.x1, 180.0);
        assert_eq!(bbox.y1, 30.0);
    }

    /// #633 — a backend that never auto-rotated (the overwhelmingly common case,
    /// including every non-PaddleOCR backend and Paddle with `auto_rotate: false`)
    /// must be left completely unaffected: no `auto_rotated` metadata key at all.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_leave_bboxes_unchanged_when_backend_did_not_auto_rotate() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "word",
            0,
        );
        element.bbox = Some(BoundingBox {
            x0: 10.0,
            y0: 20.0,
            x1: 30.0,
            y1: 60.0,
        });
        doc.push_element(element);

        let metadata = crate::types::Metadata::default();
        undo_auto_rotate_document_bboxes(&mut doc, &metadata, 200, 100);

        let bbox = doc.elements[0].bbox.expect("element must keep its bbox");
        assert_eq!(bbox.x0, 10.0);
        assert_eq!(bbox.y0, 20.0);
        assert_eq!(bbox.x1, 30.0);
        assert_eq!(bbox.y1, 60.0);
    }

    /// #1423 — element bboxes are rescaled pixel->point (still top-left) so the later
    /// `pdf_block_bbox` flip (which now receives the page height in points) lands on
    /// exact PDF coordinates instead of raw Tesseract raster pixels.
    #[cfg(feature = "pdf")]
    #[test]
    fn rescale_ocr_bboxes_scales_element_bbox_without_flipping() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "hello",
            0,
        );
        // Pixel-space, top-left origin: y0 is the box's top row, y1 its bottom row.
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        });
        doc.push_element(element);

        // 1700x2200px raster of a 612x792pt (US Letter) page: scale_x = scale_y = 0.36.
        rescale_ocr_bboxes_to_page_points(Some(&mut doc), &mut [], 1700, 2200, 612.0, 792.0);

        let bbox = doc.elements[0].bbox.expect("bbox must survive rescale");
        assert_eq!(bbox.x0, 36.0);
        assert_eq!(bbox.y0, 72.0);
        assert_eq!(bbox.x1, 108.0);
        assert_eq!(bbox.y1, 144.0);
    }

    /// #1423 — table bboxes get the full pixel->point conversion *and* the top-left ->
    /// bottom-left flip here, since nothing downstream flips them (`push_table_element`
    /// copies `Table::bounding_box` through unchanged). The result must match the
    /// bottom-left/points contract documented on `Table::bounding_box`: a box near the
    /// top of the page ends up with a y1 (top) close to `page_height_pt`, not close to 0.
    #[cfg(feature = "pdf")]
    #[test]
    fn rescale_ocr_bboxes_scales_and_flips_table_bbox() {
        use crate::types::extraction::BoundingBox;

        let mut tables = [ocr_table("| a | b |", 0)];
        // `convert_ocr_table` stores the raw pixel rect as {x0: left, y0: top, x1:
        // right, y1: bottom} — top-left origin, unscaled pixels.
        tables[0].bounding_box = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        });

        rescale_ocr_bboxes_to_page_points(None, &mut tables, 1700, 2200, 612.0, 792.0);

        let bbox = tables[0].bounding_box.expect("bbox must survive rescale");
        assert_eq!(bbox.x0, 36.0, "left edge scales by scale_x");
        assert_eq!(bbox.x1, 108.0, "right edge scales by scale_x");
        assert_eq!(bbox.y0, 648.0, "bottom = page_height_pt - bottom_px * scale_y");
        assert_eq!(bbox.y1, 720.0, "top = page_height_pt - top_px * scale_y");
        assert!(bbox.y0 < bbox.y1, "bottom-left origin: y0 (bottom) must be < y1 (top)");
    }

    /// #1423 — zero raster dimensions (e.g. a synthetic document with no rendered page
    /// behind it) must leave bboxes untouched rather than dividing by zero or
    /// fabricating a scale factor.
    #[cfg(feature = "pdf")]
    #[test]
    fn rescale_ocr_bboxes_is_a_noop_when_image_dimensions_are_zero() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "hello",
            0,
        );
        let original = BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        };
        element.bbox = Some(original);
        doc.push_element(element);

        rescale_ocr_bboxes_to_page_points(Some(&mut doc), &mut [], 0, 0, 612.0, 792.0);

        assert_eq!(doc.elements[0].bbox, Some(original));
    }

    /// #1423 end-to-end: the single-backend mixed OCR route must hand
    /// `assemble_mixed_ocr_page_document` bboxes already in the page's point space, so
    /// the resulting element bbox matches what a digital (non-OCR) page would produce
    /// for the same physical position — PDF points, origin bottom-left — not raw
    /// Tesseract raster pixels.
    #[cfg(feature = "pdf")]
    #[test]
    fn build_mixed_ocr_page_document_rescales_element_bbox_into_page_points() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut ocr_doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "hello",
            0,
        );
        // A word sitting near the top-left corner of a 1700x2200px raster.
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 260.0,
        });
        ocr_doc.push_element(element);

        let mut result = crate::types::ExtractedDocument {
            content: "hello".to_string(),
            ocr_internal_document: Some(ocr_doc),
            ..Default::default()
        };

        // 1700x2200px raster of a 612x792pt (US Letter) page.
        let (page_doc, _paragraphs) = build_mixed_ocr_page_document(
            &mut result,
            &crate::core::config::OcrConfig::default(),
            1,
            1700,
            2200,
            612.0,
            792.0,
            disabled_page_margins(),
        )
        .expect("an OCR document with a text element must produce a page document");

        let hello_element = page_doc
            .elements
            .iter()
            .find(|element| element.text == "hello")
            .expect("the OCR paragraph must survive assembly");
        let bbox = hello_element.bbox.expect("assembled element must carry a bbox");
        // scale_x = scale_y = 0.36; top-left pixel (100, 200)-(300, 260) scales to
        // page height in points (792, not the 2200px raster height):
        //   bottom = 792 - 93.6 = 698.4, top = 792 - 72 = 720.0
        // Tolerance of 1e-3 accounts for the f32 arithmetic `pdf_block_bbox`
        // (`crate::pdf::structure::adapters`) performs on the flip, which this test
        // deliberately exercises end-to-end rather than re-deriving in f64.
        assert!((bbox.x0 - 36.0).abs() < 1e-3, "x0 = {}", bbox.x0);
        assert!((bbox.x1 - 108.0).abs() < 1e-3, "x1 = {}", bbox.x1);
        assert!((bbox.y0 - 698.4).abs() < 1e-3, "y0 (bottom) = {}", bbox.y0);
        assert!((bbox.y1 - 720.0).abs() < 1e-3, "y1 (top) = {}", bbox.y1);
        // GH#1423's defining symptom is a bbox that does not fit on the page at all,
        // so assert containment rather than a "near the top" heuristic. This is the
        // guard that actually bites: if the conversion regressed to emitting raster
        // pixels, y1 would be 2200 - 200 = 2000 and blow the 792pt bound, whereas a
        // "y1 is in the upper half" check would pass on that same broken output.
        assert!(
            bbox.x1 <= 612.0 && bbox.y1 <= 792.0,
            "every OCR bbox must fit within the 612x792pt page, got ({}, {})-({}, {})",
            bbox.x0,
            bbox.y0,
            bbox.x1,
            bbox.y1
        );
    }

    /// Unit-mismatch regression for `assemble_mixed_ocr_page_document`'s font-size
    /// resolution: `rescale_ocr_bboxes_to_page_points` (exercised by the previous test)
    /// rescales `element.bbox` into PDF points, but never touches `element.ocr_geometry`
    /// -- it stays raw OCR raster pixels, because `extraction::derive::OcrElement::geometry`
    /// documents that field as public raster-pixel-space API and rescaling it in place
    /// would corrupt that contract (and round away sub-pixel precision, since its point
    /// type is `(u32, u32)`). A `Quadrilateral`-geometry element -- sceptre and paddle's
    /// shape; Tesseract's `Rectangle` geometry never takes this branch -- must therefore
    /// have its quad-edge-height font-size proxy scaled by the page's *own*
    /// points-per-pixel ratio, not left in raw pixels.
    ///
    /// This fixture's raster/page pair (1700x2200px over 612x792pt, matching the previous
    /// test) gives a height-axis scale of 792 / 2200 = 0.36. The element's quad spans a
    /// 100px-tall band; the correctly-scaled font size is 100 * 0.36 = 36.0pt.
    ///
    /// Against unfixed code (`assemble_mixed_ocr_page_document` calling
    /// `ocr_doc_to_paragraphs(&doc, page_height, 1.0)` with one flat `1.0` scale for both
    /// the bbox and geometry fallback branches), this asserts `36.0` and fails: the actual
    /// value is `100.0`, the raw unscaled raster-pixel quad-edge height -- inflated by
    /// exactly the 1/0.36 ~= 2.78x this fixture's raster resolution implies (the ~2.08x
    /// figure quoted at 150 DPI is this same ratio at a different render resolution).
    #[cfg(feature = "pdf")]
    #[test]
    fn build_mixed_ocr_page_document_scales_quad_geometry_font_size_into_page_points() {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut ocr_doc = InternalDocument::new("pdf");
        let mut element = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            "SCEPTRE HEADING",
            0,
        );
        // Deliberately a different height from the quad below (150px vs 100px), so a
        // result of `150.0 * 0.36 = 54.0` (or unscaled `150.0`) would indicate the
        // bbox-height fallback fired instead of the quad-edge one -- it must not, since
        // `resolve_ocr_font_size_pt` always prefers `ocr_geometry` when present.
        element.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 500.0,
            y1: 350.0,
        });
        // A straight (unskewed) 100px-tall quad: `quad_edge_height_px` averages the two
        // side edges, both exactly 100px here, isolating this test from the
        // skew-robustness behavior `test_ocr_doc_uses_quad_edge_height_not_skewed_aabb_height_for_font_size`
        // (`pdf::structure::adapters`) already covers.
        element.ocr_geometry = Some(crate::types::OcrBoundingGeometry::Quadrilateral {
            points: [(100, 200), (500, 200), (500, 300), (100, 300)]
                .into_iter()
                .map(Into::into)
                .collect(),
        });
        ocr_doc.push_element(element);

        let mut result = crate::types::ExtractedDocument {
            content: "SCEPTRE HEADING".to_string(),
            ocr_internal_document: Some(ocr_doc),
            ..Default::default()
        };

        let (_page_doc, paragraphs) = build_mixed_ocr_page_document(
            &mut result,
            &crate::core::config::OcrConfig::default(),
            1,
            1700,
            2200,
            612.0,
            792.0,
            disabled_page_margins(),
        )
        .expect("an OCR document with a quad-geometry text element must produce a page document");

        assert_eq!(paragraphs.len(), 1);
        assert!(
            (paragraphs[0].dominant_font_size - 36.0).abs() < 1e-3,
            "expected the quad-edge height rescaled into PDF points (100px * 0.36 = 36.0pt), got {}",
            paragraphs[0].dominant_font_size
        );
    }

    /// An already-assembled pipeline page document: one paragraph whose bbox is in the
    /// raster's *pixel* space with a bottom-left origin (`ocr_doc_to_paragraphs` has
    /// already flipped it using the raster height), plus one table carrying the raw
    /// top-left pixel rect that `push_table_element` copies onto its element.
    ///
    /// Raster is 1700x2200px; the paragraph sits 200-260px below the raster's top edge.
    #[cfg(feature = "pdf")]
    fn assembled_pipeline_page_document() -> crate::types::internal::InternalDocument {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalDocument, InternalElement};

        let mut doc = InternalDocument::new("pdf");
        let mut paragraph = InternalElement::text(ElementKind::Paragraph, "hello", 0);
        paragraph.bbox = Some(BoundingBox {
            x0: 100.0,
            y0: 1940.0,
            x1: 300.0,
            y1: 2000.0,
        });
        doc.push_element(paragraph);

        let table_bbox = BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        };
        let mut table = ocr_table("| a | b |", 1);
        table.bounding_box = Some(table_bbox);
        let table_index = doc.push_table(table);
        let mut table_element = InternalElement::text(ElementKind::Table { table_index }, "", 0);
        table_element.bbox = Some(table_bbox);
        doc.push_element(table_element);

        doc
    }

    /// #529 (extends #1423) — the `vlm_fallback` / explicit-`pipeline` route builds its
    /// page document without going through `build_mixed_ocr_page_document`, so it never
    /// received the pixel -> point conversion at all and emitted raw raster pixels. The
    /// paragraph bbox is scaled only (it is already bottom-left), the table bbox and its
    /// element get the full scale-and-flip, and every box must fit on the page.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_emit_page_point_bboxes_when_ocr_runs_via_vlm_fallback_pipeline() {
        const PAGE_WIDTH_PT: f64 = 612.0;
        const PAGE_HEIGHT_PT: f64 = 792.0;

        // 1700x2200px raster of a 612x792pt (US Letter) page: scale_x = scale_y = 0.36.
        let page_doc = build_pipeline_ocr_page_document(
            Some(assembled_pipeline_page_document()),
            Vec::new(),
            Vec::new(),
            "hello",
            4,
            (1700, 2200),
            (PAGE_WIDTH_PT as f32, PAGE_HEIGHT_PT as f32),
        )
        .expect("a pipeline document must produce a page document");

        let paragraph = page_doc
            .elements
            .iter()
            .find(|element| element.text == "hello")
            .expect("the pipeline paragraph must survive");
        let bbox = paragraph.bbox.expect("paragraph must keep its bbox");
        assert_eq!(bbox.x0, 36.0);
        assert_eq!(bbox.y0, 698.4);
        assert_eq!(bbox.x1, 108.0);
        assert_eq!(bbox.y1, 720.0);
        // The defining symptom of #1423 is a box that cannot fit on the page: unconverted,
        // this paragraph reports y1 = 2000 against a 792pt page.
        assert!(
            bbox.x1 <= PAGE_WIDTH_PT && bbox.y1 <= PAGE_HEIGHT_PT,
            "paragraph bbox must fit within the page, got ({}, {})-({}, {})",
            bbox.x0,
            bbox.y0,
            bbox.x1,
            bbox.y1
        );

        let table_bbox = page_doc.tables[0]
            .bounding_box
            .expect("the table must keep its bounding box");
        assert_eq!(table_bbox.x0, 36.0);
        assert_eq!(table_bbox.y0, 648.0, "bottom = 792 - 400 * 0.36");
        assert_eq!(table_bbox.x1, 108.0);
        assert_eq!(table_bbox.y1, 720.0, "top = 792 - 200 * 0.36");
        assert!(
            table_bbox.x1 <= PAGE_WIDTH_PT && table_bbox.y1 <= PAGE_HEIGHT_PT,
            "table bbox must fit within the page, got ({}, {})-({}, {})",
            table_bbox.x0,
            table_bbox.y0,
            table_bbox.x1,
            table_bbox.y1
        );

        let table_element_bbox = page_doc
            .elements
            .iter()
            .find(|element| matches!(element.kind, crate::types::internal::ElementKind::Table { .. }))
            .and_then(|element| element.bbox)
            .expect("the table element must keep its bbox");
        assert_eq!(
            (
                table_element_bbox.x0,
                table_element_bbox.y0,
                table_element_bbox.x1,
                table_element_bbox.y1
            ),
            (36.0, 648.0, 108.0, 720.0),
            "the table element must report the same bottom-left point rect as its table"
        );
    }

    /// #529 — a pipeline stage that produced only a table (no structured document) must
    /// still get a page document whose table bbox is in page points, and a stage that
    /// produced nothing structured must still produce no document at all.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_convert_pipeline_table_bboxes_when_no_structured_document_is_returned() {
        use crate::types::extraction::BoundingBox;

        let mut table = ocr_table("| a | b |", 1);
        table.bounding_box = Some(BoundingBox {
            x0: 100.0,
            y0: 200.0,
            x1: 300.0,
            y1: 400.0,
        });

        let page_doc = build_pipeline_ocr_page_document(
            None,
            vec![table],
            Vec::new(),
            "scanned prose",
            2,
            (1700, 2200),
            (612.0, 792.0),
        )
        .expect("a pipeline result with a table must produce a page document");

        assert_eq!(page_doc.tables.len(), 1);
        assert_eq!(page_doc.tables[0].page_number, 2, "table renumbered onto its page");
        let bbox = page_doc.tables[0].bounding_box.expect("table bbox must survive");
        assert_eq!((bbox.x0, bbox.y0, bbox.x1, bbox.y1), (36.0, 648.0, 108.0, 720.0));

        assert!(
            build_pipeline_ocr_page_document(
                None,
                Vec::new(),
                Vec::new(),
                "text only",
                2,
                (1700, 2200),
                (612.0, 792.0)
            )
            .is_none(),
            "a text-only pipeline result must keep the raw-text replacement path"
        );
    }

    /// A single-page PDF with a landscape 200x100pt MediaBox and the given `/Rotate`.
    ///
    /// Mirrors the fixture builder in `layout_runner`'s tests, which is where the
    /// rotation convention exercised below is established.
    #[cfg(feature = "pdf")]
    fn rotated_landscape_pdf(rotation: i64) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page_id = document.new_object_id();
        let content_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));

        let mut page = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 100.into()],
            "Resources" => dictionary! {},
            "Contents" => content_id,
        };
        page.set("Rotate", rotation);
        document.objects.insert(page_id, Object::Dictionary(page));

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page_id.into()],
                "Count" => 1,
            }),
        );

        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("fixture PDF must serialize");
        bytes
    }

    /// #530 — on a page with `/Rotate` 90 or 270 the OCR bbox conversion must still land
    /// inside the page. `xberg_native_pdf` renders such a page in *displayed* orientation (with
    /// width and height swapped relative to the MediaBox), but every OCR route rasterizes
    /// through `normalize_rendered_page_for_ocr`, which applies the inverse quarter turn
    /// and hands OCR a raster back in raw MediaBox orientation — the same convention
    /// `layout_runner::render_layout_chunk` encodes by using the raw MediaBox dimensions
    /// whenever `normalize_for_ocr` is set. `page_dimensions_pt` also returns the raw
    /// MediaBox, so the two agree and no axis swap belongs in the conversion.
    ///
    /// This test pins that agreement: it fails if the raster stops being MediaBox-oriented
    /// or if a swap is introduced into the conversion, either of which puts rotated-page
    /// boxes off the page.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_convert_ocr_bboxes_within_page_bounds_on_rotated_pages() {
        for rotation in [90, 270] {
            let bytes = rotated_landscape_pdf(rotation);
            let rendered = render_selected_pages_for_ocr(&bytes, &[0]).expect("rotated page must render for OCR");
            let (_, image) = rendered.first().expect("page 0 must be rendered");
            let (raster_width_px, raster_height_px) = (image.width(), image.height());

            let document = xberg_native_pdf::PdfDocument::from_bytes(bytes.clone()).expect("fixture PDF must open");
            let (page_width_pt, page_height_pt) = page_dimensions_pt(&document, 0);
            assert_eq!(
                (page_width_pt, page_height_pt),
                (200.0, 100.0),
                "/Rotate {rotation}: page_dimensions_pt reports the raw MediaBox"
            );
            assert!(
                raster_width_px > raster_height_px,
                "/Rotate {rotation}: the OCR raster must keep the MediaBox's landscape \
                 orientation, got {raster_width_px}x{raster_height_px}"
            );

            // A table box covering the whole raster must convert to exactly the whole page.
            let mut tables = [ocr_table("| a | b |", 1)];
            tables[0].bounding_box = Some(crate::types::extraction::BoundingBox {
                x0: 0.0,
                y0: 0.0,
                x1: f64::from(raster_width_px),
                y1: f64::from(raster_height_px),
            });
            rescale_ocr_bboxes_to_page_points(
                None,
                &mut tables,
                raster_width_px,
                raster_height_px,
                page_width_pt,
                page_height_pt,
            );

            let bbox = tables[0].bounding_box.expect("table bbox must survive rescale");
            assert_eq!(
                (bbox.x0, bbox.y0, bbox.x1, bbox.y1),
                (0.0, 0.0, 200.0, 100.0),
                "/Rotate {rotation}: a full-raster box must map onto the full page"
            );
            assert!(
                bbox.x1 <= f64::from(page_width_pt) && bbox.y1 <= f64::from(page_height_pt),
                "/Rotate {rotation}: converted bbox must fit within the page"
            );
        }
    }

    /// The PDF OCR route is the only caller that can know a raster's true resolution, and it
    /// must say so: without the hint the OCR preprocessor assumes 72 DPI for a page rendered at
    /// 150 and resizes by more than twice the correct factor.
    ///
    /// Fails on unfixed code: `ocr_config_with_page_rotation_hint` takes two arguments there, so
    /// this does not compile. Dropping the third argument makes it compile and then fail on the
    /// first assertion — an unrotated page is a documented no-op, so `backend_options` stays
    /// `None` and the lookup yields `None` rather than `Some(150.0)`.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_stamp_source_dpi_hint_even_on_an_unrotated_page() {
        const RENDER_DPI: f64 = 150.0;
        let config = crate::core::config::ocr::OcrConfig::default();

        let hinted = ocr_config_with_page_rotation_hint(&config, 0, Some(RENDER_DPI));

        let options = hinted
            .backend_options
            .as_ref()
            .expect("a known source DPI must produce backend_options");
        assert_eq!(
            options
                .get(crate::core::config::ocr::SOURCE_DPI_BACKEND_OPTION)
                .and_then(serde_json::Value::as_f64),
            Some(RENDER_DPI)
        );
        assert!(
            options.get("page_rotation_degrees").is_none(),
            "an unrotated page must not gain a rotation hint it does not need"
        );
    }

    /// Both hints are independent and both survive on a page that has each.
    ///
    /// Fails on unfixed code by not compiling (two-argument signature); with the third argument
    /// dropped the `source_dpi` assertion fails with `left: None, right: Some(96.0)`.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_stamp_both_page_hints_when_a_rotated_page_has_a_known_dpi() {
        const REDUCED_RENDER_DPI: f64 = 96.0;
        let config = crate::core::config::ocr::OcrConfig::default();

        let hinted = ocr_config_with_page_rotation_hint(&config, 270, Some(REDUCED_RENDER_DPI));

        let options = hinted.backend_options.as_ref().expect("both hints must be carried");
        assert_eq!(
            options.get("page_rotation_degrees").and_then(serde_json::Value::as_u64),
            Some(270)
        );
        assert_eq!(
            options
                .get(crate::core::config::ocr::SOURCE_DPI_BACKEND_OPTION)
                .and_then(serde_json::Value::as_f64),
            Some(REDUCED_RENDER_DPI)
        );
    }

    /// With nothing to say the helper still borrows rather than clones, so pages that carry
    /// neither hint keep paying nothing.
    ///
    /// Fails on unfixed code by not compiling; the borrow behaviour itself is unchanged, which
    /// is what this pins.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_borrow_config_when_no_page_hint_applies() {
        let config = crate::core::config::ocr::OcrConfig::default();

        let hinted = ocr_config_with_page_rotation_hint(&config, 0, None);

        assert!(matches!(hinted, Cow::Borrowed(_)), "no hints must mean no config clone");
    }

    /// The derivation itself, at the call site's own boundary: a Letter page rendered at the 150
    /// DPI the route asks for is 1275px wide, and that is what must reach the hint.
    ///
    /// Fails on unfixed code: `rendered_page_source_dpi` does not exist, so this does not
    /// compile — there was no per-page DPI anywhere on this route to assert against.
    #[cfg(feature = "pdf")]
    #[test]
    fn should_derive_source_dpi_from_the_rendered_letter_page() {
        let content = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let doc = xberg_native_pdf::PdfDocument::from_bytes(content).expect("fixture must open");

        assert_eq!(rendered_page_source_dpi(&doc, 0, 1275), Some(150.0));
    }

    /// Closes the gap left by 972d2269f7: that commit threaded a `page_rotation_degrees`
    /// hint into the full-page and per-page/mixed OCR routes via
    /// `ocr_config_with_page_rotation_hint`, but `extract_with_ocr`'s images-driven branch
    /// (the one the layout-detection route feeds pre-rendered pages through, see
    /// `crates/xberg/src/extractors/pdf/mod.rs`) never computed a rotation at all: its only
    /// rotation source, `lazy_pdf_render_state`, is only populated when `images.is_none()`
    /// (see the `if !use_document_processing && images.is_none()` guard above), so every
    /// lookup fell through to `unwrap_or(0)`.
    ///
    /// Fails on unfixed code: `ocr_config_with_page_rotation_hint` is documented as a no-op
    /// for a rotation of 0, so an unfixed call leaves `OcrConfig.backend_options` at `None`
    /// instead of carrying `{"page_rotation_degrees": 270}` for a `/Rotate 270` page.
    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn should_thread_page_rotation_hint_through_the_images_driven_ocr_route() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Mutex;

        struct RotationCapturingBackend {
            captured_backend_options: Mutex<Vec<Option<serde_json::Value>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for RotationCapturingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], config: &OcrConfig) -> crate::Result<ExtractedDocument> {
                self.captured_backend_options
                    .lock()
                    .unwrap()
                    .push(config.backend_options.clone());
                Ok(ExtractedDocument {
                    content: "text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for RotationCapturingBackend {
            fn name(&self) -> &str {
                "rotation-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = std::sync::Arc::new(RotationCapturingBackend {
            captured_backend_options: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        // Same fixture convention as `should_convert_ocr_bboxes_within_page_bounds_on_rotated_pages`:
        // a landscape MediaBox with `/Rotate 270`, mirroring the ordinance-scan case this
        // session's fix (972d2269f7) targeted.
        let content = rotated_landscape_pdf(270);
        let rendered = render_selected_pages_for_ocr(&content, &[0]).expect("rotated fixture page must render for OCR");
        let images: Vec<image::DynamicImage> = rendered.into_iter().map(|(_, image)| image).collect();

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "rotation-capturing-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        // `content` is passed alongside `images`, mirroring the real layout-detection call
        // site (`crates/xberg/src/extractors/pdf/mod.rs`), which always has the original PDF
        // bytes available even when it hands in pre-rendered images.
        let result = extract_with_ocr(
            Some(&content),
            Some(&images),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("rotation-capturing-mock").unwrap();

        assert!(result.is_ok(), "extract_with_ocr should succeed: {:?}", result.err());

        let captured = backend.captured_backend_options.lock().unwrap();
        assert_eq!(captured.len(), 1, "backend should have been called exactly once");
        let rotation_hint = captured[0]
            .as_ref()
            .and_then(|opts| opts.get("page_rotation_degrees"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            rotation_hint,
            Some(270),
            "the images-driven OCR route (used by layout detection) must carry the page's \
             /Rotate value into backend_options, exactly as the full-page and per-page routes \
             already do (972d2269f7); got backend_options = {:?}",
            captured[0]
        );
    }

    /// #643 — a backend that declares `PageOrientationHandling::RequiresUpright` must receive
    /// an upright raster on a rotated page, not the same MediaBox-oriented (sideways) raster
    /// every other backend gets. `normalize_rendered_page_for_ocr` deliberately hands every
    /// backend that raw-orientation raster (#530); Tesseract self-corrects it and PaddleOCR
    /// recognises the rotated text correctly (only block order needs fixing), but a
    /// `RequiresUpright` backend (sceptre in production) emits character garbage on it instead
    /// — confirmed this session by feeding the same page through an upright render.
    ///
    /// Fails on unfixed code: without `upright_raster_for_backend` wired into the OCR call, the
    /// backend receives the same landscape MediaBox raster as every other backend (width >
    /// height, per the #530 invariant on this fixture) instead of the portrait upright raster
    /// asserted below.
    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn should_send_an_upright_raster_to_a_requires_upright_backend_on_a_rotated_page() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, PageOrientationHandling, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Mutex;

        struct RequiresUprightCapturingBackend {
            captured_dimensions: Mutex<Vec<(u32, u32)>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for RequiresUprightCapturingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            fn page_orientation_handling(&self) -> PageOrientationHandling {
                PageOrientationHandling::RequiresUpright
            }
            async fn process_image(&self, image_bytes: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let decoded = image::load_from_memory(image_bytes).expect("captured raster bytes must decode");
                self.captured_dimensions
                    .lock()
                    .unwrap()
                    .push((decoded.width(), decoded.height()));
                Ok(ExtractedDocument {
                    content: "text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for RequiresUprightCapturingBackend {
            fn name(&self) -> &str {
                "requires-upright-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = std::sync::Arc::new(RequiresUprightCapturingBackend {
            captured_dimensions: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        // Same fixture convention as the two tests above: a landscape MediaBox with
        // `/Rotate 270`. The raw MediaBox raster (what every backend got before this fix) keeps
        // that landscape shape (width > height, #530); the upright raster this backend needs is
        // the display orientation instead, portrait (width < height).
        let content = rotated_landscape_pdf(270);

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "requires-upright-capturing-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&content),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("requires-upright-capturing-mock").unwrap();

        assert!(result.is_ok(), "extract_with_ocr should succeed: {:?}", result.err());

        let captured = backend.captured_dimensions.lock().unwrap();
        assert_eq!(captured.len(), 1, "backend should have been called exactly once");
        let (width, height) = captured[0];
        assert!(
            width < height,
            "a RequiresUpright backend must receive an upright (portrait) raster on a \
             /Rotate 270 page, not the raw landscape MediaBox raster every other backend gets; \
             got {width}x{height}"
        );
    }

    /// #643 pin — a backend that does NOT declare `RequiresUpright` (e.g. Tesseract's
    /// `SelfCorrecting`) must keep receiving exactly the raster this route has always sent it:
    /// the raw MediaBox-oriented raster `render_full_pdf_ocr_batch`/
    /// `normalize_rendered_page_for_ocr` produce (#530), byte-for-byte. Tesseract's output is
    /// the control for every OCR-rotation measurement this session; a future change that starts
    /// rotating its input for the wrong backend would invalidate that measurement silently, and
    /// this test exists so that cannot happen without failing here first.
    #[cfg(feature = "pdf")]
    #[tokio::test]
    async fn should_leave_a_self_correcting_backends_raster_byte_for_byte_unchanged() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, PageOrientationHandling, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Mutex;

        struct SelfCorrectingCapturingBackend {
            captured_bytes: Mutex<Vec<Vec<u8>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for SelfCorrectingCapturingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            fn page_orientation_handling(&self) -> PageOrientationHandling {
                PageOrientationHandling::SelfCorrecting
            }
            async fn process_image(&self, image_bytes: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                self.captured_bytes.lock().unwrap().push(image_bytes.to_vec());
                Ok(ExtractedDocument {
                    content: "text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for SelfCorrectingCapturingBackend {
            fn name(&self) -> &str {
                "self-correcting-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = std::sync::Arc::new(SelfCorrectingCapturingBackend {
            captured_bytes: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        let content = rotated_landscape_pdf(270);

        // Independently reproduce the exact raster this route sends every backend, via the
        // same private helpers `extract_with_ocr` itself calls, so the comparison below is
        // byte-for-byte against production behaviour rather than a re-derived approximation.
        let (expected_doc, _page_count, expected_rotations) =
            open_pdf_for_full_ocr(&content).expect("fixture PDF must open for OCR rendering");
        let expected_encoded = render_full_pdf_ocr_batch(
            &expected_doc,
            &expected_rotations,
            0..1,
            &crate::extractors::security::SecurityLimits::default(),
        )
        .expect("fixture page must render");
        let (_, expected_bytes, _, _) = expected_encoded.into_iter().next().expect("page 0 must render");

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "self-correcting-capturing-mock".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&content),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend("self-correcting-capturing-mock").unwrap();

        assert!(result.is_ok(), "extract_with_ocr should succeed: {:?}", result.err());

        let captured = backend.captured_bytes.lock().unwrap();
        assert_eq!(captured.len(), 1, "backend should have been called exactly once");
        assert_eq!(
            captured[0], *expected_bytes,
            "a SelfCorrecting backend's raster must stay byte-for-byte identical to the raw \
             MediaBox raster this route has always sent it; this pins that a future orientation \
             fix cannot silently start rotating it"
        );
    }

    /// Two-page PDF: page 1 an ordinary unrotated portrait page, page 2 a landscape MediaBox
    /// with the given `/Rotate`. Lets a test target `ocr_page_numbers = &[2]` and prove the
    /// rotation applied is page 2's own value, not page 0's/page 1's -- `rotated_landscape_pdf`
    /// above only builds a single page, which cannot distinguish "the correct page's rotation"
    /// from "whatever happens to be at index 0".
    #[cfg(feature = "pdf")]
    fn two_page_pdf_second_page_rotated(rotation: i64) -> Vec<u8> {
        use lopdf::{Document, Object, Stream, dictionary};

        let mut document = Document::with_version("1.5");
        let pages_id = document.new_object_id();
        let page1_id = document.new_object_id();
        let page2_id = document.new_object_id();
        let content1_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));
        let content2_id = document.add_object(Stream::new(dictionary! {}, Vec::new()));

        let page1 = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 100.into(), 200.into()],
            "Resources" => dictionary! {},
            "Contents" => content1_id,
        };
        document.objects.insert(page1_id, Object::Dictionary(page1));

        let mut page2 = dictionary! {
            "Type" => "Page",
            "Parent" => pages_id,
            "MediaBox" => vec![0.into(), 0.into(), 200.into(), 100.into()],
            "Resources" => dictionary! {},
            "Contents" => content2_id,
        };
        page2.set("Rotate", rotation);
        document.objects.insert(page2_id, Object::Dictionary(page2));

        document.objects.insert(
            pages_id,
            Object::Dictionary(dictionary! {
                "Type" => "Pages",
                "Kids" => vec![page1_id.into(), page2_id.into()],
                "Count" => 2,
            }),
        );

        let catalog_id = document.add_object(dictionary! {
            "Type" => "Catalog",
            "Pages" => pages_id,
        });
        document.trailer.set("Root", catalog_id);

        let mut bytes = Vec::new();
        document.save_to(&mut bytes).expect("fixture PDF must serialize");
        bytes
    }

    /// #651 — closes the gap in `extract_mixed_ocr_native`'s multi-stage pipeline route: its
    /// two `run_ocr_pipeline` calls passed `content: None` and, before this fix, had no
    /// parameter at all carrying the page's own `/Rotate` value, so neither the
    /// `ocr_config_with_page_rotation_hint` block-order hint nor the
    /// `upright_raster_for_backend` upright-raster correction ever reached a stage backend on
    /// this route — unlike the sibling single-backend route (972d2269f7 / #643), which already
    /// applies both.
    ///
    /// Targets page 2 of a 2-page document (not page 1) so the test cannot pass by accident:
    /// a fix that always resolved page 0's/page 1's rotation instead of the actually-OCR'd
    /// page's (e.g. from a naive index-based `content` lookup misaligned to a single detached
    /// image, which is exactly what a `content: Some(content)` fix without the explicit
    /// per-page override would have done) would still leave this backend on the wrong raster.
    ///
    /// Fails on unfixed code: without the rotation threaded from `extract_mixed_ocr_native`
    /// through `run_ocr_pipeline` to the stage's OCR call, the `RequiresUpright` backend
    /// receives the raw landscape MediaBox raster (width > height, per the #530 invariant) and
    /// no `page_rotation_degrees` in `backend_options`, so both assertions below fail.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_pipeline_route_threads_page_rotation_to_a_requires_upright_backend() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, PageOrientationHandling, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::{Arc, Mutex};

        struct RequiresUprightCapturingPipelineBackend {
            captured_dimensions: Mutex<Vec<(u32, u32)>>,
            captured_backend_options: Mutex<Vec<Option<serde_json::Value>>>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for RequiresUprightCapturingPipelineBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            fn page_orientation_handling(&self) -> PageOrientationHandling {
                PageOrientationHandling::RequiresUpright
            }
            async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let decoded = image::load_from_memory(image_bytes).expect("captured raster bytes must decode");
                self.captured_dimensions
                    .lock()
                    .unwrap()
                    .push((decoded.width(), decoded.height()));
                self.captured_backend_options
                    .lock()
                    .unwrap()
                    .push(config.backend_options.clone());
                Ok(ExtractedDocument {
                    content: "page two ocr text".to_string(),
                    ..Default::default()
                })
            }
        }

        impl Plugin for RequiresUprightCapturingPipelineBackend {
            fn name(&self) -> &str {
                "pipeline-requires-upright-capturing-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let backend = Arc::new(RequiresUprightCapturingPipelineBackend {
            captured_dimensions: Mutex::new(Vec::new()),
            captured_backend_options: Mutex::new(Vec::new()),
        });
        crate::plugins::register_ocr_backend(backend.clone()).unwrap();

        // Page 2 is landscape (200x100) with `/Rotate 270`; page 1 is an ordinary portrait
        // page and is never OCR'd (`ocr_page_numbers = &[2]` below), so a fix that resolved
        // page 1's (nonexistent) rotation instead of page 2's own would not accidentally
        // satisfy this test.
        let pdf = two_page_pdf_second_page_rotated(270);

        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: "pipeline-requires-upright-capturing-mock".to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            // Accept the first (only) stage unconditionally: this test exercises rotation
            // threading, not the quality-based selection policy.
            quality_thresholds: OcrQualityThresholds {
                pipeline_min_quality: 0.0,
                ..Default::default()
            },
        };

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(pipeline),
                ..Default::default()
            }),
            ..Default::default()
        };

        // `accepted_ocr_page_replacements` (which the returned map at index 1 reflects) only
        // accepts an OCR'd page whose number has a matching, non-overlapping native-text
        // boundary, so both pages need real boundaries even though only page 2 is OCR'd.
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[2], &pdf, &config, None)
            .await
            .expect("extract_mixed_ocr_native must succeed");

        crate::plugins::unregister_ocr_backend("pipeline-requires-upright-capturing-mock").unwrap();

        assert!(
            result.1.contains_key(&2),
            "page 2's OCR text must have been accepted as a replacement: {:?}",
            result.1
        );

        let captured_dims = backend.captured_dimensions.lock().unwrap();
        assert_eq!(captured_dims.len(), 1, "backend should have been called exactly once");
        let (width, height) = captured_dims[0];
        assert!(
            width < height,
            "a RequiresUpright backend driven through the multi-stage pipeline route must \
             receive an upright (portrait) raster for a /Rotate 270 page, not the raw \
             landscape MediaBox raster every backend used to get; got {width}x{height}"
        );

        let captured_options = backend.captured_backend_options.lock().unwrap();
        let rotation_hint = captured_options[0]
            .as_ref()
            .and_then(|opts| opts.get("page_rotation_degrees"))
            .and_then(|v| v.as_u64());
        assert_eq!(
            rotation_hint,
            Some(270),
            "the multi-stage pipeline route must also carry the page's /Rotate value into \
             backend_options, exactly as the single-backend route already does (972d2269f7); \
             got backend_options = {:?}",
            captured_options[0]
        );
    }

    /// Build a single-line OCR "block" element carrying an hOCR `x_fsize` (points)
    /// attribute, mirroring what `ocr::hocr_parser` attaches for tesseract output.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    fn ocr_font_block(text: &str, font_size_pt: &str, y0: f64, y1: f64) -> crate::types::internal::InternalElement {
        use crate::types::extraction::BoundingBox;
        use crate::types::internal::{ElementKind, InternalElement};
        use crate::types::ocr_elements::OcrElementLevel;

        let mut elem = InternalElement::text(
            ElementKind::OcrText {
                level: OcrElementLevel::Block,
            },
            text,
            0,
        );
        elem.bbox = Some(BoundingBox {
            x0: 10.0,
            y0,
            x1: 500.0,
            y1,
        });
        elem.attributes = Some(
            [("x_fsize".to_string(), font_size_pt.to_string())]
                .into_iter()
                .collect(),
        );
        elem
    }

    /// Two-page `Vec<Vec<PdfParagraph>>` with one large-font heading block, two
    /// ordered-list-marker blocks, and enough body prose (all at the same font size)
    /// to clear `MIN_BLOCKS_FOR_FONT_HEADING` and establish a reliable body-font
    /// baseline for the k-means heading heuristic.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    fn heading_and_list_ocr_pages() -> Vec<Vec<crate::pdf::structure::types::PdfParagraph>> {
        let mut page1 = crate::types::internal::InternalDocument::new("test");
        page1.push_element(ocr_font_block("ANNUAL REPORT OVERVIEW", "28", 10.0, 50.0));
        page1.push_element(ocr_font_block(
            "This document summarizes the annual results for the reporting period in detail.",
            "11",
            60.0,
            90.0,
        ));
        page1.push_element(ocr_font_block(
            "1. First item in the numbered list of findings",
            "11",
            100.0,
            130.0,
        ));
        page1.push_element(ocr_font_block(
            "2. Second item continuing the numbered list of findings",
            "11",
            140.0,
            170.0,
        ));
        page1.push_element(ocr_font_block(
            "Additional narrative text follows describing the broader context for readers.",
            "11",
            180.0,
            210.0,
        ));

        let mut page2 = crate::types::internal::InternalDocument::new("test");
        page2.push_element(ocr_font_block(
            "Further discussion continues here with more explanatory prose for the reader.",
            "11",
            10.0,
            40.0,
        ));
        page2.push_element(ocr_font_block(
            "Another paragraph of body text appears on the second page of the document.",
            "11",
            50.0,
            80.0,
        ));

        vec![
            crate::pdf::structure::adapters::ocr_doc_to_paragraphs(
                &page1,
                1000,
                crate::pdf::structure::adapters::OcrFontSizeScale::uniform(1.0),
            ),
            crate::pdf::structure::adapters::ocr_doc_to_paragraphs(
                &page2,
                1000,
                crate::pdf::structure::adapters::OcrFontSizeScale::uniform(1.0),
            ),
        ]
    }

    /// Pins the WP-A fix: today (before wiring OCR paragraphs through
    /// `extract_document_structure_from_segments`) `heuristically_restructured_ocr_pages`
    /// does not exist and the OCR route never promotes anything, so a scanned-PDF
    /// Markdown extraction shows zero headings and zero list items regardless of how
    /// font sizes vary across the page (`dominant_font_size` was hardcoded to `12.0`
    /// for every OCR paragraph). This must fail against that code: it asserts both a
    /// `Heading` and a `ListItem` element are present.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_promotes_headings_and_lists_for_markdown() {
        let pages = heading_and_list_ocr_pages();
        let mut config = ExtractionConfig::default();
        config.output_format = crate::core::config::OutputFormat::Markdown;

        let doc = heuristically_restructured_ocr_pages(&pages, &[1000.0, 1000.0], &[], &config)
            .expect("font-size variation and list markers should produce a restructured document");

        assert!(
            doc.elements
                .iter()
                .any(|element| matches!(element.kind, crate::types::internal::ElementKind::Heading { .. })),
            "expected at least one Heading element, got kinds: {:?}",
            doc.elements.iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
        assert!(
            doc.elements
                .iter()
                .any(|element| matches!(element.kind, crate::types::internal::ElementKind::ListItem { .. })),
            "expected at least one ListItem element, got kinds: {:?}",
            doc.elements.iter().map(|e| &e.kind).collect::<Vec<_>>()
        );
    }

    /// Pins the `Plain` gate: the exact same input that produces headings and list
    /// items under `Markdown` (previous test) must be left untouched under `Plain` --
    /// `heuristically_restructured_ocr_pages` must return `None` so the caller keeps
    /// its pre-existing, unstructured assembly and Plain-format output stays
    /// byte-identical to before WP-A. A version of the fix that forgot the
    /// `output_format` gate (ran the heuristic unconditionally) would fail this.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_is_noop_for_plain_output() {
        let pages = heading_and_list_ocr_pages();
        let config = ExtractionConfig::default();
        assert_eq!(config.output_format, crate::core::config::OutputFormat::Plain);

        assert!(heuristically_restructured_ocr_pages(&pages, &[1000.0, 1000.0], &[], &config).is_none());
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_applies_explicit_content_filter_to_plain_output() {
        let pages = heading_and_list_ocr_pages();
        let config = ExtractionConfig {
            content_filter: Some(crate::core::config::ContentFilterConfig::default()),
            ..ExtractionConfig::default()
        };

        assert!(heuristically_restructured_ocr_pages(&pages, &[1000.0, 1000.0], &[], &config).is_some());
    }

    /// Pins the ML-layout precedence: when `ocr_doc_to_layout_paragraphs` (or
    /// `promote_anchored_ordered_list_sequences`) has already classified a paragraph
    /// as a heading or list item, the segment heuristic must not run at all --
    /// re-deriving structure from bare segments would discard that classification
    /// instead of adding to it (the ML-layout OCR route already measures correct
    /// headings on the reference fixture; this test only pins the *skip*, not that
    /// route's own output).
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_skips_when_already_layout_classified() {
        let mut pages = heading_and_list_ocr_pages();
        pages[0][0].heading_level = Some(1);
        let mut config = ExtractionConfig::default();
        config.output_format = crate::core::config::OutputFormat::Markdown;

        assert!(heuristically_restructured_ocr_pages(&pages, &[1000.0, 1000.0], &[], &config).is_none());
    }

    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_filters_already_classified_pages_when_requested() {
        let mut pages = heading_and_list_ocr_pages();
        pages[0][0].heading_level = Some(1);
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            content_filter: Some(crate::core::config::ContentFilterConfig::default()),
            ..ExtractionConfig::default()
        };

        assert!(heuristically_restructured_ocr_pages(&pages, &[1000.0, 1000.0], &[], &config).is_some());
    }

    /// Pins a content-loss defect surfaced while A/B-testing hybrid OCR on scanned documents: the VLM
    /// backend (`llm::vlm_ocr::VlmOcrBackend::process_image`) never populates
    /// `ocr_internal_document` or `ocr_elements` -- it returns bare markdown `content`
    /// plus `tables` parsed separately out of that same text via `extract_gfm_tables`,
    /// with `bounding_box: None` on every table. The page's paragraphs are then built by
    /// `ocr_text_to_paragraphs`, which puts the page text in `PdfParagraph.text` and
    /// leaves `.lines` empty (there is no per-line geometry to carry).
    ///
    /// `segments_from_ocr_pages` -- this heuristic's only way to see page content --
    /// harvests `SegmentData` exclusively from `paragraph.lines`, so it sees zero
    /// segments for every such page regardless of how much prose `.text` actually
    /// holds. `extract_document_structure_from_segments` therefore reconstructs zero
    /// paragraphs. When no table exists either, the resulting document has no elements
    /// at all, `!doc.elements.is_empty()` is false, and this function correctly returns
    /// `None` so the caller's own text-based fallback assembly (which does read
    /// `.text`) wins.
    ///
    /// But `assemble_internal_document` still emits a `Table` element for every
    /// `tables` entry even when every page's paragraph list is empty. So the instant a
    /// VLM page contains at least one GFM table, the reconstructed document is
    /// non-empty -- it holds exactly the table elements -- and `Some(doc)` wins over
    /// the caller's fallback, discarding every paragraph of prose the page actually
    /// had. This must fail against that code: it asserts the prose surrounding the
    /// table survives whenever this function accepts its own restructured document.
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_preserves_prose_around_a_bare_text_table() {
        let page_text = "Before the table, prose explains what follows.\n\n\
                          | Col A | Col B |\n\
                          | --- | --- |\n\
                          | 1 | 2 |\n\n\
                          After the table, more prose continues the document.";
        let pages = vec![crate::pdf::structure::adapters::ocr_text_to_paragraphs(page_text)];
        let table = ocr_table("| Col A | Col B |\n| --- | --- |\n| 1 | 2 |", 1);

        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..ExtractionConfig::default()
        };

        let result = heuristically_restructured_ocr_pages(&pages, &[1000.0], std::slice::from_ref(&table), &config);

        // The fix makes this return None so the caller's `.text` fallback wins, and BEFORE the
        // fix it returned Some(doc) holding only the Table. Asserting only inside `if let Some`
        // therefore passes vacuously post-fix and tests nothing, so pin the outcome first: a
        // prose-free reconstruction must be rejected outright. ~keep
        assert!(
            result.is_none(),
            "a reconstruction holding only a table must be rejected so the caller's text fallback \
             is used; got: {:?}",
            result
                .as_ref()
                .map(|d| d.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>())
        );

        // Retained so that if the heuristic is ever changed to return a document here, that
        // document is still held to preserving the prose around the table. ~keep
        if let Some(doc) = result {
            let rendered = doc
                .elements
                .iter()
                .map(|element| element.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            assert!(
                rendered.contains("Before the table"),
                "prose before the table was dropped; elements: {:?}",
                doc.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>()
            );
            assert!(
                rendered.contains("After the table"),
                "prose after the table was dropped; elements: {:?}",
                doc.elements.iter().map(|e| (&e.kind, &e.text)).collect::<Vec<_>>()
            );
        }
    }

    /// A document-wide "some prose survived" check is insufficient for hybrid OCR: a
    /// geometry-backed page can contribute a valid heading or paragraph while a separate
    /// bare-text page contributes no segments at all. A table on that bare-text page still
    /// makes the combined reconstruction non-empty, so accepting it would silently discard
    /// only the second page's prose. The heuristic must decline the combined reconstruction
    /// and let the caller's per-page text fallback preserve both pages. ~keep
    #[cfg(any(feature = "ocr", feature = "ocr-pipeline"))]
    #[test]
    fn heuristically_restructured_ocr_pages_rejects_mixed_document_when_one_page_loses_prose() {
        let structured_page = heading_and_list_ocr_pages().remove(0);
        let bare_page_text = "Second-page prose before the table.\n\n\
                              | Col A | Col B |\n\
                              | --- | --- |\n\
                              | 1 | 2 |\n\n\
                              Second-page prose after the table.";
        let bare_page = crate::pdf::structure::adapters::ocr_text_to_paragraphs(bare_page_text);
        let pages = vec![structured_page, bare_page];
        let table = ocr_table("| Col A | Col B |\n| --- | --- |\n| 1 | 2 |", 2);
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..ExtractionConfig::default()
        };

        let result =
            heuristically_restructured_ocr_pages(&pages, &[1000.0, 1000.0], std::slice::from_ref(&table), &config);

        assert!(
            result.is_none(),
            "a reconstruction that loses one page's prose must be rejected even when another \
             page contributed structured prose; got: {:?}",
            result.as_ref().map(|document| {
                document
                    .elements
                    .iter()
                    .map(|element| (element.page, &element.kind, &element.text))
                    .collect::<Vec<_>>()
            })
        );
    }

    /// Closes the gap this session's fix targets: before it,
    /// `heuristically_restructured_ocr_pages` had exactly one call site, inside
    /// `extract_with_ocr_for_page` -- but `extract_mixed_ocr_native`'s single-backend
    /// route (`--ocr-scanned-pages` / `--force-ocr-pages` with no pipeline configured)
    /// builds its per-page document via `build_mixed_ocr_page_document` ->
    /// `assemble_mixed_ocr_page_document`, which goes straight from bare paragraphs to
    /// `assemble_internal_document` and never calls the heuristic at all. So a scanned
    /// PDF extracted through this route emitted zero headings under Markdown output no
    /// matter how much its font sizes varied.
    ///
    /// The mock backend returns the *same* heading+body content for every page
    /// (`ocr_font_block`, reused from `heading_and_list_ocr_pages`'s per-page fixture)
    /// deliberately: this route's per-page OCR calls run concurrently through a
    /// `JoinSet`, so nothing here may depend on call order or backend-visible page
    /// identity.
    ///
    /// Fails on unfixed code: `structured_ocr_pages` contains only `Paragraph`
    /// elements (from `assemble_internal_document` over unclassified paragraphs), no
    /// `Heading`.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_single_backend_route_promotes_document_global_headings() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct HeadingMockBackend;

        fn heading_body_document() -> crate::types::internal::InternalDocument {
            let mut doc = crate::types::internal::InternalDocument::new("test");
            doc.push_element(ocr_font_block("ANNUAL REPORT OVERVIEW", "28", 10.0, 50.0));
            doc.push_element(ocr_font_block(
                "This document summarizes the annual results for the reporting period in detail.",
                "11",
                60.0,
                90.0,
            ));
            doc.push_element(ocr_font_block(
                "1. First item in the numbered list of findings",
                "11",
                100.0,
                130.0,
            ));
            doc.push_element(ocr_font_block(
                "2. Second item continuing the numbered list of findings",
                "11",
                140.0,
                170.0,
            ));
            doc.push_element(ocr_font_block(
                "Additional narrative text follows describing the broader context for readers.",
                "11",
                180.0,
                210.0,
            ));
            doc
        }

        #[async_trait::async_trait]
        impl OcrBackend for HeadingMockBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "ANNUAL REPORT OVERVIEW body text with enough words to avoid the \
                              recognition-noise gate"
                        .to_string(),
                    ocr_internal_document: Some(heading_body_document()),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for HeadingMockBackend {
            fn name(&self) -> &str {
                "doc-global-heuristic-mixed-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(HeadingMockBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: "doc-global-heuristic-mixed-mock".to_string(),
                ..Default::default()
            }),
            output_format: crate::core::config::OutputFormat::Markdown,
            pdf_options: Some(pdf_config_with_disabled_page_margins()),
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[1, 2], &pdf, &config, None)
            .await
            .expect("extract_mixed_ocr_native must succeed");

        crate::plugins::unregister_ocr_backend("doc-global-heuristic-mixed-mock").unwrap();

        let structured_pages = result.2;
        assert!(
            !structured_pages.is_empty(),
            "expected at least one structured OCR page"
        );
        let all_kinds: Vec<_> = structured_pages
            .values()
            .flat_map(|doc| doc.elements.iter().map(|e| e.kind))
            .collect();
        assert!(
            all_kinds
                .iter()
                .any(|kind| matches!(kind, crate::types::internal::ElementKind::Heading { .. })),
            "expected the document-global heading heuristic to promote a Heading element on \
             the mixed OCR single-backend route; got element kinds: {all_kinds:?}"
        );
    }

    /// Same defect, pipeline route: `run_ocr_pipeline_for_page` drives each OCR'd page
    /// through `extract_with_ocr_for_page` with exactly one detached image per call
    /// (`std::slice::from_ref`), so even though that function's heuristic call site
    /// technically runs, it only ever sees one page at a time -- too few blocks/pages
    /// for `build_heading_map`'s clustering to promote anything reliably, and (before
    /// `skip_document_global_heuristic` existed) any classification it *did* manage
    /// would mark the page's paragraphs "already structured" and block this test's
    /// document-wide pass from running at all. `points_per_pixel_override` and
    /// `skip_document_global_heuristic` together are what let this route feed the
    /// same document-global pass every page's bare paragraphs at once, exactly like
    /// the single-backend route above.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn mixed_ocr_pipeline_route_promotes_document_global_headings() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage, OcrQualityThresholds};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::{ExtractedDocument, PageBoundary};
        use std::sync::Arc;

        struct HeadingMockPipelineBackend;

        fn heading_body_document() -> crate::types::internal::InternalDocument {
            let mut doc = crate::types::internal::InternalDocument::new("test");
            doc.push_element(ocr_font_block("ANNUAL REPORT OVERVIEW", "28", 10.0, 50.0));
            doc.push_element(ocr_font_block(
                "This document summarizes the annual results for the reporting period in detail.",
                "11",
                60.0,
                90.0,
            ));
            doc.push_element(ocr_font_block(
                "1. First item in the numbered list of findings",
                "11",
                100.0,
                130.0,
            ));
            doc.push_element(ocr_font_block(
                "2. Second item continuing the numbered list of findings",
                "11",
                140.0,
                170.0,
            ));
            doc.push_element(ocr_font_block(
                "Additional narrative text follows describing the broader context for readers.",
                "11",
                180.0,
                210.0,
            ));
            doc
        }

        #[async_trait::async_trait]
        impl OcrBackend for HeadingMockPipelineBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Ok(ExtractedDocument {
                    content: "ANNUAL REPORT OVERVIEW body text with enough words to avoid the \
                              recognition-noise gate"
                        .to_string(),
                    ocr_internal_document: Some(heading_body_document()),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for HeadingMockPipelineBackend {
            fn name(&self) -> &str {
                "doc-global-heuristic-pipeline-mock"
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(HeadingMockPipelineBackend)).unwrap();

        let pdf = build_minimal_two_page_pdf(612.0, 792.0);
        let page1_text = "page one native text";
        let page2_text = "page two native text";
        let native_text = format!("{page1_text}\n{page2_text}");
        let boundaries = vec![
            PageBoundary {
                byte_start: 0,
                byte_end: page1_text.len(),
                page_number: 1,
            },
            PageBoundary {
                byte_start: page1_text.len() + 1,
                byte_end: native_text.len(),
                page_number: 2,
            },
        ];

        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                pipeline: Some(OcrPipelineConfig {
                    stages: vec![OcrPipelineStage {
                        backend: "doc-global-heuristic-pipeline-mock".to_string(),
                        priority: 100,
                        language: None,
                        tesseract_config: None,
                        paddle_ocr_config: None,
                        vlm_config: None,
                        backend_options: None,
                    }],
                    // Accept the first (only) stage unconditionally: this test exercises
                    // structure promotion, not the quality-based selection policy.
                    quality_thresholds: OcrQualityThresholds {
                        pipeline_min_quality: 0.0,
                        ..Default::default()
                    },
                }),
                ..Default::default()
            }),
            output_format: crate::core::config::OutputFormat::Markdown,
            pdf_options: Some(pdf_config_with_disabled_page_margins()),
            ..Default::default()
        };

        let result = extract_mixed_ocr_native(&native_text, &boundaries, &[1, 2], &pdf, &config, None)
            .await
            .expect("extract_mixed_ocr_native must succeed");

        crate::plugins::unregister_ocr_backend("doc-global-heuristic-pipeline-mock").unwrap();

        let structured_pages = result.2;
        assert!(
            !structured_pages.is_empty(),
            "expected at least one structured OCR page"
        );
        let all_kinds: Vec<_> = structured_pages
            .values()
            .flat_map(|doc| doc.elements.iter().map(|e| e.kind))
            .collect();
        assert!(
            all_kinds
                .iter()
                .any(|kind| matches!(kind, crate::types::internal::ElementKind::Heading { .. })),
            "expected the document-global heading heuristic to promote a Heading element on \
             the mixed OCR pipeline route; got element kinds: {all_kinds:?}"
        );
    }

    /// Text an OCR backend returns when handed the recovered image XObject bytes.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    const XOBJECT_RECOVERED_TEXT: &str = "RECOVERED FROM EMBEDDED IMAGE XOBJECT";

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn xobject_test_payload(content: &str) -> crate::types::ExtractedDocument {
        use crate::types::{Formula, ImagePreprocessingMetadata, LlmUsage, Metadata, Table};

        crate::types::ExtractedDocument {
            content: content.to_string(),
            metadata: Metadata {
                image_preprocessing: Some(ImagePreprocessingMetadata {
                    original_dimensions: (10, 20).into(),
                    original_dpi: (72.0, 72.0).into(),
                    target_dpi: 321,
                    scale_factor: 1.0,
                    auto_adjusted: false,
                    final_dpi: 321,
                    new_dimensions: None,
                    resample_method: "test".to_string(),
                    dimension_clamped: false,
                    calculated_dpi: None,
                    skipped_resize: true,
                    resize_error: None,
                }),
                ..Default::default()
            },
            tables: vec![Table {
                cells: vec![vec!["header".to_string()], vec!["value".to_string()]],
                markdown: "| header |\n| --- |\n| value |\n".to_string(),
                page_number: 99,
                ..Default::default()
            }],
            formulas: vec![Formula {
                latex: "x^2".to_string(),
                bbox: None,
                page: Some(99),
            }],
            llm_usage: Some(vec![LlmUsage {
                model: "recovery-model".to_string(),
                source: "vlm_ocr".to_string(),
                input_tokens: Some(100),
                output_tokens: Some(50),
                total_tokens: Some(150),
                estimated_cost: Some(0.001),
                finish_reason: Some("stop".to_string()),
            }]),
            ..Default::default()
        }
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn fallback_test_images(count: usize) -> Vec<crate::pdf::native::images::PageFallbackImage> {
        (0..count)
            .map(|_| crate::pdf::native::images::PageFallbackImage {
                bytes: bytes::Bytes::from_static(b"ignored image bytes"),
                format: "jpeg",
                recovery: crate::pdf::native::images::XObjectRecovery::EmbeddedJpeg,
            })
            .collect()
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    struct XObjectPayloadBackend {
        result: crate::types::ExtractedDocument,
        calls: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    impl crate::plugins::Plugin for XObjectPayloadBackend {
        fn name(&self) -> &str {
            "xobject-payload-test-backend"
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> crate::Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> crate::Result<()> {
            Ok(())
        }
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[async_trait::async_trait]
    impl crate::plugins::OcrBackend for XObjectPayloadBackend {
        fn backend_type(&self) -> crate::plugins::OcrBackendType {
            crate::plugins::OcrBackendType::Custom
        }

        fn supports_language(&self, _: &str) -> bool {
            true
        }

        async fn process_image(
            &self,
            _: &[u8],
            _: &crate::core::config::OcrConfig,
        ) -> crate::Result<crate::types::ExtractedDocument> {
            self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(self.result.clone())
        }
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn xobject_recovery_preserves_payload_and_renumbers_document_page() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend: std::sync::Arc<dyn crate::plugins::OcrBackend> = std::sync::Arc::new(XObjectPayloadBackend {
            result: xobject_test_payload(XOBJECT_RECOVERED_TEXT),
            calls,
        });
        let limits = crate::extractors::security::SecurityLimits::default();
        let mut budget = crate::extractors::security::SecurityBudget::from_limits(&limits);

        let recovery = recover_image_xobjects(
            &backend,
            &fallback_test_images(1),
            6,
            &crate::core::config::OcrConfig::default(),
            &mut budget,
        )
        .await
        .expect("bounded recovery must succeed");

        assert_eq!(recovery.text, XOBJECT_RECOVERED_TEXT);
        assert_eq!(recovery.llm_usage.len(), 1);
        assert_eq!(recovery.tables.len(), 1);
        assert_eq!(recovery.tables[0].page_number, 7);
        assert_eq!(recovery.formulas.len(), 1);
        assert_eq!(recovery.formulas[0].page, Some(7));
        assert_eq!(
            recovery.image_preprocessing.expect("metadata must survive").target_dpi,
            321
        );
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn xobject_recovery_rejects_nonempty_output_over_content_budget() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend: std::sync::Arc<dyn crate::plugins::OcrBackend> = std::sync::Arc::new(XObjectPayloadBackend {
            result: crate::types::ExtractedDocument {
                content: "oversized".to_string(),
                ..Default::default()
            },
            calls: calls.clone(),
        });
        let limits = crate::extractors::security::SecurityLimits {
            max_content_size: 8,
            ..Default::default()
        };
        let mut budget = crate::extractors::security::SecurityBudget::from_limits(&limits);

        let error = recover_image_xobjects(
            &backend,
            &fallback_test_images(1),
            0,
            &crate::core::config::OcrConfig::default(),
            &mut budget,
        )
        .await
        .expect_err("nonempty recovery output above max_content_size must fail closed");

        assert!(matches!(error, crate::XbergError::Security { .. }));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    async fn assert_xobject_result_exceeds_content_budget(result: crate::types::ExtractedDocument) {
        let backend: std::sync::Arc<dyn crate::plugins::OcrBackend> = std::sync::Arc::new(XObjectPayloadBackend {
            result,
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let limits = crate::extractors::security::SecurityLimits {
            max_content_size: 8,
            ..Default::default()
        };
        let mut budget = crate::extractors::security::SecurityBudget::from_limits(&limits);

        let error = recover_image_xobjects(
            &backend,
            &fallback_test_images(1),
            0,
            &crate::core::config::OcrConfig::default(),
            &mut budget,
        )
        .await
        .expect_err("structured recovery text above max_content_size must fail closed");

        assert!(matches!(error, crate::XbergError::Security { .. }));
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn xobject_recovery_rejects_oversized_structured_text() {
        use crate::types::{ExtractedDocument, Formula, ImagePreprocessingMetadata, LlmUsage, Metadata, Table};

        let oversized = "123456789".to_string();
        let results = [
            ExtractedDocument {
                tables: vec![Table {
                    markdown: oversized.clone(),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ExtractedDocument {
                tables: vec![Table {
                    cells: vec![vec![oversized.clone()]],
                    ..Default::default()
                }],
                ..Default::default()
            },
            ExtractedDocument {
                tables: vec![Table {
                    table_id: Some(oversized.clone()),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ExtractedDocument {
                tables: vec![Table {
                    columns: Some(vec![oversized.clone()]),
                    ..Default::default()
                }],
                ..Default::default()
            },
            ExtractedDocument {
                llm_usage: Some(vec![LlmUsage {
                    model: oversized.clone(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ExtractedDocument {
                llm_usage: Some(vec![LlmUsage {
                    source: oversized.clone(),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ExtractedDocument {
                llm_usage: Some(vec![LlmUsage {
                    finish_reason: Some(oversized.clone()),
                    ..Default::default()
                }]),
                ..Default::default()
            },
            ExtractedDocument {
                metadata: Metadata {
                    image_preprocessing: Some(ImagePreprocessingMetadata {
                        original_dimensions: (1, 1).into(),
                        original_dpi: (72.0, 72.0).into(),
                        target_dpi: 72,
                        scale_factor: 1.0,
                        auto_adjusted: false,
                        final_dpi: 72,
                        new_dimensions: None,
                        resample_method: oversized.clone(),
                        dimension_clamped: false,
                        calculated_dpi: None,
                        skipped_resize: true,
                        resize_error: None,
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            ExtractedDocument {
                metadata: Metadata {
                    image_preprocessing: Some(ImagePreprocessingMetadata {
                        original_dimensions: (1, 1).into(),
                        original_dpi: (72.0, 72.0).into(),
                        target_dpi: 72,
                        scale_factor: 1.0,
                        auto_adjusted: false,
                        final_dpi: 72,
                        new_dimensions: None,
                        resample_method: String::new(),
                        dimension_clamped: false,
                        calculated_dpi: None,
                        skipped_resize: false,
                        resize_error: Some(oversized.clone()),
                    }),
                    ..Default::default()
                },
                ..Default::default()
            },
            ExtractedDocument {
                formulas: vec![Formula {
                    latex: oversized,
                    bbox: None,
                    page: None,
                }],
                ..Default::default()
            },
        ];

        for result in results {
            assert_xobject_result_exceeds_content_budget(result).await;
        }
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn xobject_recovery_rejects_tables_over_cell_budget() {
        let backend: std::sync::Arc<dyn crate::plugins::OcrBackend> = std::sync::Arc::new(XObjectPayloadBackend {
            result: crate::types::ExtractedDocument {
                tables: vec![crate::types::Table {
                    cells: vec![vec!["one".to_string(), "two".to_string()]],
                    ..Default::default()
                }],
                ..Default::default()
            },
            calls: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        });
        let limits = crate::extractors::security::SecurityLimits {
            max_table_cells: 1,
            ..Default::default()
        };
        let mut budget = crate::extractors::security::SecurityBudget::from_limits(&limits);

        let error = recover_image_xobjects(
            &backend,
            &fallback_test_images(1),
            0,
            &crate::core::config::OcrConfig::default(),
            &mut budget,
        )
        .await
        .expect_err("recovery tables above max_table_cells must fail closed");

        assert!(matches!(error, crate::XbergError::Security { .. }));
    }

    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn xobject_recovery_bounds_backend_attempts_by_iteration_budget() {
        let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let backend: std::sync::Arc<dyn crate::plugins::OcrBackend> = std::sync::Arc::new(XObjectPayloadBackend {
            result: crate::types::ExtractedDocument {
                content: "ok".to_string(),
                ..Default::default()
            },
            calls: calls.clone(),
        });
        let limits = crate::extractors::security::SecurityLimits {
            max_iterations: 1,
            ..Default::default()
        };
        let mut budget = crate::extractors::security::SecurityBudget::from_limits(&limits);

        let error = recover_image_xobjects(
            &backend,
            &fallback_test_images(2),
            0,
            &crate::core::config::OcrConfig::default(),
            &mut budget,
        )
        .await
        .expect_err("more XObject attempts than max_iterations must fail closed");

        assert!(matches!(error, crate::XbergError::Security { .. }));
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// The reporter's error in #1444, verbatim.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    const VLM_NO_CONTENT_ERROR: &str = "VLM OCR returned no content";

    /// Single page carrying exactly one embedded DCT/JPEG image XObject; already relied on
    /// by `test_page_ocr_fallback_image_bytes_recovers_real_image` in
    /// `crate::pdf::native::images`.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn single_xobject_fixture_bytes() -> Vec<u8> {
        let pdf_path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
        assert!(pdf_path.exists(), "missing fixture: {}", pdf_path.display());
        std::fs::read(&pdf_path).expect("failed to read test PDF fixture")
    }

    /// Whether these bytes are the recovered image XObject (an embedded JPEG) rather than
    /// the whole-page raster, which this module always encodes as PNG.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    fn is_embedded_jpeg(data: &[u8]) -> bool {
        data.starts_with(&[0xFF, 0xD8])
    }

    /// #1444, dominant hole: a per-page backend failure propagated straight out of
    /// `extract_with_ocr` with `?`, aborting the whole document *before* the blank-page
    /// image-XObject fallback further down the same loop could ever run. That is exactly the
    /// reporter's symptom -- a scanned PDF dying with "OCR error: VLM OCR returned no
    /// content" while the recovery that would have saved the page sat unreachable.
    ///
    /// The page must now degrade into that fallback, and the failure must still be visible.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_page_from_image_xobjects_when_the_backend_errors_on_the_page_raster() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "page-raster-error-fallback-test-backend";

        struct FailOnPageRasterBackend;

        #[async_trait::async_trait]
        impl OcrBackend for FailOnPageRasterBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, data: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                if is_embedded_jpeg(data) {
                    Ok(ExtractedDocument {
                        content: XOBJECT_RECOVERED_TEXT.to_string(),
                        ..Default::default()
                    })
                } else {
                    Err(crate::XbergError::Plugin {
                        message: VLM_NO_CONTENT_ERROR.to_string(),
                        plugin_name: "ocr".to_string(),
                    })
                }
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FailOnPageRasterBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(FailOnPageRasterBackend)).unwrap();

        let pdf_bytes = single_xobject_fixture_bytes();
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, _, doc, _, _, _, _, _, _) =
            result.expect("a per-page backend failure must degrade to the XObject fallback, not abort the document");
        assert_eq!(
            text, XOBJECT_RECOVERED_TEXT,
            "the failed page's text must come from its embedded image XObject"
        );

        let warnings = doc
            .expect("the fallback and failure warnings must produce an internal document")
            .processing_warnings;
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("OCR was retried on the embedded image bytes")),
            "the XObject fallback must be reported; got: {warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("OCR of page 1 failed") && w.message.contains(VLM_NO_CONTENT_ERROR)),
            "the backend failure must survive as a warning rather than vanish; got: {warnings:?}"
        );
    }

    /// #1444, holes 2 and 4 together. Two independent reasons the fallback could not fire
    /// for a layout-detected scanned page:
    ///
    /// 1. pre-rendered `images` left `lazy_pdf_render_state` unopened, so the fallback had
    ///    no document to read XObjects from and was skipped outright;
    /// 2. a chatty backend that *describes* the blank page ("The image is entirely blank.")
    ///    clears `is_page_text_blank`'s 3-non-whitespace-character floor, so the page did
    ///    not even look blank.
    ///
    /// Here the supplied page raster is pure white -- there is no ink on it at all -- so the
    /// backend's answer is a description, not a transcription, and the page's embedded image
    /// XObject is what actually carries the text.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_blank_inked_page_when_backend_describes_blankness_over_prerendered_images() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "chatty-blank-description-test-backend";
        const BLANK_DESCRIPTION: &str = "The image is entirely blank.";

        struct DescribesBlankPageBackend;

        #[async_trait::async_trait]
        impl OcrBackend for DescribesBlankPageBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, data: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let content = if is_embedded_jpeg(data) {
                    XOBJECT_RECOVERED_TEXT
                } else {
                    BLANK_DESCRIPTION
                };
                Ok(ExtractedDocument {
                    content: content.to_string(),
                    ..Default::default()
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for DescribesBlankPageBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(DescribesBlankPageBackend)).unwrap();

        // Stands in for what layout detection hands the OCR loop: an already-rendered page
        // image. All white, i.e. exactly what xberg_native_pdf substitutes for a page whose image
        // XObjects it could not draw.
        let blank_page = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(200, 260, image::Rgb([255; 3])));
        let pdf_bytes = single_xobject_fixture_bytes();
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&pdf_bytes),
            Some(std::slice::from_ref(&blank_page)),
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, _, _, _, _, _, _, _, _, _, _) = result.expect("OCR must succeed");
        assert_eq!(
            text, XOBJECT_RECOVERED_TEXT,
            "a description of a blank raster must not be accepted as that page's transcription"
        );
    }

    /// #1444, hole 3: when every pipeline stage errors outright, no stage ever reached its
    /// own per-page fallback, and `run_ocr_pipeline` gave up with "All OCR pipeline backends
    /// failed" having never looked at the pages' embedded images.
    ///
    /// The backend here fails its first two calls -- the whole-page raster, then that page's
    /// one image XObject inside the stage -- so the stage genuinely produces nothing and the
    /// pipeline exhausts its only stage. The third call is the pipeline's own last-resort
    /// recovery, which must run and must succeed.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_recover_from_image_xobjects_when_every_pipeline_stage_fails() {
        use crate::core::config::{OcrConfig, OcrPipelineConfig, OcrPipelineStage};
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        const BACKEND_NAME: &str = "pipeline-all-stages-fail-test-backend";

        struct FailsFirstTwoCallsBackend {
            calls: Arc<AtomicUsize>,
        }

        #[async_trait::async_trait]
        impl OcrBackend for FailsFirstTwoCallsBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                let call = self.calls.fetch_add(1, Ordering::SeqCst) + 1;
                if call <= 2 {
                    return Err(crate::XbergError::Plugin {
                        message: VLM_NO_CONTENT_ERROR.to_string(),
                        plugin_name: "ocr".to_string(),
                    });
                }
                Ok(xobject_test_payload(XOBJECT_RECOVERED_TEXT))
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for FailsFirstTwoCallsBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        let calls = Arc::new(AtomicUsize::new(0));
        crate::plugins::register_ocr_backend(Arc::new(FailsFirstTwoCallsBackend { calls: calls.clone() })).unwrap();

        let pdf_bytes = single_xobject_fixture_bytes();
        let pipeline = OcrPipelineConfig {
            stages: vec![OcrPipelineStage {
                backend: BACKEND_NAME.to_string(),
                priority: 100,
                language: None,
                tesseract_config: None,
                paddle_ocr_config: None,
                vlm_config: None,
                backend_options: None,
            }],
            quality_thresholds: Default::default(),
        };
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                pipeline: Some(pipeline.clone()),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = run_ocr_pipeline(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            &pipeline,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let (text, tables, _, doc, usage, _, _, formulas, preprocessing, _) =
            result.expect("the pipeline must try the pages' embedded images before reporting total failure");
        assert_eq!(text, XOBJECT_RECOVERED_TEXT);
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "expected one page-raster call, one in-stage fallback call, and one pipeline-level recovery call"
        );
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].model, "recovery-model");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].page_number, 1);
        assert_eq!(formulas.len(), 1);
        assert_eq!(formulas[0].page, Some(1));
        assert_eq!(preprocessing[&1].target_dpi, 321);

        let warnings = doc
            .expect("recovery warnings must produce a document")
            .processing_warnings;
        assert!(
            warnings
                .iter()
                .any(|w| w.message.contains("OCR was retried on the embedded image bytes")),
            "the pipeline-level recovery must be reported; got: {warnings:?}"
        );
    }

    /// #1444: degrading per-page failures to warnings must not turn a wholesale OCR failure
    /// into a silently empty document. This PDF has no content stream and no image XObjects,
    /// so there is nothing to recover and the error must still be reported -- but as one
    /// aggregate failure naming the page count, not as the raw first backend error.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[tokio::test]
    async fn should_error_when_every_page_fails_and_nothing_can_be_recovered() {
        use crate::core::config::OcrConfig;
        use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
        use crate::types::ExtractedDocument;
        use std::sync::Arc;

        const BACKEND_NAME: &str = "always-failing-unrecoverable-test-backend";

        struct AlwaysFailingBackend;

        #[async_trait::async_trait]
        impl OcrBackend for AlwaysFailingBackend {
            fn backend_type(&self) -> OcrBackendType {
                OcrBackendType::Custom
            }
            fn supports_language(&self, _: &str) -> bool {
                true
            }
            async fn process_image(&self, _: &[u8], _: &OcrConfig) -> crate::Result<ExtractedDocument> {
                Err(crate::XbergError::Plugin {
                    message: VLM_NO_CONTENT_ERROR.to_string(),
                    plugin_name: "ocr".to_string(),
                })
            }
            fn supports_document_processing(&self) -> bool {
                false
            }
        }

        impl Plugin for AlwaysFailingBackend {
            fn name(&self) -> &str {
                BACKEND_NAME
            }
            fn version(&self) -> String {
                "1.0.0".to_string()
            }
            fn initialize(&self) -> crate::Result<()> {
                Ok(())
            }
            fn shutdown(&self) -> crate::Result<()> {
                Ok(())
            }
        }

        crate::plugins::register_ocr_backend(Arc::new(AlwaysFailingBackend)).unwrap();

        let pdf_bytes = crate::pdf::render::build_minimal_pdf_with_mediabox(612.0, 792.0);
        let config = ExtractionConfig {
            ocr: Some(OcrConfig {
                backend: BACKEND_NAME.to_string(),
                ..Default::default()
            }),
            ..Default::default()
        };

        let result = extract_with_ocr(
            Some(&pdf_bytes),
            None,
            #[cfg(feature = "layout-detection")]
            None,
            &config,
            None,
        )
        .await;

        crate::plugins::unregister_ocr_backend(BACKEND_NAME).unwrap();

        let error = result
            .expect_err("a document with no recoverable page must still fail")
            .to_string();
        assert!(
            error.contains("OCR failed on all 1 page(s)") && error.contains(VLM_NO_CONTENT_ERROR),
            "the aggregate failure must name the page count and the root cause; got: {error}"
        );
    }

    /// Supporting unit test for the ink sentinel (not a regression test -- the function is
    /// new in #1444). Pins the two contracts the guard depends on: an all-white raster reads
    /// as blank, and a raster carrying even a small mark does not.
    #[cfg(all(feature = "pdf", any(feature = "ocr", feature = "ocr-pipeline")))]
    #[test]
    fn page_raster_is_blank_distinguishes_an_empty_raster_from_an_inked_one() {
        fn encode_png(img: &image::RgbImage) -> Vec<u8> {
            use image::ImageEncoder;
            let mut out = std::io::Cursor::new(Vec::new());
            image::codecs::png::PngEncoder::new(&mut out)
                .write_image(img, img.width(), img.height(), image::ColorType::Rgb8.into())
                .expect("PNG encode must succeed");
            out.into_inner()
        }

        let white = image::RgbImage::from_pixel(400, 500, image::Rgb([255; 3]));
        let limits = crate::extractors::security::SecurityLimits::default();
        assert!(page_raster_is_blank(&encode_png(&white), &limits));

        let mut inked = white.clone();
        for y in 100..140 {
            for x in 100..140 {
                inked.put_pixel(x, y, image::Rgb([0; 3]));
            }
        }
        assert!(!page_raster_is_blank(&encode_png(&inked), &limits));

        // A short answer over an inked raster is a real (if terse) transcription and must
        // not be escalated; over a blank one it is a description of blankness.
        assert!(!page_needs_xobject_fallback(
            "Invoice 4471",
            &encode_png(&inked),
            &limits
        ));
        assert!(page_needs_xobject_fallback(
            "The image is entirely blank.",
            &encode_png(&white),
            &limits,
        ));
    }

    /// Tesseract-shaped input: a calibrated `Legibility` scale with an in-range raw mean.
    /// The upstream mean is an integer percent, so 0.85 is exactly representable and an
    /// exact `assert_eq!` (rather than an epsilon comparison) is valid here.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_normalizes_legibility_score_by_scale_max() {
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 100.0 };
        let result = page_ocr_confidence(semantics, Some(85.0), 120, "tesseract").unwrap();
        assert_eq!(result.score, Some(0.85));
        assert_eq!(result.word_count, 120);
        assert_eq!(result.backend, "tesseract");
    }

    /// `Uncalibrated` semantics never yield a score, no matter what raw number is reported,
    /// but the caller must still learn the page was OCR'd and by which backend.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_is_none_for_uncalibrated_semantics_but_keeps_word_count() {
        let result = page_ocr_confidence(
            crate::plugins::ConfidenceSemantics::Uncalibrated,
            Some(0.93),
            42,
            "sceptre",
        )
        .unwrap();
        assert!(result.score.is_none());
        assert_eq!(result.word_count, 42);
        assert_eq!(result.backend, "sceptre");
    }

    /// `None` semantics (no page-level confidence reported at all) never yield a score.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_is_none_for_none_semantics() {
        let result = page_ocr_confidence(crate::plugins::ConfidenceSemantics::None, Some(0.5), 10, "paddle").unwrap();
        assert!(result.score.is_none());
    }

    /// A missing raw mean (no words to average over) must yield `score: None`, never a
    /// substituted `Some(0.0)` -- zero words and zero confidence are different facts.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_is_none_not_zero_when_raw_confidence_is_missing() {
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 100.0 };
        let result = page_ocr_confidence(semantics, None, 0, "tesseract").unwrap();
        assert!(result.score.is_none());
        assert_ne!(result.score, Some(0.0));
    }

    /// A non-positive `scale_max` must not divide into NaN or infinity; it is treated as
    /// having no calibrated scale, same as `Uncalibrated`.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_is_none_when_scale_max_is_non_positive() {
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 0.0 };
        let result = page_ocr_confidence(semantics, Some(50.0), 30, "tesseract").unwrap();
        assert!(result.score.is_none());
    }

    /// A raw value above `scale_max` is clamped to exactly `1.0`, not left over-range.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_clamps_raw_value_above_scale_max_to_one() {
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 100.0 };
        let result = page_ocr_confidence(semantics, Some(105.0), 5, "tesseract").unwrap();
        assert_eq!(result.score, Some(1.0));
    }

    /// Survivor-bias pin: a tiny `word_count` must not be hidden behind a confident-looking
    /// score. The score is computed exactly as for a large sample, but `word_count` still
    /// reports the true (small) sample size so a caller can discount it accordingly.
    #[cfg(feature = "ocr")]
    #[test]
    fn page_ocr_confidence_preserves_tiny_word_count_alongside_a_high_score() {
        let semantics = crate::plugins::ConfidenceSemantics::Legibility { scale_max: 100.0 };
        let result = page_ocr_confidence(semantics, Some(94.0), 3, "tesseract").unwrap();
        assert_eq!(result.score, Some(0.94));
        assert_eq!(result.word_count, 3);
    }
}
