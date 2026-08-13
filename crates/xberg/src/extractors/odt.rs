//! ODT (OpenDocument Text) extractor using native Rust parsing.
//!
//! Supports: OpenDocument Text (.odt)

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::office_metadata;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ExtractedImage;
use crate::types::Metadata;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::revisions::{DiffLine, DocumentRevision, RevisionAnchor, RevisionDelta, RevisionKind};
use crate::types::uri::{ExtractedUri, UriKind};
use ahash::AHashMap;
use async_trait::async_trait;
use bytes::Bytes;
use roxmltree::Document;
use std::borrow::Cow;
use std::io::Cursor;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const ODT_WARNING_SOURCE: &str = "odt";

/// High-performance ODT extractor using native Rust XML parsing.
///
/// This extractor provides:
/// - Fast text extraction via roxmltree XML parsing
/// - Comprehensive metadata extraction from meta.xml
/// - Table extraction with row and cell support
/// - Formatting preservation (bold, italic, strikeout)
/// - Support for headings, paragraphs, and special elements
#[cfg_attr(alef, alef(skip))]
pub struct OdtExtractor;

impl OdtExtractor {
    /// Create a new ODT extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for OdtExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for OdtExtractor {
    fn name(&self) -> &str {
        "odt-extractor"
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
        "Native Rust ODT (OpenDocument Text) extractor with metadata and table support"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

/// Resolved formatting properties for a text style.
#[derive(Default, Clone)]
pub(crate) struct OdtStyleProps {
    bold: bool,
    italic: bool,
    underline: bool,
    strikethrough: bool,
    color: Option<String>,
    font_size: Option<String>,
}

/// Build a map from style-name to resolved formatting properties.
///
/// Parses `<style:style>` elements from the `office:automatic-styles` section
/// of content.xml and resolves `style:text-properties` attributes.
pub(crate) fn build_style_map(root: roxmltree::Node) -> AHashMap<String, OdtStyleProps> {
    let mut styles = AHashMap::new();
    for child in root.children() {
        if child.tag_name().name() == "automatic-styles" || child.tag_name().name() == "styles" {
            for style_node in child.children() {
                if style_node.tag_name().name() == "style"
                    && let Some(name) = style_node
                        .attribute(("urn:oasis:names:tc:opendocument:xmlns:style:1.0", "name"))
                        .or_else(|| style_node.attribute("style:name"))
                {
                    let mut props = OdtStyleProps::default();
                    for prop_child in style_node.children() {
                        if prop_child.tag_name().name() == "text-properties" {
                            if let Some(fw) = prop_child
                                .attribute((
                                    "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0",
                                    "font-weight",
                                ))
                                .or_else(|| prop_child.attribute("fo:font-weight"))
                            {
                                props.bold = fw == "bold";
                            }
                            if let Some(fs) = prop_child
                                .attribute((
                                    "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0",
                                    "font-style",
                                ))
                                .or_else(|| prop_child.attribute("fo:font-style"))
                            {
                                props.italic = fs == "italic";
                            }
                            if let Some(ul) = prop_child
                                .attribute((
                                    "urn:oasis:names:tc:opendocument:xmlns:style:1.0",
                                    "text-underline-style",
                                ))
                                .or_else(|| prop_child.attribute("style:text-underline-style"))
                            {
                                props.underline = ul != "none";
                            }
                            if let Some(st) = prop_child
                                .attribute((
                                    "urn:oasis:names:tc:opendocument:xmlns:style:1.0",
                                    "text-line-through-style",
                                ))
                                .or_else(|| prop_child.attribute("style:text-line-through-style"))
                            {
                                props.strikethrough = st != "none";
                            }
                            if let Some(color) = prop_child
                                .attribute(("urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0", "color"))
                                .or_else(|| prop_child.attribute("fo:color"))
                                && color != "#000000"
                            {
                                props.color = Some(color.to_string());
                            }
                            if let Some(size) = prop_child
                                .attribute((
                                    "urn:oasis:names:tc:opendocument:xmlns:xsl-fo-compatible:1.0",
                                    "font-size",
                                ))
                                .or_else(|| prop_child.attribute("fo:font-size"))
                            {
                                props.font_size = Some(size.to_string());
                            }
                        }
                    }
                    styles.insert(name.to_string(), props);
                }
            }
        }
    }
    styles
}

/// Build a map from `text:list-style` name to whether that style is ordered
/// (numbered) or unordered (bulleted).
///
/// A list style is ordered if any of its level definitions is a
/// `text:list-level-style-number`; a style containing only
/// `text:list-level-style-bullet` levels is unordered. Scans
/// `office:automatic-styles` and `office:styles`, mirroring
/// [`build_style_map`], since a list style directly applied to a `text:list`
/// commonly lives in content.xml's automatic-styles while a shared/named one
/// lives in styles.xml's `office:styles` (#104: previously every ODT list was
/// hardcoded unordered regardless of its actual style).
pub(crate) fn build_list_style_map(root: roxmltree::Node) -> AHashMap<String, bool> {
    let mut styles = AHashMap::new();
    for child in root.children() {
        if child.tag_name().name() == "automatic-styles" || child.tag_name().name() == "styles" {
            for style_node in child.children() {
                if style_node.tag_name().name() == "list-style"
                    && let Some(name) = style_node
                        .attribute(("urn:oasis:names:tc:opendocument:xmlns:style:1.0", "name"))
                        .or_else(|| style_node.attribute("style:name"))
                {
                    let ordered = style_node
                        .children()
                        .any(|level| level.tag_name().name() == "list-level-style-number");
                    styles.insert(name.to_string(), ordered);
                }
            }
        }
    }
    styles
}

/// Parsed metadata for a single `<text:changed-region>` from `<text:tracked-changes>`.
pub(crate) struct OdtChangeRegion {
    author: Option<String>,
    timestamp: Option<String>,
    kind: RevisionKind,
    /// Text content for insertion/deletion regions; empty for format changes.
    content_text: String,
}

/// Parse every `<text:changed-region>` element inside a `<text:tracked-changes>`
/// node and return a map from `text:id` → `OdtChangeRegion`.
///
/// The function looks for `<text:tracked-changes>` as a direct child of `text_node`
/// (the `<office:text>` element). If no tracked-changes element is present the map
/// is empty.
fn parse_odt_tracked_changes(text_node: roxmltree::Node) -> AHashMap<String, OdtChangeRegion> {
    let mut map = AHashMap::new();

    let tracked_changes = text_node.children().find(|n| n.tag_name().name() == "tracked-changes");
    let Some(tc) = tracked_changes else {
        return map;
    };

    for region in tc.children() {
        if region.tag_name().name() != "changed-region" {
            continue;
        }

        let id = region
            .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "id"))
            .or_else(|| region.attribute("text:id"));
        let Some(id) = id else { continue };

        let mut kind = RevisionKind::FormatChange;
        let mut content_text = String::new();
        let mut author: Option<String> = None;
        let mut timestamp: Option<String> = None;

        for child in region.children() {
            match child.tag_name().name() {
                "change-info" => {
                    for info_child in child.children() {
                        match info_child.tag_name().name() {
                            "creator" => {
                                if let Some(t) = info_child.text() {
                                    let trimmed = t.trim();
                                    if !trimmed.is_empty() {
                                        author = Some(trimmed.to_string());
                                    }
                                }
                            }
                            "date" => {
                                if let Some(t) = info_child.text() {
                                    let trimmed = t.trim();
                                    if !trimmed.is_empty() {
                                        timestamp = Some(trimmed.to_string());
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
                "insertion" => {
                    kind = RevisionKind::Insertion;
                    content_text = collect_region_text(child);
                }
                "deletion" => {
                    kind = RevisionKind::Deletion;
                    content_text = collect_region_text(child);
                }
                "format-change" => {
                    kind = RevisionKind::FormatChange;
                }
                _ => {}
            }
        }

        map.insert(
            id.to_string(),
            OdtChangeRegion {
                author,
                timestamp,
                kind,
                content_text,
            },
        );
    }

    map
}

/// Collect all text content from the children of a changed-region variant element
/// (`<text:insertion>` or `<text:deletion>`).
fn collect_region_text(node: roxmltree::Node) -> String {
    let mut parts = Vec::new();
    for desc in node.descendants() {
        if desc.is_text() {
            let t = desc.text().unwrap_or("");
            if !t.trim().is_empty() {
                parts.push(t.to_string());
            }
        }
    }
    parts.join("")
}

/// Pre-extract all images from the ODT ZIP archive into a map keyed by href path.
///
/// Scans the archive for files under `Pictures/` (the standard ODT image directory)
/// and builds a lookup map so that image references in content.xml can be resolved
/// to binary data without re-borrowing the archive during XML walking.
pub(crate) fn pre_extract_images(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
) -> AHashMap<String, (Vec<u8>, String)> {
    use std::io::Read;

    let mut images = AHashMap::new();
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();

    for name in names {
        if !name.starts_with("Pictures/") {
            continue;
        }
        let ext = name.rsplit('.').next().map(|e| e.to_lowercase()).unwrap_or_default();
        let format = match ext.as_str() {
            "jpg" | "jpeg" => "jpeg",
            "png" => "png",
            "gif" => "gif",
            "webp" => "webp",
            "svg" => "svg",
            "bmp" => "bmp",
            "tiff" | "tif" => "tiff",
            _ => "png",
        };
        if let Ok(mut file) = archive.by_name(&name) {
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_ok() && !buf.is_empty() {
                images.insert(name, (buf, format.to_string()));
            }
        }
    }
    images
}

/// Pre-extract embedded formula objects (MathML) from the ODT archive.
///
/// ODT stores formulas in subdirectories like `Object 1/content.xml` containing
/// MathML markup. This function scans for those and converts them to LaTeX via
/// the shared [`crate::extraction::mathml`] converter.
///
/// Returns `Err` if the `SecurityBudget` is exhausted while converting a
/// formula (matching the OMML converter's untrusted-input handling); this
/// aborts the whole extraction rather than silently truncating a formula.
pub(crate) fn pre_extract_formulas(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    budget: &mut SecurityBudget,
) -> crate::error::Result<AHashMap<String, String>> {
    use std::io::Read;

    let mut formulas = AHashMap::new();
    let names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|f| f.name().to_string()))
        .collect();

    for name in &names {
        if !name.ends_with("/content.xml") || name == "content.xml" {
            continue;
        }
        if let Ok(mut file) = archive.by_name(name) {
            let mut xml = String::new();
            if file.read_to_string(&mut xml).is_ok() && xml.contains("math") {
                let text = crate::extraction::mathml::convert_mathml_str_to_latex(&xml, budget)?;
                if !text.is_empty() {
                    let dir = name.trim_end_matches("/content.xml");
                    formulas.insert(dir.to_string(), text.clone());
                    formulas.insert(format!("{}/", dir), text);
                }
            }
        }
    }

    Ok(formulas)
}

/// Extract description text from a `draw:frame` element by looking at
/// `svg:title`, `svg:desc`, and `text:p` children of the frame.
fn extract_frame_description(frame: roxmltree::Node) -> Option<String> {
    for child in frame.children() {
        let name = child.tag_name().name();
        if name == "title"
            && let Some(text) = child.text()
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    for child in frame.children() {
        let name = child.tag_name().name();
        if name == "desc"
            && let Some(text) = child.text()
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    for child in frame.children() {
        if child.tag_name().name() == "p"
            && let Some(text) = extract_node_text(child)
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

/// Process a single `draw:frame`: an embedded formula object takes priority,
/// otherwise any `draw:image` inside is resolved against the pre-extracted
/// image map (or emitted as a text placeholder when unresolvable).
///
/// Shared by the `"p"` arm (a frame nested in paragraph flow) and the
/// top-level `"frame"` arm (a page-anchored frame, #100), so both paths stay
/// in sync instead of drifting.
fn handle_odt_frame(
    frame: roxmltree::Node,
    image_data: &AHashMap<String, (Vec<u8>, String)>,
    formula_data: &AHashMap<String, String>,
    builder: &mut InternalDocumentBuilder,
) {
    use crate::types::internal::{ElementKind, InternalElement};

    let mut is_formula = false;
    for frame_child in frame.children() {
        if frame_child.tag_name().name() == "object" {
            let obj_href = frame_child
                .attribute(("http://www.w3.org/1999/xlink", "href"))
                .or_else(|| frame_child.attribute("xlink:href"));
            if let Some(href) = obj_href {
                let normalized = href.trim_start_matches("./");
                if let Some(formula_text) = formula_data.get(normalized) {
                    builder.push_formula(formula_text, None, None);
                    is_formula = true;
                    break;
                }
            }
        }
    }

    if is_formula {
        return;
    }

    for frame_child in frame.descendants() {
        if frame_child.tag_name().name() != "image" {
            continue;
        }
        let href = frame_child
            .attribute(("http://www.w3.org/1999/xlink", "href"))
            .or_else(|| frame_child.attribute("xlink:href"));

        let description = extract_frame_description(frame).or_else(|| {
            frame
                .attribute(("urn:oasis:names:tc:opendocument:xmlns:svg-compatible:1.0", "title"))
                .or_else(|| frame.attribute("svg:title"))
                .map(|s| s.to_string())
        });

        let extracted = href.and_then(|h| image_data.get(h).map(|(data, format)| (data.clone(), format.clone())));

        if let Some((data, format)) = extracted {
            let (image_kind, kind_confidence) =
                crate::extraction::image_kind::classify(&data, &format, None, None, None, None, false);

            let image = ExtractedImage {
                data: Bytes::from(data),
                format: Cow::Owned(format),
                image_index: 0,
                page_number: None,
                width: None,
                height: None,
                colorspace: None,
                bits_per_component: None,
                is_mask: false,
                description: description.clone(),
                ocr_result: None,
                bounding_box: None,
                source_path: None,
                image_kind: Some(image_kind),
                kind_confidence: Some(kind_confidence),
                cluster_id: None,
                caption: None,
                qr_codes: None,
                data_base64: None,
            };
            let idx = builder.push_image(description.as_deref(), image, None, None);
            if let Some(h) = href {
                let mut attrs = AHashMap::with_capacity(1);
                attrs.insert("src".to_string(), h.to_string());
                builder.set_attributes(idx, attrs);
            }
        } else {
            let text_val = description.as_deref().or(href).unwrap_or("");
            let elem = InternalElement::text(ElementKind::Image { image_index: 0 }, text_val, 0);
            let idx = builder.push_element(elem);
            if let Some(h) = href {
                let mut attrs = AHashMap::with_capacity(1);
                attrs.insert("src".to_string(), h.to_string());
                builder.set_attributes(idx, attrs);
            }
            // `image_data` was pre-populated only from `Pictures/` (#171): a
            // `draw:image` referencing anything else (a linked external file, or a
            // package member outside the standard directory) has no binary to
            // attach, so the image silently degrades into a text placeholder. ~keep
            if let Some(h) = href {
                builder.add_warning(crate::core::diagnostics::warning(
                    ODT_WARNING_SOURCE,
                    format!(
                        "Image reference '{h}' could not be resolved to embedded image data (expected \
                         under Pictures/); a text placeholder was extracted instead of the image"
                    ),
                ));
            }
        }
    }
}

/// Build an `InternalDocument` from ODT content.xml.
///
/// Walks the XML tree and emits flat elements through `InternalDocumentBuilder`.
/// Captures headings, paragraphs, lists, tables, images, formulas, footnotes
/// (with inline markers), and headers/footers with appropriate content layers.
///
/// `budget` enforces hostile-input limits (iteration count, cumulative content
/// size). Any limit violation is converted into `XbergError::Security` via `?`.
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
            // A ZIP without content.xml is not an ODF document at all (#112):
            // there is no body to return, so `Ok` with an empty document would
            // silently tell the caller "this is the document" for something
            // that isn't one. Fail loudly instead.
            return Err(crate::error::XbergError::parsing(
                "ODT archive is missing content.xml; this is not a valid OpenDocument Text file",
            ));
        }
    }

    let doc = Document::parse(&xml_content)
        .map_err(|e| crate::error::XbergError::parsing(format!("Failed to parse content.xml: {}", e)))?;

    let root = doc.root_element();
    let style_map = build_style_map(root);
    let mut list_style_map = build_list_style_map(root);
    // Named list styles (e.g. "L1") are commonly declared in styles.xml's
    // shared `office:styles` rather than content.xml's automatic-styles
    // (#104); merge them in without overriding a same-named style already
    // found in content.xml.
    if let Ok(mut styles_file) = archive.by_name("styles.xml") {
        use std::io::Read;
        let mut styles_xml = String::new();
        if styles_file.read_to_string(&mut styles_xml).is_ok()
            && let Ok(styles_doc) = Document::parse(&styles_xml)
        {
            for (name, ordered) in build_list_style_map(styles_doc.root_element()) {
                list_style_map.entry(name).or_insert(ordered);
            }
        }
    }
    let mut builder = InternalDocumentBuilder::new("odt");

    let mut tracked_changes_present = false;
    let mut revisions: Vec<DocumentRevision> = Vec::new();

    for body_child in root.children() {
        if body_child.tag_name().name() == "body" {
            for text_elem in body_child.children() {
                if text_elem.tag_name().name() == "text" {
                    tracked_changes_present = text_elem.children().any(|n| n.tag_name().name() == "tracked-changes");

                    let change_map = parse_odt_tracked_changes(text_elem);
                    build_internal_elements(
                        text_elem,
                        &mut builder,
                        &style_map,
                        &list_style_map,
                        &image_data,
                        &formula_data,
                        budget,
                        &change_map,
                        &mut revisions,
                    )?;
                }
            }
        }
    }

    extract_odt_internal_headers_footers(archive, &mut builder);

    let mut internal_doc = builder.build();

    internal_doc.revisions = if tracked_changes_present { Some(revisions) } else { None };

    Ok(internal_doc)
}

/// Recursively walk ODT XML elements and populate the `InternalDocumentBuilder`.
///
/// `change_map` contains every `<text:changed-region>` parsed from
/// `<text:tracked-changes>`. When the walker encounters a `<text:change-start>`
/// or `<text:change>` body marker it looks up the region in `change_map` and
/// appends a `DocumentRevision` to `revisions`.
///
/// **Accepted-changes view** (matches DOCX behaviour): inserted text is emitted
/// into the extracted content as live text; deleted text is *not* emitted. The
/// revision list is the separate audit trail for deleted content.
///
/// Returns `Err` when any `SecurityBudget` limit is violated. Each top-level
/// child element consumes one `budget.step()` and emitted text bytes are
/// charged to `budget.account_text()`.
#[allow(clippy::too_many_arguments)]
pub(crate) fn build_internal_elements(
    parent: roxmltree::Node,
    builder: &mut InternalDocumentBuilder,
    style_map: &AHashMap<String, OdtStyleProps>,
    list_style_map: &AHashMap<String, bool>,
    image_data: &AHashMap<String, (Vec<u8>, String)>,
    formula_data: &AHashMap<String, String>,
    budget: &mut SecurityBudget,
    change_map: &AHashMap<String, OdtChangeRegion>,
    revisions: &mut Vec<DocumentRevision>,
) -> crate::error::Result<()> {
    use crate::types::document_structure::ContentLayer;

    let mut footnote_counter = 0u32;
    let mut comment_counter = 0u32;
    let mut paragraph_index: usize = 0;

    for node in parent.children() {
        budget.step()?;
        match node.tag_name().name() {
            "change-start" | "change" => {
                let change_id = node
                    .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "change-id"))
                    .or_else(|| node.attribute("text:change-id"));
                if let Some(id) = change_id {
                    if let Some(region) = change_map.get(id) {
                        let delta = match region.kind {
                            RevisionKind::Insertion => RevisionDelta {
                                content: if region.content_text.is_empty() {
                                    vec![]
                                } else {
                                    vec![DiffLine::Added(region.content_text.clone())]
                                },
                                ..Default::default()
                            },
                            RevisionKind::Deletion => RevisionDelta {
                                content: if region.content_text.is_empty() {
                                    vec![]
                                } else {
                                    vec![DiffLine::Removed(region.content_text.clone())]
                                },
                                ..Default::default()
                            },
                            RevisionKind::FormatChange | RevisionKind::Comment => RevisionDelta::default(),
                        };
                        revisions.push(DocumentRevision {
                            revision_id: id.to_string(),
                            author: region.author.clone(),
                            timestamp: region.timestamp.clone(),
                            kind: region.kind,
                            anchor: Some(RevisionAnchor::Paragraph { index: paragraph_index }),
                            delta,
                        });
                    } else {
                        tracing::warn!(
                            change_id = id,
                            "ODT body marker references unknown changed-region; skipping"
                        );
                    }
                }
            }
            "change-end" => {}
            "h" => {
                let (text, _annotations, uris) = collect_odt_annotations(node, style_map);
                for uri in uris {
                    builder.push_uri(uri);
                }
                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    let level = node
                        .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "outline-level"))
                        .and_then(|v| v.parse::<u8>().ok())
                        .unwrap_or(1);
                    builder.push_heading(level, trimmed, None, None);
                }
                paragraph_index += 1;
            }
            "frame" => {
                // A `draw:frame` reached directly here (not via the "p" arm's
                // descendant scan) is anchored to the page/paragraph rather
                // than wrapped in a `text:p` — commonly `text:anchor-type="page"`
                // (#100). It was previously invisible to this walker entirely.
                handle_odt_frame(node, image_data, formula_data, builder);
            }
            "p" => {
                let mut footnote_markers: Vec<(String, String)> = Vec::new();

                for desc in node.descendants() {
                    if desc.tag_name().name() == "frame" {
                        handle_odt_frame(desc, image_data, formula_data, builder);
                    }
                }

                for child in node.descendants() {
                    if child.tag_name().name() == "note" {
                        // The key prefix carries the note class (#118): "fn"
                        // for a footnote, "en" for an endnote. Both ref and
                        // definition are keyed identically so a consumer can
                        // tell the two apart from the anchor alone, since
                        // `ContentLayer`/`ElementKind` have no separate
                        // endnote variant to carry that distinction. ~keep
                        let key_prefix = if odt_note_label(child) == "endnote" { "en" } else { "fn" };
                        let note_id = child
                            .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "id"))
                            .or_else(|| child.attribute("text:id"));

                        for note_child in child.children() {
                            if note_child.tag_name().name() == "note-citation" {
                                let citation_text = extract_node_text(note_child).unwrap_or_default();
                                let citation_trimmed = citation_text.trim();
                                if !citation_trimmed.is_empty() {
                                    footnote_counter += 1;
                                    let key = note_id
                                        .map(|id| format!("{key_prefix}{id}"))
                                        .unwrap_or_else(|| format!("{key_prefix}{footnote_counter}"));
                                    footnote_markers.push((citation_trimmed.to_string(), key.clone()));
                                    builder.push_footnote_ref(citation_trimmed, &key, None);
                                }
                            }
                            if note_child.tag_name().name() == "note-body"
                                && let Some(note_text) = extract_node_text(note_child)
                            {
                                let trimmed = note_text.trim();
                                if !trimmed.is_empty() {
                                    let key = note_id.map(|id| format!("{key_prefix}{id}")).unwrap_or_else(|| {
                                        if footnote_counter == 0 {
                                            footnote_counter += 1;
                                        }
                                        format!("{key_prefix}{footnote_counter}")
                                    });
                                    let def_idx = builder.push_footnote_definition(trimmed, &key, None);
                                    builder.set_layer(def_idx, ContentLayer::Footnote);
                                }
                            }
                        }
                    }
                }

                // Comments (`office:annotation`, #100). Structurally a comment
                // is the same shape as a footnote (an inline marker plus an
                // out-of-line body); reuse the footnote-ref/-definition
                // machinery rather than adding a dedicated `ElementKind`.
                for child in node.descendants() {
                    if child.tag_name().name() != "annotation" {
                        continue;
                    }
                    let creator = child
                        .children()
                        .find(|n| n.tag_name().name() == "creator")
                        .and_then(|n| n.text())
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty());
                    let body: String = child
                        .children()
                        .filter(|n| n.tag_name().name() == "p")
                        .filter_map(extract_node_text)
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if body.is_empty() {
                        continue;
                    }

                    comment_counter += 1;
                    let annotation_name = child
                        .attribute(("urn:oasis:names:tc:opendocument:xmlns:office:1.0", "name"))
                        .or_else(|| child.attribute("office:name"));
                    let key = annotation_name
                        .map(|name| format!("cmt{name}"))
                        .unwrap_or_else(|| format!("cmt{comment_counter}"));
                    let marker = creator.unwrap_or_else(|| format!("comment {comment_counter}"));
                    builder.push_footnote_ref(&marker, &key, None);
                    let def_idx = builder.push_footnote_definition(&body, &key, None);
                    builder.set_layer(def_idx, ContentLayer::Footnote);
                }

