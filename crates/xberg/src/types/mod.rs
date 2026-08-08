//! Core types for document extraction.

pub mod annotations;
pub mod builder;
pub mod classification;
pub mod diagram;
pub mod djot;
pub mod document_structure;
pub mod entity;
pub mod extraction;
pub mod form_field;
pub mod formats;
pub mod formula;
pub mod internal;
pub mod internal_builder;
pub mod metadata;
pub mod ocr_elements;
pub mod page;
pub mod qr;
pub mod redaction;
pub mod revisions;
pub mod serde_helpers;
pub mod summary;
pub mod tables;
pub mod translation;
pub mod uri;

pub use annotations::*;
pub use classification::{ClassificationLabel, PageClassification};
pub use djot::*;
pub use document_structure::{
    AnnotationKind, ContentLayer, DocumentNode, DocumentRelationship, DocumentStructure, GridCell, NodeContent, NodeId,
    NodeIndex, RelationshipKind, TableGrid, TextAnnotation,
};
pub use entity::{Entity, EntityCategory};
pub use extraction::*;
pub use form_field::{FormFieldType, PdfFormField};
pub use formats::*;
pub use formula::Formula;
pub use metadata::*;
pub use ocr_elements::*;
pub use page::*;
pub use qr::{QrBoundingBox, QrCode};
pub use redaction::{PiiCategory, RedactionFinding, RedactionReport, RedactionStrategy};
pub use revisions::{
    CellChange, DiffLine, DocumentRevision, PropertyChange, RevisionAnchor, RevisionDelta, RevisionKind,
};
pub use summary::{DocumentSummary, SummaryStrategy};
pub use tables::*;
pub use translation::Translation;
pub use uri::*;

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;
    use std::borrow::Cow;
    use std::sync::Arc;

    #[test]
    fn test_metadata_serialization_with_format() {
        let mut metadata = Metadata {
            format: Some(FormatMetadata::Text(TextMetadata {
                line_count: 1,
                word_count: 2,
                character_count: 13,
                headers: None,
                links: None,
                code_blocks: None,
            })),
            ..Default::default()
        };

        metadata
            .additional
            .insert(Cow::Borrowed("quality_score"), serde_json::json!(1.0));

        let json = serde_json::to_value(&metadata).unwrap();
        println!("Serialized metadata: {}", serde_json::to_string_pretty(&json).unwrap());

        let format = json.get("format").expect("format field should be present");
        assert_eq!(format.get("format_type").unwrap(), "text");
        assert_eq!(format.get("line_count").unwrap(), 1);
        assert_eq!(format.get("word_count").unwrap(), 2);
        assert_eq!(format.get("character_count").unwrap(), 13);

        let additional = json.get("additional").expect("additional field should be present");
        assert_eq!(additional.get("quality_score").unwrap(), 1.0);
    }

    #[test]
    fn test_arc_table_serialization_format() {
        let table = Table {
            cells: vec![vec!["A".to_string(), "B".to_string()]],
            markdown: "| A | B |\n|---|---|\n".to_string(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        };

        let json = serde_json::to_value(&table).unwrap();

        assert_eq!(json.get("cells").unwrap()[0][0], "A");
        assert_eq!(json.get("markdown").unwrap(), "| A | B |\n|---|---|\n");
        assert_eq!(json.get("page_number").unwrap(), 1);
    }

    #[test]
    fn test_arc_table_roundtrip() {
        let original = Table {
            cells: vec![
                vec!["X".to_string(), "Y".to_string()],
                vec!["1".to_string(), "2".to_string()],
            ],
            markdown: "| X | Y |\n|---|---|\n| 1 | 2 |\n".to_string(),
            page_number: 5,
            bounding_box: None,
            ..Default::default()
        };

        let json = serde_json::to_string(&original).unwrap();
        let deserialized: Table = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.cells, original.cells);
        assert_eq!(deserialized.markdown, original.markdown);
        assert_eq!(deserialized.page_number, original.page_number);
    }

    #[test]
    fn test_arc_sharing_preserved_before_serialization() {
        let shared_table = Arc::new(Table {
            cells: vec![vec!["shared".to_string()]],
            markdown: "| shared |".to_string(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        });

        let tables_before = [Arc::clone(&shared_table), Arc::clone(&shared_table)].to_vec();
        assert_eq!(Arc::strong_count(&tables_before[0]), 3);
        assert_eq!(Arc::strong_count(&tables_before[1]), 3);
        assert!(Arc::ptr_eq(&tables_before[0], &tables_before[1]));
    }

    #[test]
    fn test_vec_arc_table_serialization_format() {
        let tables = vec![
            Table {
                cells: vec![vec!["A".to_string()]],
                markdown: "| A |".to_string(),
                page_number: 1,
                bounding_box: None,
                ..Default::default()
            },
            Table {
                cells: vec![vec!["B".to_string()]],
                markdown: "| B |".to_string(),
                page_number: 2,
                bounding_box: None,
                ..Default::default()
            },
        ];

        let json = serde_json::to_string(&tables).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();

        assert!(parsed.is_array());
        assert_eq!(parsed.as_array().unwrap().len(), 2);
        assert_eq!(parsed[0]["cells"][0][0], "A");
        assert_eq!(parsed[1]["cells"][0][0], "B");
    }

    #[test]
    fn test_page_content_arc_tables_roundtrip() {
        let page = PageContent {
            page_number: 3,
            content: "Page 3 content".to_string(),
            tables: vec![
                Arc::new(Table {
                    cells: vec![vec!["Table1".to_string()]],
                    markdown: "| Table1 |".to_string(),
                    page_number: 3,
                    bounding_box: None,
                    ..Default::default()
                }),
                Arc::new(Table {
                    cells: vec![vec!["Table2".to_string()]],
                    markdown: "| Table2 |".to_string(),
                    page_number: 3,
                    bounding_box: None,
                    ..Default::default()
                }),
            ],
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        };

        let json = serde_json::to_string(&page).unwrap();
        let deserialized: PageContent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.page_number, 3);
        assert_eq!(deserialized.content, "Page 3 content");
        assert_eq!(deserialized.tables.len(), 2);
        assert_eq!(deserialized.tables[0].cells[0][0], "Table1");
        assert_eq!(deserialized.tables[1].cells[0][0], "Table2");
    }

    #[test]
    fn test_page_content_image_indices_roundtrip() {
        let page = PageContent {
            page_number: 1,
            content: "Page with images".to_string(),
            tables: Vec::new(),
            image_indices: vec![0, 1],
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        };

        let json = serde_json::to_string(&page).unwrap();
        let deserialized: PageContent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.image_indices, vec![0, 1]);

        let v: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(v["image_indices"][0], 0);
        assert_eq!(v["image_indices"][1], 1);
    }

    #[test]
    fn test_arc_sharing_loss_with_page_content() {
        let shared_table = Arc::new(Table {
            cells: vec![vec!["shared across pages".to_string()]],
            markdown: "| shared across pages |".to_string(),
            page_number: 0,
            bounding_box: None,
            ..Default::default()
        });

        let page1 = PageContent {
            page_number: 1,
            content: "Page 1".to_string(),
            tables: vec![Arc::clone(&shared_table)],
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        };

        let page2 = PageContent {
            page_number: 2,
            content: "Page 2".to_string(),
            tables: vec![Arc::clone(&shared_table)],
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        };

        assert!(Arc::ptr_eq(&page1.tables[0], &page2.tables[0]));

        let pages = vec![page1, page2];
        let json = serde_json::to_string(&pages).unwrap();
        let deserialized: Vec<PageContent> = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.len(), 2);
        assert_eq!(deserialized[0].tables[0].cells, deserialized[1].tables[0].cells);
        assert!(!Arc::ptr_eq(&deserialized[0].tables[0], &deserialized[1].tables[0]));
    }

    #[test]
    fn test_empty_page_content_arcs() {
        let page = PageContent {
            page_number: 5,
            content: "No tables or images".to_string(),
            tables: Vec::new(),
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        };

        let json = serde_json::to_string(&page).unwrap();
        let deserialized: PageContent = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.page_number, 5);
        assert_eq!(deserialized.tables.len(), 0);
        assert_eq!(deserialized.image_indices.len(), 0);
    }

    #[test]
    fn test_serde_vec_arc_module_behavior() {
        let table1 = Table {
            cells: vec![vec!["A".to_string()]],
            markdown: "| A |".to_string(),
            page_number: 1,
            bounding_box: None,
            ..Default::default()
        };

        let table2 = Table {
            cells: vec![vec!["B".to_string()]],
            markdown: "| B |".to_string(),
            page_number: 2,
            bounding_box: None,
            ..Default::default()
        };

        let json = serde_json::to_string(&vec![table1, table2]).unwrap();
        assert!(json.contains("\"A\""));
        assert!(json.contains("\"B\""));
    }

    #[test]
    fn test_extracted_image_with_bounding_box_serialization() {
        let image = ExtractedImage {
            data: Bytes::from_static(&[0xFF, 0xD8]),
            format: Cow::Borrowed("jpeg"),
            image_index: 0,
            page_number: Some(1),
            width: Some(640),
            height: Some(480),
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: None,
            bounding_box: Some(BoundingBox {
                x0: 72.0,
                y0: 200.0,
                x1: 540.0,
                y1: 600.0,
            }),
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        };

        let json = serde_json::to_string(&image).unwrap();
        assert!(json.contains("\"bounding_box\""));
        assert!(json.contains("\"x0\":72.0"));
        assert!(json.contains("\"y0\":200.0"));
        assert!(json.contains("\"x1\":540.0"));
        assert!(json.contains("\"y1\":600.0"));

        let deserialized: ExtractedImage = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.image_index, 0);
        assert!(deserialized.bounding_box.is_some());
        let bbox = deserialized.bounding_box.unwrap();
        assert_eq!(bbox.x0, 72.0);
        assert_eq!(bbox.y0, 200.0);
        assert_eq!(bbox.x1, 540.0);
        assert_eq!(bbox.y1, 600.0);
    }

    #[test]
    fn test_extracted_image_without_bounding_box_omitted_in_json() {
        let image = ExtractedImage {
            data: Bytes::from_static(&[0x89, 0x50]),
            format: Cow::Borrowed("png"),
            image_index: 0,
            page_number: None,
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
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

        let json = serde_json::to_string(&image).unwrap();
        assert!(!json.contains("bounding_box"));
    }

    #[test]
    fn test_extracted_image_bounding_box_backward_compatibility() {
        let json = r#"{
            "data": "Rm9v",
            "format": "png",
            "image_index": 5,
            "page_number": 2,
            "width": 100,
            "height": 200,
            "colorspace": null,
            "bits_per_component": null,
            "is_mask": false,
            "description": null,
            "ocr_result": null
        }"#;

        let image: ExtractedImage = serde_json::from_str(json).unwrap();
        assert_eq!(image.image_index, 5);
        assert_eq!(image.page_number, Some(2));
        assert!(image.bounding_box.is_none());
    }

    #[test]
    fn test_extracted_image_with_bounding_box_all_fields() {
        let image = ExtractedImage {
            data: Bytes::from_static(&[0xFF, 0xD8, 0xFF, 0xE0]),
            format: Cow::Borrowed("jpeg"),
            image_index: 3,
            page_number: Some(5),
            width: Some(1920),
            height: Some(1080),
            colorspace: Some("RGB".to_string()),
            bits_per_component: Some(8),
            is_mask: false,
            description: Some("Test image".to_string()),
            ocr_result: None,
            bounding_box: Some(BoundingBox {
                x0: 100.0,
                y0: 150.0,
                x1: 800.0,
                y1: 950.0,
            }),
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        };

        let json_value = serde_json::to_value(&image).unwrap();
        let deserialized: ExtractedImage = serde_json::from_value(json_value).unwrap();

        assert_eq!(deserialized.image_index, 3);
        assert_eq!(deserialized.page_number, Some(5));
        assert_eq!(deserialized.width, Some(1920));
        assert_eq!(deserialized.height, Some(1080));
        assert_eq!(deserialized.colorspace, Some("RGB".to_string()));
        assert_eq!(deserialized.bits_per_component, Some(8));
        assert_eq!(deserialized.description, Some("Test image".to_string()));
        assert!(deserialized.bounding_box.is_some());
        let bbox = deserialized.bounding_box.unwrap();
        assert_eq!(bbox.x0, 100.0);
        assert_eq!(bbox.y0, 150.0);
        assert_eq!(bbox.x1, 800.0);
        assert_eq!(bbox.y1, 950.0);
    }

    #[test]
    fn test_extracted_image_bounding_box_clone() {
        let image = ExtractedImage {
            data: Bytes::from_static(&[0xFF, 0xD8]),
            format: Cow::Borrowed("jpeg"),
            image_index: 1,
            page_number: Some(1),
            width: Some(640),
            height: Some(480),
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: None,
            ocr_result: None,
            bounding_box: Some(BoundingBox {
                x0: 10.0,
                y0: 20.0,
                x1: 100.0,
                y1: 200.0,
            }),
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        };

        let cloned = image.clone();
        assert_eq!(cloned.image_index, image.image_index);
        assert_eq!(cloned.bounding_box, image.bounding_box);

        let debug = format!("{:?}", image);
        assert!(debug.contains("bounding_box"));
    }

    #[test]
    fn extracted_image_data_base64_absent_by_default() {
        let image = ExtractedImage::default();
        let json = serde_json::to_string(&image).unwrap();
        assert!(
            !json.contains("data_base64"),
            "data_base64 must not appear when None: {json}"
        );
    }

    #[test]
    fn extracted_image_data_base64_round_trips() {
        use base64::Engine as _;
        let raw = b"hello image";
        let b64 = base64::engine::general_purpose::STANDARD.encode(raw);
        let image = ExtractedImage {
            data: bytes::Bytes::from_static(raw),
            data_base64: Some(b64.clone()),
            ..Default::default()
        };
        let json = serde_json::to_string(&image).unwrap();
        assert!(json.contains("data_base64"), "data_base64 must appear when Some");
        let back: ExtractedImage = serde_json::from_str(&json).unwrap();
        assert_eq!(back.data_base64.as_deref(), Some(b64.as_str()));
    }

    #[test]
    fn extracted_image_old_json_without_data_base64_deserializes_cleanly() {
        let json = r#"{"data":[],"format":"png","image_index":0,"is_mask":false}"#;
        let image: ExtractedImage = serde_json::from_str(json).unwrap();
        assert!(
            image.data_base64.is_none(),
            "absent data_base64 must deserialize to None"
        );
    }
}
