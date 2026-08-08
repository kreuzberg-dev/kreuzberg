//! ODP (OpenDocument Presentation) extractor.
//!
//! OpenDocument Presentation files share the ODF ZIP + XML container with ODT,
//! but their body is `office:presentation > draw:page` (one page per slide)
//! rather than `office:text`. Slide text lives in
//! `draw:page > draw:frame > draw:text-box`, tables in
//! `draw:page > draw:frame > table:table`, and images in
//! `draw:page > draw:frame > draw:image`.
//!
//! To avoid duplicating parsing logic, this extractor reuses the ODT walker
//! [`build_internal_elements`] for each text box (it already handles
//! `text:p` / `text:h` / `text:list` / nested tables and inline images) and the
//! shared ODF helpers ([`build_style_map`], [`pre_extract_images`],
//! [`pre_extract_formulas`], [`extract_table_cells`]). Only the outer
//! slide → frame traversal and slide markers are ODP-specific.
//!
//! A slide's drawing content can also be a `draw:custom-shape` (and other
//! shape kinds) or a `draw:g` group holding further frames/shapes nested to
//! arbitrary depth — [`process_page_object`] recurses through all of that.
//!
//! Speaker notes (`presentation:notes`, a sibling of the drawing frames inside
//! each `draw:page`) are extracted separately from slide body text: the main
//! traversal walks the page's *direct-child* `draw:frame`/`draw:g` elements
//! only, never `descendants()`, so notes are never folded into slide content;
//! [`extract_odp_notes_text`] handles the `presentation:notes` sibling on its
//! own and the result lands as a distinct raw block.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::office_metadata;
use crate::extractors::odt::{
    build_internal_elements, build_list_style_map, build_style_map, extract_table_cells, pre_extract_formulas,
    pre_extract_images,
};
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
use crate::types::Metadata;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use ahash::AHashMap;
use async_trait::async_trait;
use bytes::Bytes;
use roxmltree::Document;
use std::borrow::Cow;
use std::io::Cursor;

/// Canonical MIME type for OpenDocument Presentation files.
const ODP_MIME: &str = "application/vnd.oasis.opendocument.presentation";

/// ODF drawing namespace, used to resolve `draw:*` attributes.
const DRAWING_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:drawing:1.0";
/// XLink namespace, used to resolve `xlink:href` on images.
const XLINK_NS: &str = "http://www.w3.org/1999/xlink";

/// Native Rust extractor for OpenDocument Presentation (`.odp`) files.
#[cfg_attr(alef, alef(skip))]
pub struct OdpExtractor;