                let (mut text, annotations, uris) = collect_odt_annotations(node, style_map);
                for uri in uris {
                    builder.push_uri(uri);
                }

                for frame in node.children().filter(|n| n.tag_name().name() == "frame") {
                    for text_box in frame.children().filter(|n| n.tag_name().name() == "text-box") {
                        for nested_p in text_box.children().filter(|n| n.tag_name().name() == "p") {
                            let (caption, _, caption_uris) = collect_odt_annotations(nested_p, style_map);
                            for uri in caption_uris {
                                builder.push_uri(uri);
                            }
                            let caption_trimmed = caption.trim();
                            if !caption_trimmed.is_empty() {
                                if !text.is_empty() {
                                    text.push('\n');
                                }
                                text.push_str(caption_trimmed);
                            }
                        }
                    }
                }

                for (citation, _key) in &footnote_markers {
                    let marker = format!("[^{}]", citation);
                    if !text.contains(&marker) {
                        text.push_str(&marker);
                    }
                }

                let trimmed = text.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    builder.push_paragraph(trimmed, annotations, None, None);
                }
                paragraph_index += 1;
            }
            "table" => {
                let cells = extract_table_cells(node);
                if !cells.is_empty() {
                    let cell_count: usize = cells.iter().map(|r| r.len()).sum();
                    budget.add_cells(cell_count)?;
                    if table_has_lossy_repeated_cells(node) {
                        builder.add_warning(crate::core::diagnostics::warning(
                            ODT_WARNING_SOURCE,
                            "A table cell with table:number-columns-repeated and non-empty content was \
                             collapsed to a single cell; the extracted row is narrower than the source \
                             table and following columns may be misaligned",
                        ));
                    }
                    builder.push_table_from_cells(&cells, None, None);
                }
            }
            "list" => {
                build_internal_list(node, builder, list_style_map, image_data, formula_data);
            }
            "section" => {
                build_internal_elements(
                    node,
                    builder,
                    style_map,
                    list_style_map,
                    image_data,
                    formula_data,
                    budget,
                    change_map,
                    revisions,
                )?;
            }
            // Indexes (table of contents, illustration/table/object/user
            // indexes, the alphabetical index, and the bibliography) wrap
            // their rendered entries in `text:index-body`; without this arm
            // they fell through to `_ => {}` and their content was dropped
            // entirely (#100). ~keep
            "table-of-content" | "illustration-index" | "table-index" | "object-index" | "user-index"
            | "alphabetical-index" | "bibliography" => {
                if let Some(index_body) = node.children().find(|n| n.tag_name().name() == "index-body") {
                    build_internal_elements(
                        index_body,
                        builder,
                        style_map,
                        list_style_map,
                        image_data,
                        formula_data,
                        budget,
                        change_map,
                        revisions,
                    )?;
                }
            }
            _ => {}
        }
    }
    Ok(())
}

