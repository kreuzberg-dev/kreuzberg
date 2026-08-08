//! Regression test for defect #54.
//!
//! The renderer registry handed foreign/public renderers an `ExtractedDocument` built by
//! `ExtractedDocument::from(doc)`, which runs the derivation with
//! `include_document_structure = false` and never populates `elements`. A public renderer
//! plugin therefore received `document: None` and `elements: None` and was structurally
//! unable to do layout-aware rendering — it only ever saw a flat content string.

use std::sync::Arc;

use xberg::Result;
use xberg::core::config::OutputFormat;
use xberg::extraction::derive::derive_extraction_result;
use xberg::plugins::{Plugin, Renderer, register_renderer};
use xberg::types::ExtractedDocument;
use xberg::types::internal::{ElementKind, InternalDocument, InternalElement};

const RENDERER_NAME: &str = "issue-54-structure-probe";

/// Public renderer that reports the layout signals it was handed.
struct StructureProbe;

impl Plugin for StructureProbe {
    fn name(&self) -> &str {
        RENDERER_NAME
    }
}

impl Renderer for StructureProbe {
    fn render_result(&self, result: &ExtractedDocument) -> Result<String> {
        Ok(format!(
            "document={} elements={} content={}",
            result.document.is_some(),
            result.elements.as_ref().map_or(0, Vec::len),
            result.content,
        ))
    }
}

fn probe_document() -> InternalDocument {
    let mut doc = InternalDocument::new("text/plain");
    doc.push_element(InternalElement::text(ElementKind::Heading { level: 1 }, "Title", 0));
    doc.push_element(InternalElement::text(ElementKind::Paragraph, "Body text", 0));
    doc
}

#[test]
fn should_hand_public_renderers_a_document_structure_and_element_list() {
    register_renderer(Arc::new(StructureProbe)).expect("renderer registration must succeed");

    let result = derive_extraction_result(probe_document(), false, OutputFormat::Custom(RENDERER_NAME.to_string()));

    assert_eq!(
        result.formatted_content.as_deref(),
        Some("document=true elements=2 content=Title\nBody text")
    );
}