impl OdpExtractor {
    /// Create a new ODP extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for OdpExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for OdpExtractor {
    fn name(&self) -> &str {
        "odp-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Native Rust ODP (OpenDocument Presentation) extractor with slide, table, and image support"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// Parse an ODP `content.xml` into an [`InternalDocument`], emitting one slide
/// marker per `draw:page` followed by that slide's text, tables, and images.
fn build_internal_document(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    budget: &mut SecurityBudget,
) -> crate::error::Result<InternalDocument> {
    let image_data = pre_extract_images(archive);
    let formula_data = pre_extract_formulas(archive, budget)?;

    let mut xml_content = String::new();
    match archive.by_name("content.xml") {
        Ok(mut file) => {
            use std::io::Read;
            file.read_to_string(&mut xml_content)
                .map_err(|e| crate::error::XbergError::parsing(format!("Failed to read content.xml: {}", e)))?;
        }
        Err(_) => {
            // Same defect as ODT's (#112), in the sibling extractor: a ZIP without
            // content.xml is not an ODF document at all, so returning `Ok` with an
            // empty document tells the caller "this is the presentation" for
            // something that isn't one. Fail loudly instead.
            return Err(crate::error::XbergError::parsing(
                "ODP archive is missing content.xml; this is not a valid OpenDocument Presentation file",
            ));
        }
    }

    let doc = Document::parse(&xml_content)
        .map_err(|e| crate::error::XbergError::parsing(format!("Failed to parse content.xml: {}", e)))?;

    let root = doc.root_element();
    let style_map = build_style_map(root);
    let list_style_map = build_list_style_map(root);
    let mut builder = InternalDocumentBuilder::new("odp");

    // Presentations carry no tracked changes; feed the shared ODT walker empty
    // collaboration state so its change-tracking arms are inert. ~keep
    let empty_changes = AHashMap::new();
    let mut revisions = Vec::new();

    let mut slide_number: u32 = 0;
    for body in root.children().filter(|n| n.tag_name().name() == "body") {
        for presentation in body.children().filter(|n| n.tag_name().name() == "presentation") {
            for page in presentation.children().filter(|n| n.tag_name().name() == "page") {
                budget.step()?;
                slide_number += 1;

                let slide_name = page
                    .attribute((DRAWING_NS, "name"))
                    .or_else(|| page.attribute("draw:name"));
                builder.push_slide(slide_number, slide_name, None);

                // Direct-child frames/groups only — never descendants — so the
                // sibling `presentation:notes` element is not pulled into
                // slide text; `process_page_object` recurses into `draw:g`
                // (and nested frames within it) on its own (#116). ~keep
                for page_child in page.children() {
                    if matches!(page_child.tag_name().name(), "frame" | "g") {
                        process_page_object(
                            page_child,
                            &mut builder,
                            &style_map,
                            &list_style_map,
                            &image_data,
                            &formula_data,
                            budget,
                            &empty_changes,
                            &mut revisions,
                        )?;
                    }
                }

                // Speaker notes (`presentation:notes`, #95): a sibling of the
                // slide's drawing frames, deliberately not visited by the loop
                // above. Flattened to a raw block (rather than folded into
                // slide body text) so it stays distinguishable from what's
                // actually shown on the slide.
                if let Some(notes_text) = page
                    .children()
                    .find(|n| n.tag_name().name() == "notes")
                    .and_then(extract_odp_notes_text)
                {
                    builder.push_raw_block("odp-speaker-notes", &notes_text, Some(slide_number));
                }
            }
        }
    }

    extract_odp_master_page_text(archive, &mut builder);

    Ok(builder.build())
}

/// Process one drawing object reached from a slide page: a `draw:frame`
/// (unwrapped to its content), a `draw:g` group (recursed into, arbitrarily
/// nested, #116), or a bare shape/table/image/object.
///
/// Shapes other than `draw:frame > draw:text-box` (custom shapes, rectangles,
/// ellipses, lines, connectors, ...) can carry their own `text:p` content
/// directly and were previously invisible to this walker entirely (#116).
#[allow(clippy::too_many_arguments)]
fn process_page_object(
    node: roxmltree::Node,
    builder: &mut InternalDocumentBuilder,
    style_map: &AHashMap<String, crate::extractors::odt::OdtStyleProps>,
    list_style_map: &AHashMap<String, bool>,
    image_data: &AHashMap<String, (Vec<u8>, String)>,
    formula_data: &AHashMap<String, String>,
    budget: &mut SecurityBudget,
    empty_changes: &AHashMap<String, crate::extractors::odt::OdtChangeRegion>,
    revisions: &mut Vec<crate::types::revisions::DocumentRevision>,
) -> crate::error::Result<()> {
    match node.tag_name().name() {
        "frame" | "g" => {
            for child in node.children() {
                process_page_object(
                    child,
                    builder,
                    style_map,
                    list_style_map,
                    image_data,
                    formula_data,
                    budget,
                    empty_changes,
                    revisions,
                )?;
            }
        }
        "text-box" | "custom-shape" | "rect" | "ellipse" | "circle" | "line" | "polygon" | "polyline" | "path"
        | "connector" | "regular-polygon" | "measure" => {
            build_internal_elements(
                node,
                builder,
                style_map,
                list_style_map,
                image_data,
                formula_data,
                budget,
                empty_changes,
                revisions,
            )?;
        }
        "table" => {
            let cells = extract_table_cells(node);
            if !cells.is_empty() {
                let cell_count: usize = cells.iter().map(|row| row.len()).sum();
                budget.add_cells(cell_count)?;
                builder.push_table_from_cells(&cells, None, None);
            }
        }
        "image" => {
            push_frame_image(node, image_data, builder);
        }
        "object" | "object-ole" => {
            // An embedded object (#116): a MathML formula resolves through
            // the same lookup ODT uses; anything else (e.g. an embedded
            // spreadsheet) cannot be turned into text by this extractor, so
            // its loss must be surfaced rather than silently dropped, per
            // the extractor-warning convention in `core::diagnostics`.
            let href = node
                .attribute((XLINK_NS, "href"))
                .or_else(|| node.attribute("xlink:href"));
            let resolved = href.and_then(|h| formula_data.get(h.trim_start_matches("./")));
            if let Some(formula_text) = resolved {
                builder.push_formula(formula_text, None, None);
            } else {
                let message = match href {
                    Some(h) => format!("Embedded object '{h}' is not a formula and could not be extracted as text"),
                    None => "An embedded object could not be extracted as text".to_string(),
                };
                builder.add_warning(crate::core::diagnostics::warning("odp", message));
            }
        }
        _ => {}
    }
    Ok(())
}

/// Flatten a `presentation:notes` element's slide-like body into plain text
/// for a raw block (#95). Notes share the same `draw:page`-style shape
/// (`draw:frame > draw:text-box > text:p`) as the slide itself.
fn extract_odp_notes_text(notes: roxmltree::Node) -> Option<String> {
    let mut paragraphs = Vec::new();
    for frame in notes.descendants().filter(|n| n.tag_name().name() == "frame") {
        for text_box in frame.children().filter(|n| n.tag_name().name() == "text-box") {
            for p in text_box.children().filter(|n| matches!(n.tag_name().name(), "p" | "h")) {
                if let Some(text) = crate::extractors::odt::extract_node_text(p) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        paragraphs.push(trimmed.to_string());
                    }
                }
            }
        }
    }
    if paragraphs.is_empty() {
        None
    } else {
        Some(paragraphs.join("\n"))
    }
}