/// Build list structure from an ODT `text:list` element for InternalDocumentBuilder.
///
/// Resolves the list's own `text:style-name` against `list_style_map` to
/// determine whether it is ordered (numbered) or unordered (bulleted); a
/// nested `text:list` resolves its own style-name independently rather than
/// inheriting the parent's (#104: every ODT list was previously hardcoded
/// unordered regardless of its actual style).
fn build_internal_list(
    list_node: roxmltree::Node,
    builder: &mut InternalDocumentBuilder,
    list_style_map: &AHashMap<String, bool>,
    image_data: &AHashMap<String, (Vec<u8>, String)>,
    formula_data: &AHashMap<String, String>,
) {
    let ordered = list_node
        .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "style-name"))
        .or_else(|| list_node.attribute("text:style-name"))
        .and_then(|name| list_style_map.get(name))
        .copied()
        .unwrap_or(false);

    builder.push_list(ordered);
    for item in list_node.children() {
        if item.tag_name().name() == "list-item" {
            for child in item.children() {
                match child.tag_name().name() {
                    "p" | "h" => {
                        if let Some(text) = extract_node_text(child) {
                            let trimmed = text.trim();
                            if !trimmed.is_empty() {
                                builder.push_list_item(trimmed, ordered, vec![], None, None);
                            }
                        }
                        // An item holds its formula and its images in a frame,
                        // which the text above does not carry. Each one follows
                        // the item that holds it, so the order of the document
                        // survives.
                        for desc in child.descendants() {
                            if desc.tag_name().name() == "frame" {
                                handle_odt_frame(desc, image_data, formula_data, builder);
                            }
                        }
                    }
                    "list" => {
                        build_internal_list(child, builder, list_style_map, image_data, formula_data);
                    }
                    _ => {}
                }
            }
        }
    }
    builder.end_list();
}

/// Extract headers and footers from styles.xml for InternalDocumentBuilder.
fn extract_odt_internal_headers_footers(
    archive: &mut zip::ZipArchive<Cursor<Vec<u8>>>,
    builder: &mut InternalDocumentBuilder,
) {
    use crate::types::document_structure::ContentLayer;

    let mut styles_xml = String::new();
    if let Ok(mut file) = archive.by_name("styles.xml") {
        use std::io::Read;
        if file.read_to_string(&mut styles_xml).is_err() {
            // The member exists but could not be decoded: unlike a missing
            // styles.xml (an ODT with no headers/footers at all, which is not a
            // loss), this is a real document whose header/footer content is now
            // unreadable (#171).
            builder.add_warning(crate::core::diagnostics::warning(
                ODT_WARNING_SOURCE,
                "styles.xml could not be read as text; any headers and footers it defines were not extracted",
            ));
            return;
        }
    } else {
        return;
    }

    let Ok(doc) = Document::parse(&styles_xml) else {
        builder.add_warning(crate::core::diagnostics::warning(
            ODT_WARNING_SOURCE,
            "styles.xml could not be parsed as XML; any headers and footers it defines were not extracted",
        ));
        return;
    };

    for node in doc.root_element().descendants() {
        match node.tag_name().name() {
            "header" => {
                if let Some(text) = extract_node_text(node) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        let idx = builder.push_paragraph(trimmed, vec![], None, None);
                        builder.set_layer(idx, ContentLayer::Header);
                    }
                }
            }
            "footer" => {
                if let Some(text) = extract_node_text(node) {
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        let idx = builder.push_paragraph(trimmed, vec![], None, None);
                        builder.set_layer(idx, ContentLayer::Footer);
                    }
                }
            }
            _ => {}
        }
    }
}

/// Collect text and annotations from an ODT paragraph/heading node's children.
///
/// Walks `<text:span>` children, resolves their `text:style-name` against the
/// style map, and produces byte-offset `TextAnnotation`s. Delegates to
/// [`collect_inline_run`], which recurses so that spans nested inside spans
/// and `text:a` wrapping a styled `text:span` are not truncated (#93, #94).
fn collect_odt_annotations(
    node: roxmltree::Node,
    style_map: &AHashMap<String, OdtStyleProps>,
) -> (String, Vec<crate::types::TextAnnotation>, Vec<ExtractedUri>) {
    let mut text = String::new();
    let mut annotations = Vec::new();
    let mut uris = Vec::new();

    collect_inline_run(node, style_map, &mut text, &mut annotations, &mut uris);

    if text.is_empty()
        && let Some(t) = node.text()
    {
        text = t.to_string();
    }

    (text, annotations, uris)
}