/// Extract any static text from `styles.xml`'s `style:master-page` elements
/// (#116) — slide masters commonly carry footer/placeholder text applied to
/// every slide that uses them, which was previously never visited by this
/// extractor.
fn extract_odp_master_page_text(archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>, builder: &mut InternalDocumentBuilder) {
    use std::io::Read;

    let mut styles_xml = String::new();
    let Ok(mut file) = archive.by_name("styles.xml") else {
        return;
    };
    if file.read_to_string(&mut styles_xml).is_err() {
        return;
    }
    let Ok(doc) = Document::parse(&styles_xml) else {
        return;
    };

    for master_page in doc
        .root_element()
        .descendants()
        .filter(|n| n.tag_name().name() == "master-page")
    {
        let mut paragraphs = Vec::new();
        for p in master_page
            .descendants()
            .filter(|n| matches!(n.tag_name().name(), "p" | "h"))
        {
            if let Some(text) = crate::extractors::odt::extract_node_text(p) {
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    paragraphs.push(trimmed.to_string());
                }
            }
        }
        if !paragraphs.is_empty() {
            builder.push_raw_block("odp-master-page", &paragraphs.join("\n"), None);
        }
    }
}

/// Resolve a `draw:image`'s referenced bytes from the pre-extracted image map
/// and push it onto the builder. Unresolvable references are skipped silently
/// (the ODT walker handles inline images; only page-level frame images land
/// here).
fn push_frame_image(
    image_node: roxmltree::Node,
    image_data: &AHashMap<String, (Vec<u8>, String)>,
    builder: &mut InternalDocumentBuilder,
) {
    let href = image_node
        .attribute((XLINK_NS, "href"))
        .or_else(|| image_node.attribute("xlink:href"));
    let Some(href) = href else { return };
    let Some((data, format)) = image_data.get(href).cloned() else {
        return;
    };

    let (image_kind, kind_confidence) =
        crate::extraction::image_kind::classify(&data, &format, None, None, None, None, false);

    let image = ExtractedImage {
        data: Bytes::from(data),
        format: Cow::Owned(format),
        image_kind: Some(image_kind),
        kind_confidence: Some(kind_confidence),
        ..Default::default()
    };
    let idx = builder.push_image(None, image, None, None);
    let mut attrs = AHashMap::with_capacity(1);
    attrs.insert("src".to_string(), href.to_string());
    builder.set_attributes(idx, attrs);
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for OdpExtractor {
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, content, config),
            fields(
                extractor.name = self.name(),
                content.size_bytes = content.len(),
            )
        )
    )]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "odp", size_bytes = content.len(), "extraction starting");
        let content_owned = content.to_vec();

        let cursor = Cursor::new(content_owned.clone());
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| crate::error::XbergError::parsing(format!("Failed to open ZIP archive: {}", e)))?;

        let mut budget = SecurityBudget::from_config(config);
        let mut doc = build_internal_document(&mut archive, &mut budget)?;
        doc.mime_type = mime_type.to_string();

        let mut metadata_map = AHashMap::new();

        let meta_cursor = Cursor::new(content_owned);
        let mut meta_archive = zip::ZipArchive::new(meta_cursor).map_err(|e| {
            crate::error::XbergError::parsing(format!("Failed to open ZIP archive for metadata: {}", e))
        })?;

        // ODP `meta.xml` uses the same ODF metadata schema as ODT. ~keep
        if let Ok(props) = office_metadata::extract_odt_properties(&mut meta_archive) {
            if let Some(title) = props.title {
                metadata_map.insert(Cow::Borrowed("title"), serde_json::Value::String(title));
            }
            if let Some(creator) = props.creator {
                metadata_map.insert(
                    Cow::Borrowed("authors"),
                    serde_json::Value::Array(vec![serde_json::Value::String(creator.clone())]),
                );
                metadata_map.insert(Cow::Borrowed("created_by"), serde_json::Value::String(creator));
            }
            if let Some(initial_creator) = props.initial_creator {
                metadata_map.insert(
                    Cow::Borrowed("initial_creator"),
                    serde_json::Value::String(initial_creator),
                );
            }
            if let Some(subject) = props.subject {
                metadata_map.insert(Cow::Borrowed("subject"), serde_json::Value::String(subject));
            }
            if let Some(keywords) = props.keywords {
                metadata_map.insert(Cow::Borrowed("keywords"), serde_json::Value::String(keywords));
            }
            if let Some(description) = props.description {
                metadata_map.insert(Cow::Borrowed("description"), serde_json::Value::String(description));
            }
            if let Some(creation_date) = props.creation_date {
                metadata_map.insert(Cow::Borrowed("created_at"), serde_json::Value::String(creation_date));
            }
            if let Some(date) = props.date {
                metadata_map.insert(Cow::Borrowed("modified_at"), serde_json::Value::String(date));
            }
            if let Some(language) = props.language {
                metadata_map.insert(Cow::Borrowed("language"), serde_json::Value::String(language));
            }
            if let Some(generator) = props.generator {
                metadata_map.insert(Cow::Borrowed("generator"), serde_json::Value::String(generator));
            }
            if let Some(editing_duration) = props.editing_duration {
                metadata_map.insert(
                    Cow::Borrowed("editing_duration"),
                    serde_json::Value::String(editing_duration),
                );
            }
            if let Some(editing_cycles) = props.editing_cycles {
                metadata_map.insert(
                    Cow::Borrowed("editing_cycles"),
                    serde_json::Value::String(editing_cycles),
                );
            }
            if let Some(page_count) = props.page_count {
                metadata_map.insert(
                    Cow::Borrowed("page_count"),
                    serde_json::Value::Number(page_count.into()),
                );
            }
            if let Some(word_count) = props.word_count {
                metadata_map.insert(
                    Cow::Borrowed("word_count"),
                    serde_json::Value::Number(word_count.into()),
                );
            }
            if let Some(character_count) = props.character_count {
                metadata_map.insert(
                    Cow::Borrowed("character_count"),
                    serde_json::Value::Number(character_count.into()),
                );
            }
            if let Some(paragraph_count) = props.paragraph_count {
                metadata_map.insert(
                    Cow::Borrowed("paragraph_count"),
                    serde_json::Value::Number(paragraph_count.into()),
                );
            }
            if let Some(table_count) = props.table_count {
                metadata_map.insert(
                    Cow::Borrowed("table_count"),
                    serde_json::Value::Number(table_count.into()),
                );
            }
            if let Some(image_count) = props.image_count {
                metadata_map.insert(
                    Cow::Borrowed("image_count"),
                    serde_json::Value::Number(image_count.into()),
                );
            }
        }

        let title = metadata_map
            .remove(&Cow::Borrowed("title"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let subject = metadata_map
            .remove(&Cow::Borrowed("subject"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let authors = metadata_map.remove(&Cow::Borrowed("authors")).and_then(|v| {
            v.as_array()
                .map(|arr| arr.iter().filter_map(|v| v.as_str().map(|s| s.to_string())).collect())
        });
        let created_by = metadata_map
            .remove(&Cow::Borrowed("created_by"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let created_at = metadata_map
            .remove(&Cow::Borrowed("created_at"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let modified_at = metadata_map
            .remove(&Cow::Borrowed("modified_at"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let language = metadata_map
            .remove(&Cow::Borrowed("language"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        let keywords = metadata_map.remove(&Cow::Borrowed("keywords")).and_then(|v| {
            v.as_str().map(|s| {
                s.split(',')
                    .map(|k| k.trim().to_string())
                    .filter(|k| !k.is_empty())
                    .collect()
            })
        });

        doc.metadata = Metadata {
            title,
            subject,
            authors,
            keywords,
            language,
            created_at,
            modified_at,
            created_by,
            additional: metadata_map,
            ..Default::default()
        };

        if let Some(ref filter) = config.content_filter {
            use crate::types::document_structure::ContentLayer;
            doc.elements.retain(|elem| match elem.layer {
                ContentLayer::Header => filter.include_headers,
                ContentLayer::Footer => filter.include_footers,
                _ => true,
            });
        }

        tracing::debug!(
            element_count = doc.elements.len(),
            format = "odp",
            "extraction complete"
        );
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[ODP_MIME]
    }

    fn priority(&self) -> i32 {
        60
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal::ElementKind;
    use std::io::Write;

    /// Wrap a presentation body fragment into a valid in-memory `.odp` ZIP for
    /// deterministic, network-free extraction tests. `body_inner` is placed
    /// inside `<office:presentation>`.
    fn odp_bytes(body_inner: &str) -> Vec<u8> {
        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <office:body>
    <office:presentation>{body_inner}</office:presentation>
  </office:body>
</office:document-content>"#
        );

        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            // The `mimetype` entry must be first and stored uncompressed per the ODF spec. ~keep
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(ODP_MIME.as_bytes()).unwrap();

            let deflated =
                zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("content.xml", deflated).unwrap();
            zip.write_all(content_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }
        buf
    }

    /// A minimal single-slide deck with one text box.
    fn minimal_odp() -> Vec<u8> {
        odp_bytes(
            r#"<draw:page draw:name="Intro"><draw:frame><draw:text-box>
                 <text:p>Hello Slide</text:p>
               </draw:text-box></draw:frame></draw:page>"#,
        )
    }

    #[tokio::test]
    async fn test_odp_extracts_slide_text() {
        let bytes = minimal_odp();
        let extractor = OdpExtractor::new();
        let doc = extractor
            .extract_content(&bytes, ODP_MIME, &ExtractionConfig::default())
            .await
            .expect("ODP extraction should succeed");

        let has_text = doc.elements.iter().any(|e| e.text.contains("Hello Slide"));
        assert!(has_text, "extracted content should contain the slide's text");

        let slide_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, ElementKind::Slide { .. }))
            .count();
        assert_eq!(slide_count, 1, "one draw:page should yield one slide marker");
    }

    #[tokio::test]
    async fn test_odp_extracts_table_cells() {
        // A slide whose frame holds a table (draw:frame > table:table), the
        // real-world nesting — a sibling of text boxes, not inside one. ~keep
        let bytes = odp_bytes(
            r#"<draw:page draw:name="Data"><draw:frame><table:table>
                 <table:table-row>
                   <table:table-cell><text:p>A1</text:p></table:table-cell>
                   <table:table-cell><text:p>B1</text:p></table:table-cell>
                 </table:table-row>
               </table:table></draw:frame></draw:page>"#,
        );
        let doc = OdpExtractor::new()
            .extract_content(&bytes, ODP_MIME, &ExtractionConfig::default())
            .await
            .expect("ODP extraction should succeed");

        assert!(
            doc.elements.iter().any(|e| matches!(e.kind, ElementKind::Table { .. })),
            "the table frame should produce a Table element"
        );
        assert!(
            doc.tables.iter().any(|t| t.cells.iter().flatten().any(|c| c == "A1"))
                && doc.tables.iter().any(|t| t.cells.iter().flatten().any(|c| c == "B1")),
            "table cell text A1/B1 should be extracted"
        );
    }

    /// Confirms that ODP inherits the ODT formula (MathML→LaTeX) pipeline for
    /// free, since it reuses `pre_extract_formulas`/`build_internal_elements`.
    #[tokio::test]
    async fn test_odp_extracts_embedded_formula_as_latex() {
        let content_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
    xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
    xmlns:xlink="http://www.w3.org/1999/xlink">
  <office:body>
    <office:presentation><draw:page draw:name="Formula"><draw:frame><draw:text-box>
      <text:p><draw:frame><draw:object xlink:href="./Object 1"/></draw:frame></text:p>
    </draw:text-box></draw:frame></draw:page></office:presentation>
  </office:body>
</office:document-content>"#;
        let formula_content_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<math xmlns="http://www.w3.org/1998/Math/MathML">
  <msup><mi>x</mi><mn>2</mn></msup>
</math>"#;

        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(ODP_MIME.as_bytes()).unwrap();

            let deflated =
                zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("content.xml", deflated).unwrap();
            zip.write_all(content_xml.as_bytes()).unwrap();
            zip.start_file("Object 1/content.xml", deflated).unwrap();
            zip.write_all(formula_content_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let doc = OdpExtractor::new()
            .extract_content(&buf, ODP_MIME, &ExtractionConfig::default())
            .await
            .expect("ODP extraction should succeed");

        let formula = doc
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::Formula))
            .expect("expected a Formula element");
        assert_eq!(formula.text, "x^{2}");
    }

    #[tokio::test]
    async fn test_odp_extractor_supports_odp_mime() {
        let extractor = OdpExtractor::new();
        assert!(extractor.supported_mime_types().contains(&ODP_MIME));
    }

    #[tokio::test]
    async fn test_odp_extractor_plugin_interface() {
        let extractor = OdpExtractor::new();
        assert_eq!(extractor.name(), "odp-extractor");
        assert_eq!(extractor.priority(), 60);
        extractor.initialize().expect("initialize should succeed");
        extractor.shutdown().expect("shutdown should succeed");
    }

    #[tokio::test]
    async fn test_odp_extractor_default() {
        let a = OdpExtractor::new();
        let b = OdpExtractor;
        assert_eq!(a.name(), b.name());
    }

    /// Fixture-backed smoke test over the real `.odp` files in
    /// `test_documents/odp/`. Skips cleanly when the fixtures are absent (the
    /// directory is a git submodule that may not be checked out).
    #[tokio::test]
    async fn test_odp_real_fixtures() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents/odp");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return;
        };

        let extractor = OdpExtractor::new();
        let mut files = 0usize;
        let mut any_text = false;
        let mut any_slide = false;
        let mut any_table = false;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("odp") {
                continue;
            }
            files += 1;
            let bytes = std::fs::read(&path).expect("failed to read fixture");
            let doc = extractor
                .extract_content(&bytes, ODP_MIME, &ExtractionConfig::default())
                .await
                .unwrap_or_else(|e| panic!("extraction failed for {}: {e}", path.display()));

            if doc.elements.iter().any(|e| !e.text.trim().is_empty()) {
                any_text = true;
            }
            if doc.elements.iter().any(|e| matches!(e.kind, ElementKind::Slide { .. })) {
                any_slide = true;
            }
            if doc.elements.iter().any(|e| matches!(e.kind, ElementKind::Table { .. })) {
                any_table = true;
            }
        }

        if files == 0 {
            return;
        }
        assert!(any_text, "at least one .odp fixture should yield non-empty text");
        assert!(any_slide, "at least one .odp fixture should yield a slide marker");
        assert!(
            any_table,
            "at least one .odp fixture (with_table.odp) should yield a table"
        );
    }
}