/// Recursively walk the inline children of a paragraph/heading/span/anchor
/// node, accumulating flattened text plus byte-offset annotations.
///
/// Recursing (instead of reading only `child.text()`, which roxmltree
/// resolves to just the first text-node child) is what lets this survive
/// `text:span` nested inside `text:span` and `text:a` wrapping a styled
/// `text:span` — both silently dropped everything past the first level
/// before this fix (#93, #94).
fn collect_inline_run(
    node: roxmltree::Node,
    style_map: &AHashMap<String, OdtStyleProps>,
    text: &mut String,
    annotations: &mut Vec<crate::types::TextAnnotation>,
    uris: &mut Vec<ExtractedUri>,
) {
    use crate::types::builder;
    use crate::types::document_structure::{AnnotationKind, TextAnnotation};

    for child in node.children() {
        match child.tag_name().name() {
            "span" => {
                let start = text.len() as u32;
                collect_inline_run(child, style_map, text, annotations, uris);
                let end = text.len() as u32;
                if end == start {
                    continue;
                }

                let style_name = child
                    .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "style-name"))
                    .or_else(|| child.attribute("text:style-name"));
                if let Some(name) = style_name
                    && let Some(props) = style_map.get(name)
                {
                    if props.bold {
                        annotations.push(builder::bold(start, end));
                    }
                    if props.italic {
                        annotations.push(builder::italic(start, end));
                    }
                    if props.underline {
                        annotations.push(builder::underline(start, end));
                    }
                    if props.strikethrough {
                        annotations.push(builder::strikethrough(start, end));
                    }
                    if let Some(ref color) = props.color {
                        annotations.push(TextAnnotation {
                            start,
                            end,
                            kind: AnnotationKind::Color { value: color.clone() },
                        });
                    }
                    if let Some(ref size) = props.font_size {
                        annotations.push(TextAnnotation {
                            start,
                            end,
                            kind: AnnotationKind::FontSize { value: size.clone() },
                        });
                    }
                }
            }
            "tab" => {
                text.push('\t');
            }
            "line-break" => {
                text.push('\n');
            }
            "note" | "annotation" | "annotation-end" => {
                // Footnotes/endnotes are collected separately by the caller
                // (dedicated marker + definition pass); comments are handled
                // as their own elements (#100). Neither belongs inline here.
            }
            name if is_odf_dropped_field(name) => {
                // Field placeholder (page-number/page-count/date/time/
                // file-name/author-name, ...): ODF writers cache the field's
                // last-computed *display* value as the element's text content
                // so a reader without a live layout engine still sees
                // something, but that cached value is meaningless outside
                // the authoring application — page numbers depend on
                // pagination this extractor never performs, and a cached
                // date/time/filename/author is stale or context-dependent.
                // Emitting it verbatim also risks leaking the literal
                // editor-placeholder text (e.g. LibreOffice's `<number>`) as
                // if it were real document content (#69). Drop the field
                // rather than resolve it. ~keep
            }
            "a" => {
                let start = text.len() as u32;
                collect_inline_run(child, style_map, text, annotations, uris);
                let end = text.len() as u32;
                if end == start {
                    continue;
                }

                let url = child
                    .attribute(("http://www.w3.org/1999/xlink", "href"))
                    .or_else(|| child.attribute("xlink:href"))
                    .unwrap_or("");
                if !url.is_empty() {
                    let link_text = text[start as usize..end as usize].to_string();
                    annotations.push(builder::link(start, end, url, None));
                    let kind = if url.starts_with('#') {
                        UriKind::Anchor
                    } else if url.starts_with("mailto:") {
                        UriKind::Email
                    } else {
                        UriKind::Hyperlink
                    };
                    uris.push(ExtractedUri {
                        url: url.to_string(),
                        label: Some(link_text),
                        page: None,
                        kind,
                    });
                }
            }
            _ => {
                if let Some(t) = child.text() {
                    text.push_str(t);
                } else {
                    // Unknown wrapper element (e.g. `text:ruby`,
                    // `text:meta`): recurse rather than drop its subtree.
                    collect_inline_run(child, style_map, text, annotations, uris);
                }
            }
        }
    }
}

/// ODF pagination field elements whose cached display text must not be emitted
/// as literal document content (#69).
///
/// `text:page-number` and `text:page-count` depend on a pagination pass this
/// extractor never performs, so there is no value it could resolve them to.
/// What they *do* carry is whatever the authoring application last cached as
/// the field's display text — and for a slide master, LibreOffice Impress
/// caches its own editor placeholder, the literal string `<number>`. Emitting
/// that verbatim puts a stray pseudo-tag into the extracted content.
///
/// Deliberately limited to the pagination fields. The sibling fields
/// (`text:date`, `text:time`, `text:file-name`, `text:author-name`) cache a
/// real last-computed value rather than a placeholder, and that value is text
/// the document visibly displays, so dropping them would lose content.
fn is_odf_dropped_field(tag_name: &str) -> bool {
    matches!(tag_name, "page-number" | "page-count")
}

/// Extract table cells as `Vec<Vec<String>>` from an ODT table element.
///
/// Handles both direct `table:table-row` children and rows nested inside
/// `table:table-header-rows` containers, which ODT uses for header rows.
pub(crate) fn extract_table_cells(table_node: roxmltree::Node) -> Vec<Vec<String>> {
    let mut rows = Vec::new();
    for child in table_node.children() {
        match child.tag_name().name() {
            "table-row" => {
                if let Some(row) = extract_row_cells(child) {
                    rows.push(row);
                }
            }
            "table-header-rows" => {
                for row_node in child.children() {
                    if row_node.tag_name().name() == "table-row"
                        && let Some(row) = extract_row_cells(row_node)
                    {
                        rows.push(row);
                    }
                }
            }
            _ => {}
        }
    }
    rows
}

/// Detect whether `table_node` has a `table:table-cell` whose
/// `table:number-columns-repeated` is greater than one and whose content is
/// non-empty.
///
/// [`extract_row_cells`] emits the repeated cell exactly once instead of
/// `number-columns-repeated` times, so a repeated cell that actually carries
/// text collapses the row width and misaligns every following column
/// (#171). A repeated *empty* cell (by far the common case — ODT writers pad
/// trailing blank columns this way) loses nothing worth naming, so it is
/// deliberately excluded to avoid warning on ordinary tables.
fn table_has_lossy_repeated_cells(table_node: roxmltree::Node) -> bool {
    const TABLE_NS: &str = "urn:oasis:names:tc:opendocument:xmlns:table:1.0";

    let row_has_lossy_cell = |row_node: roxmltree::Node| {
        row_node.children().any(|cell_node| {
            if cell_node.tag_name().name() != "table-cell" {
                return false;
            }
            let repeated = cell_node
                .attribute((TABLE_NS, "number-columns-repeated"))
                .or_else(|| cell_node.attribute("table:number-columns-repeated"))
                .and_then(|value| value.parse::<u32>().ok())
                .unwrap_or(1);
            repeated > 1 && extract_node_text(cell_node).is_some_and(|text| !text.trim().is_empty())
        })
    };

    table_node.children().any(|child| match child.tag_name().name() {
        "table-row" => row_has_lossy_cell(child),
        "table-header-rows" => child
            .children()
            .any(|row_node| row_node.tag_name().name() == "table-row" && row_has_lossy_cell(row_node)),
        _ => false,
    })
}

/// Extract cell text values from a single table row node.
fn extract_row_cells(row_node: roxmltree::Node) -> Option<Vec<String>> {
    let mut row_cells = Vec::new();
    for cell_node in row_node.children() {
        if cell_node.tag_name().name() == "table-cell" {
            let cell_text = extract_node_text(cell_node).unwrap_or_default();
            row_cells.push(cell_text.trim().to_string());
        }
    }
    if row_cells.is_empty() { None } else { Some(row_cells) }
}

/// Extract text from a single XML node, handling spans, formatting, and
/// nested block content.
///
/// Used for contexts that can only carry a single flat string — table
/// cells, list items, header/footer paragraphs, frame descriptions — where
/// [`collect_odt_annotations`]'s richer element output has nowhere to go.
///
/// Recurses into every child rather than reading only `child.text()`, so
/// spans/paragraphs/lists/tables nested arbitrarily deep are no longer
/// silently dropped (#93, #117): a nested `text:list` or `text:table` is
/// flattened inline instead of vanishing, and a `text:note` is rendered as a
/// bracketed `[footnote: ...]`/`[endnote: ...]` aside (#118) instead of being
/// lost outright. `office:annotation` (a comment) is intentionally skipped
/// here — comments are extracted as their own elements from paragraph body
/// text (#100) and would otherwise leak into cell/list-item text.
///
/// # Arguments
/// * `node` - The XML node to extract text from
///
/// # Returns
/// * `Option<String>` - The extracted text with formatting preserved
pub(crate) fn extract_node_text(node: roxmltree::Node) -> Option<String> {
    let mut text_parts: Vec<String> = Vec::new();

    for child in node.children() {
        match child.tag_name().name() {
            "tab" => {
                text_parts.push("\t".to_string());
            }
            "line-break" => {
                text_parts.push("\n".to_string());
            }
            "annotation" | "annotation-end" => {}
            name if is_odf_dropped_field(name) => {
                // See `is_odf_dropped_field`: a field's cached display text
                // (e.g. a slide master's page-number placeholder) is not
                // real document content and must not be emitted verbatim
                // (#69).
            }
            "note" => {
                if let Some(note_text) = extract_note_inline(child) {
                    push_block_text(&mut text_parts, note_text);
                }
            }
            "p" | "h" | "list" | "list-item" => {
                if let Some(text) = extract_node_text(child) {
                    push_block_text(&mut text_parts, text);
                }
            }
            "table" => {
                if let Some(text) = extract_table_inline(child) {
                    push_block_text(&mut text_parts, text);
                }
            }
            _ => {
                if let Some(text) = child.text() {
                    text_parts.push(text.to_string());
                } else if child.has_children()
                    && let Some(text) = extract_node_text(child)
                {
                    text_parts.push(text);
                }
            }
        }
    }

    if text_parts.is_empty() {
        node.text().map(|s| s.to_string())
    } else {
        Some(text_parts.join(""))
    }
}

/// Append a block-level chunk of already-flattened text to `text_parts`,
/// separating it from whatever preceded it with a newline so nested
/// paragraphs/lists/tables/notes don't run into adjacent text (#117).
fn push_block_text(text_parts: &mut Vec<String>, text: String) {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return;
    }
    if !text_parts.is_empty() {
        text_parts.push("\n".to_string());
    }
    text_parts.push(trimmed.to_string());
}

/// Flatten a `text:note` (footnote or endnote) into an inline aside for
/// contexts that cannot host a separate footnote element, such as a table
/// cell or list item (#117). Labels the aside by `text:note-class` so
/// footnotes and endnotes remain distinguishable even when flattened
/// (#118).
fn extract_note_inline(note: roxmltree::Node) -> Option<String> {
    let label = odt_note_label(note);

    let citation = note
        .children()
        .find(|n| n.tag_name().name() == "note-citation")
        .and_then(extract_node_text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let body = note
        .children()
        .find(|n| n.tag_name().name() == "note-body")
        .and_then(extract_node_text)
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());

    match (citation, body) {
        (Some(c), Some(b)) => Some(format!("[{label} {c}: {b}]")),
        (None, Some(b)) => Some(format!("[{label}: {b}]")),
        (Some(c), None) => Some(format!("[{label} {c}]")),
        (None, None) => None,
    }
}

/// Resolve a `text:note`'s `text:note-class` to a human-readable label,
/// defaulting to `"footnote"` per the ODF spec when the attribute is absent
/// (#118).
fn odt_note_label(note: roxmltree::Node) -> &'static str {
    let note_class = note
        .attribute(("urn:oasis:names:tc:opendocument:xmlns:text:1.0", "note-class"))
        .or_else(|| note.attribute("text:note-class"))
        .unwrap_or("footnote");
    if note_class == "endnote" { "endnote" } else { "footnote" }
}

/// Flatten a nested `text:table` (a table inside a table cell or list item)
/// into row-per-line, cell-joined inline text (#117). The outer flat-string
/// contexts this feeds into cannot host a real nested `Table` element.
fn extract_table_inline(table_node: roxmltree::Node) -> Option<String> {
    let rows = extract_table_cells(table_node);
    if rows.is_empty() {
        return None;
    }
    Some(rows.iter().map(|row| row.join(" | ")).collect::<Vec<_>>().join("\n"))
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for OdtExtractor {
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
        tracing::debug!(format = "odt", size_bytes = content.len(), "extraction starting");
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

        if let Ok(odt_props) = office_metadata::extract_odt_properties(&mut meta_archive) {
            if let Some(title) = odt_props.title {
                metadata_map.insert(Cow::Borrowed("title"), serde_json::Value::String(title));
            }
            if let Some(creator) = odt_props.creator {
                metadata_map.insert(
                    Cow::Borrowed("authors"),
                    serde_json::Value::Array(vec![serde_json::Value::String(creator.clone())]),
                );
                metadata_map.insert(Cow::Borrowed("created_by"), serde_json::Value::String(creator));
            }
            if let Some(initial_creator) = odt_props.initial_creator {
                metadata_map.insert(
                    Cow::Borrowed("initial_creator"),
                    serde_json::Value::String(initial_creator),
                );
            }
            if let Some(subject) = odt_props.subject {
                metadata_map.insert(Cow::Borrowed("subject"), serde_json::Value::String(subject));
            }
            if let Some(keywords) = odt_props.keywords {
                metadata_map.insert(Cow::Borrowed("keywords"), serde_json::Value::String(keywords));
            }
            if let Some(description) = odt_props.description {
                metadata_map.insert(Cow::Borrowed("description"), serde_json::Value::String(description));
            }
            if let Some(creation_date) = odt_props.creation_date {
                metadata_map.insert(Cow::Borrowed("created_at"), serde_json::Value::String(creation_date));
            }
            if let Some(date) = odt_props.date {
                metadata_map.insert(Cow::Borrowed("modified_at"), serde_json::Value::String(date));
            }
            if let Some(language) = odt_props.language {
                metadata_map.insert(Cow::Borrowed("language"), serde_json::Value::String(language));
            }
            if let Some(generator) = odt_props.generator {
                metadata_map.insert(Cow::Borrowed("generator"), serde_json::Value::String(generator));
            }
            if let Some(editing_duration) = odt_props.editing_duration {
                metadata_map.insert(
                    Cow::Borrowed("editing_duration"),
                    serde_json::Value::String(editing_duration),
                );
            }
            if let Some(editing_cycles) = odt_props.editing_cycles {
                metadata_map.insert(
                    Cow::Borrowed("editing_cycles"),
                    serde_json::Value::String(editing_cycles),
                );
            }
            if let Some(page_count) = odt_props.page_count {
                metadata_map.insert(
                    Cow::Borrowed("page_count"),
                    serde_json::Value::Number(page_count.into()),
                );
            }
            if let Some(word_count) = odt_props.word_count {
                metadata_map.insert(
                    Cow::Borrowed("word_count"),
                    serde_json::Value::Number(word_count.into()),
                );
            }
            if let Some(character_count) = odt_props.character_count {
                metadata_map.insert(
                    Cow::Borrowed("character_count"),
                    serde_json::Value::Number(character_count.into()),
                );
            }
            if let Some(paragraph_count) = odt_props.paragraph_count {
                metadata_map.insert(
                    Cow::Borrowed("paragraph_count"),
                    serde_json::Value::Number(paragraph_count.into()),
                );
            }
            if let Some(table_count) = odt_props.table_count {
                metadata_map.insert(
                    Cow::Borrowed("table_count"),
                    serde_json::Value::Number(table_count.into()),
                );
            }
            if let Some(image_count) = odt_props.image_count {
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
            format = "odt",
            "extraction complete"
        );
        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/vnd.oasis.opendocument.text"]
    }

    fn priority(&self) -> i32 {
        60
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_odt_extractor_plugin_interface() {
        let extractor = OdtExtractor::new();
        assert_eq!(extractor.name(), "odt-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 60);
        assert_eq!(extractor.supported_mime_types().len(), 1);
    }

    #[tokio::test]
    async fn test_odt_extractor_supports_odt() {
        let extractor = OdtExtractor::new();
        assert!(
            extractor
                .supported_mime_types()
                .contains(&"application/vnd.oasis.opendocument.text")
        );
    }

    #[tokio::test]
    async fn test_odt_extractor_default() {
        let extractor = OdtExtractor;
        assert_eq!(extractor.name(), "odt-extractor");
    }

    #[tokio::test]
    async fn test_odt_extractor_initialize_shutdown() {
        let extractor = OdtExtractor::new();
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    /// The OASIS OpenDocument specification anchors all of its formula objects
    /// inside list items, and reported none of them.
    #[test]
    fn test_formula_in_a_list_item_reaches_the_document_after_its_item() {
        let content = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
  xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
  xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
  xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
  xmlns:xlink="http://www.w3.org/1999/xlink">
 <office:body><office:text>
  <text:list><text:list-item><text:p>First item
   <draw:frame><draw:object xlink:href="./Object 1"/></draw:frame>
  </text:p></text:list-item></text:list>
 </office:text></office:body></office:document-content>"#;
        let object = r#"<math xmlns="http://www.w3.org/1998/Math/MathML"><mi>x</mi></math>"#;

        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            let options = zip::write::SimpleFileOptions::default();
            writer.start_file("content.xml", options).unwrap();
            std::io::Write::write_all(&mut writer, content.as_bytes()).unwrap();
            writer.start_file("Object 1/content.xml", options).unwrap();
            std::io::Write::write_all(&mut writer, object.as_bytes()).unwrap();
            writer.finish().unwrap();
        }

        let mut archive = zip::ZipArchive::new(Cursor::new(buffer)).unwrap();
        let mut budget = SecurityBudget::with_defaults();
        let doc = build_internal_document(&mut archive, &mut budget).expect("extracts");

        let kinds: Vec<&crate::types::internal::ElementKind> = doc.elements.iter().map(|e| &e.kind).collect();
        let formula_at = kinds
            .iter()
            .position(|k| matches!(k, crate::types::internal::ElementKind::Formula))
            .expect("the list item's formula reaches the document");
        let item_at = kinds
            .iter()
            .position(|k| matches!(k, crate::types::internal::ElementKind::ListItem { .. }))
            .expect("the list item is present");
        assert!(
            formula_at > item_at,
            "the equation follows the item that holds it, got kinds {kinds:?}"
        );
    }

    #[test]
    fn test_extract_node_text_simple() {
        let xml = r#"<p xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">Hello world</p>"#;
        let doc = roxmltree::Document::parse(xml).unwrap();
        let node = doc.root_element();

        let result = extract_node_text(node);
        assert!(result.is_some());
        assert!(!result.unwrap().is_empty());
    }

    /// #118: `odt_note_label` must default to "footnote" and only report
    /// "endnote" when `text:note-class="endnote"` is present. This is only
    /// observable at the unit level — by the time a public `ExtractedDocument`
    /// is built, the footnote/endnote anchor key has been resolved into a
    /// node index and the "fn"/"en" prefix is no longer visible on the wire.
    #[test]
    fn test_odt_note_label_distinguishes_footnote_and_endnote() {
        const NS: &str = r#"xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0""#;

        let endnote_xml = format!(r#"<note {NS} text:note-class="endnote"/>"#);
        let endnote_doc = roxmltree::Document::parse(&endnote_xml).unwrap();
        assert_eq!(odt_note_label(endnote_doc.root_element()), "endnote");

        let footnote_xml = format!(r#"<note {NS} text:note-class="footnote"/>"#);
        let footnote_doc = roxmltree::Document::parse(&footnote_xml).unwrap();
        assert_eq!(odt_note_label(footnote_doc.root_element()), "footnote");

        let no_class_xml = "<note/>";
        let no_class_doc = roxmltree::Document::parse(no_class_xml).unwrap();
        assert_eq!(
            odt_note_label(no_class_doc.root_element()),
            "footnote",
            "an absent text:note-class must default to footnote per the ODF spec"
        );
    }

    /// #118: footnote and endnote anchor keys must use different prefixes
    /// ("fn"/"en") so a `FootnoteRef`/`FootnoteDefinition` pair for an
    /// endnote can never collide with, or be confused for, a footnote's.
    #[tokio::test]
    async fn test_odt_footnote_and_endnote_keys_use_distinct_prefixes() {
        use std::io::Write;

        let content_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content
    xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
    xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0">
  <office:body>
    <office:text>
      <text:p>Body<text:note text:note-class="footnote" text:id="ftn1">
        <text:note-citation>1</text:note-citation>
        <text:note-body><text:p>a footnote</text:p></text:note-body>
      </text:note><text:note text:note-class="endnote" text:id="ftn1">
        <text:note-citation>i</text:note-citation>
        <text:note-body><text:p>an endnote</text:p></text:note-body>
      </text:note></text:p>
    </office:text>
  </office:body>
</office:document-content>"#;

        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all("application/vnd.oasis.opendocument.text".as_bytes())
                .unwrap();
            let deflated =
                zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("content.xml", deflated).unwrap();
            zip.write_all(content_xml.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let mut archive = zip::ZipArchive::new(Cursor::new(buf)).unwrap();
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        use crate::types::internal::ElementKind;

        let footnote_def_anchor = doc
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::FootnoteDefinition) && e.text == "a footnote")
            .and_then(|e| e.anchor.clone())
            .expect("expected a footnote definition anchored to 'a footnote'");
        let endnote_def_anchor = doc
            .elements
            .iter()
            .find(|e| matches!(e.kind, ElementKind::FootnoteDefinition) && e.text == "an endnote")
            .and_then(|e| e.anchor.clone())
            .expect("expected an endnote definition anchored to 'an endnote'");

        assert!(
            footnote_def_anchor.starts_with("fn"),
            "footnote anchor key should use the 'fn' prefix, got {footnote_def_anchor:?}"
        );
        assert!(
            endnote_def_anchor.starts_with("en"),
            "endnote anchor key should use the 'en' prefix, got {endnote_def_anchor:?}"
        );
        assert_ne!(
            footnote_def_anchor, endnote_def_anchor,
            "a footnote and an endnote sharing the same text:id must not collide on anchor key"
        );
    }

    /// Helper to load test ODT, extract with document structure, and return the structure.
    async fn extract_odt_with_structure(filename: &str) -> Option<crate::types::document_structure::DocumentStructure> {
        let test_file = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../test_documents/odt")
            .join(filename);
        if !test_file.exists() {
            return None;
        }
        let content = std::fs::read(&test_file).expect("Failed to read test ODT");
        let extractor = OdtExtractor::new();
        let config = ExtractionConfig {
            include_document_structure: true,
            ..Default::default()
        };
        let result = extractor
            .extract_content(&content, "application/vnd.oasis.opendocument.text", &config)
            .await
            .expect("ODT extraction failed");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);
        result.document
    }

    #[tokio::test]
    async fn test_odt_footnote_extraction() {
        let doc = extract_odt_with_structure("footnote.odt").await;
        let Some(doc) = doc else { return };
        let has_footnote = doc.nodes.iter().any(|n| {
            matches!(
                n.content,
                crate::types::document_structure::NodeContent::Footnote { .. }
            )
        });
        assert!(
            has_footnote,
            "Footnote ODT should produce Footnote nodes in document structure"
        );
    }

    #[tokio::test]
    async fn test_odt_header_extraction() {
        let doc = extract_odt_with_structure("headers.odt").await;
        let Some(doc) = doc else { return };
        let has_heading = doc.nodes.iter().any(|n| {
            matches!(
                n.content,
                crate::types::document_structure::NodeContent::Group {
                    heading_level: Some(_),
                    ..
                }
            )
        });
        assert!(
            has_heading,
            "Headers ODT should produce Group nodes with heading_level in document structure"
        );
    }

    #[tokio::test]
    async fn test_odt_image_extraction() {
        let doc = extract_odt_with_structure("imageWithCaption.odt").await;
        let Some(doc) = doc else { return };
        let has_image = doc
            .nodes
            .iter()
            .any(|n| matches!(n.content, crate::types::document_structure::NodeContent::Image { .. }));
        assert!(has_image, "Image ODT should produce Image nodes in document structure");
    }

    #[tokio::test]
    async fn test_odt_bold_annotations() {
        let doc = extract_odt_with_structure("bold.odt").await;
        let Some(doc) = doc else { return };
        let has_bold = doc.nodes.iter().any(|n| {
            n.annotations
                .iter()
                .any(|a| matches!(a.kind, crate::types::document_structure::AnnotationKind::Bold))
        });
        assert!(
            has_bold,
            "Bold ODT should produce Bold annotations in document structure"
        );
    }

    #[tokio::test]
    async fn test_odt_italic_annotations() {
        let doc = extract_odt_with_structure("italic.odt").await;
        let Some(doc) = doc else { return };
        let has_italic = doc.nodes.iter().any(|n| {
            n.annotations
                .iter()
                .any(|a| matches!(a.kind, crate::types::document_structure::AnnotationKind::Italic))
        });
        assert!(
            has_italic,
            "Italic ODT should produce Italic annotations in document structure"
        );
    }

    #[tokio::test]
    async fn test_odt_underline_annotations() {
        let doc = extract_odt_with_structure("strikeout.odt").await;
        let Some(doc) = doc else { return };
        let has_strikethrough = doc.nodes.iter().any(|n| {
            n.annotations
                .iter()
                .any(|a| matches!(a.kind, crate::types::document_structure::AnnotationKind::Strikethrough))
        });
        assert!(
            has_strikethrough,
            "Strikeout ODT should produce Strikethrough annotations"
        );
    }

    /// Build an in-memory ODT ZIP archive from named text members, always
    /// including `mimetype` and `content.xml`.
    fn build_odt_zip(text_files: &[(&str, &str)]) -> zip::ZipArchive<Cursor<Vec<u8>>> {
        use std::io::Write;

        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.oasis.opendocument.text").unwrap();

            for (name, content) in text_files {
                let deflated =
                    zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
                zip.start_file(*name, deflated).unwrap();
                zip.write_all(content.as_bytes()).unwrap();
            }
            zip.finish().unwrap();
        }
        zip::ZipArchive::new(Cursor::new(buf)).unwrap()
    }

    const CONTENT_XML_NAMESPACES: &str = r#"xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0"
        xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0"
        xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0"
        xmlns:draw="urn:oasis:names:tc:opendocument:xmlns:drawing:1.0"
        xmlns:xlink="http://www.w3.org/1999/xlink""#;

    fn odt_warnings(doc: &InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == ODT_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #171: a `draw:image` referencing a path that was never pre-extracted from
    /// `Pictures/` (a missing member, or a reference outside that directory)
    /// silently degrades to a text placeholder; the caller must be told.
    #[tokio::test]
    async fn should_warn_when_odt_image_reference_is_not_resolved() {
        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_XML_NAMESPACES}>
  <office:body>
    <office:text>
      <draw:frame>
        <draw:image xlink:href="Pictures/missing.png"/>
      </draw:frame>
    </office:text>
  </office:body>
</office:document-content>"#
        );
        let mut archive = build_odt_zip(&[("content.xml", &content_xml)]);
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        let warnings = odt_warnings(&doc);
        assert_eq!(warnings.len(), 1, "expected exactly one odt warning, got {warnings:?}");
        assert!(
            warnings[0].contains("Pictures/missing.png") && warnings[0].contains("could not be resolved"),
            "warning must name the unresolved image reference, got {warnings:?}"
        );
    }

    /// The same reference resolved against a pre-extracted `Pictures/` member
    /// must not warn.
    #[tokio::test]
    async fn should_not_warn_when_odt_image_reference_resolves() {
        use std::io::Write;

        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_XML_NAMESPACES}>
  <office:body>
    <office:text>
      <draw:frame>
        <draw:image xlink:href="Pictures/present.png"/>
      </draw:frame>
    </office:text>
  </office:body>
</office:document-content>"#
        );

        let mut buf = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(Cursor::new(&mut buf));
            let stored = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("mimetype", stored).unwrap();
            zip.write_all(b"application/vnd.oasis.opendocument.text").unwrap();
            let deflated =
                zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("content.xml", deflated).unwrap();
            zip.write_all(content_xml.as_bytes()).unwrap();
            let deflated =
                zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Deflated);
            zip.start_file("Pictures/present.png", deflated).unwrap();
            zip.write_all(&[0x89, b'P', b'N', b'G', 0, 0, 0, 0]).unwrap();
            zip.finish().unwrap();
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(buf)).unwrap();
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        assert!(
            odt_warnings(&doc).is_empty(),
            "a resolvable image reference must not warn, got {:?}",
            odt_warnings(&doc)
        );
    }

    /// #171: `styles.xml` present but not parseable as XML must warn instead of
    /// silently returning no headers/footers, which is indistinguishable from a
    /// document that genuinely has none.
    #[tokio::test]
    async fn should_warn_when_odt_styles_xml_is_unparseable() {
        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_XML_NAMESPACES}>
  <office:body><office:text><text:p>Body</text:p></office:text></office:body>
</office:document-content>"#
        );
        let mut archive = build_odt_zip(&[("content.xml", &content_xml), ("styles.xml", "<not valid xml")]);
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        let warnings = odt_warnings(&doc);
        assert_eq!(warnings.len(), 1, "expected exactly one odt warning, got {warnings:?}");
        assert!(
            warnings[0].contains("styles.xml could not be parsed as XML"),
            "warning must name the styles.xml parse failure, got {warnings:?}"
        );
    }

    /// A missing `styles.xml` is a normal ODT with no headers/footers, not a
    /// loss, and must not warn.
    #[tokio::test]
    async fn should_not_warn_when_odt_has_no_styles_xml() {
        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_XML_NAMESPACES}>
  <office:body><office:text><text:p>Body</text:p></office:text></office:body>
</office:document-content>"#
        );
        let mut archive = build_odt_zip(&[("content.xml", &content_xml)]);
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        assert!(
            odt_warnings(&doc).is_empty(),
            "a document with no styles.xml at all must not warn, got {:?}",
            odt_warnings(&doc)
        );
    }

    /// #171: a non-empty `table:table-cell` repeated via
    /// `table:number-columns-repeated` is emitted only once by
    /// `extract_row_cells`, narrowing the row and misaligning later columns.
    #[tokio::test]
    async fn should_warn_when_odt_table_collapses_nonempty_repeated_cell() {
        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_XML_NAMESPACES}>
  <office:body>
    <office:text>
      <table:table>
        <table:table-row>
          <table:table-cell table:number-columns-repeated="3"><text:p>Value</text:p></table:table-cell>
        </table:table-row>
      </table:table>
    </office:text>
  </office:body>
</office:document-content>"#
        );
        let mut archive = build_odt_zip(&[("content.xml", &content_xml)]);
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        let warnings = odt_warnings(&doc);
        assert_eq!(warnings.len(), 1, "expected exactly one odt warning, got {warnings:?}");
        assert!(
            warnings[0].contains("number-columns-repeated"),
            "warning must name the repeated-cell collapse, got {warnings:?}"
        );
    }

    /// A repeated *empty* trailing cell (the common case for ODT writers
    /// padding a row to a fixed column count) loses nothing worth naming.
    #[tokio::test]
    async fn should_not_warn_when_odt_table_repeats_an_empty_cell() {
        let content_xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<office:document-content {CONTENT_XML_NAMESPACES}>
  <office:body>
    <office:text>
      <table:table>
        <table:table-row>
          <table:table-cell><text:p>Value</text:p></table:table-cell>
          <table:table-cell table:number-columns-repeated="5"/>
        </table:table-row>
      </table:table>
    </office:text>
  </office:body>
</office:document-content>"#
        );
        let mut archive = build_odt_zip(&[("content.xml", &content_xml)]);
        let mut budget = SecurityBudget::from_config(&ExtractionConfig::default());
        let doc = build_internal_document(&mut archive, &mut budget).expect("extraction should succeed");

        assert!(
            odt_warnings(&doc).is_empty(),
            "a repeated empty cell must not warn, got {:?}",
            odt_warnings(&doc)
        );
    }
}
