//! Render an `InternalDocument` to Docling DocTags.
//!
//! DocTags is the tag vocabulary used by Docling and the SmolDocling /
//! Granite-Docling models. The document is wrapped in `<doctag>`, one element
//! per line, with tables encoded as OTSL.
//!
//! `<loc_*>` tokens are emitted for an element when it carries a bounding box
//! and its page recorded usable dimensions. In practice that means PDF today:
//! it is the only path that populates both. Pure-text formats (Markdown, HTML)
//! have no geometry at all and so carry no location tokens — a documented,
//! deliberate degradation rather than a silent one.
//!
//! One limitation remains, inherited from the document model rather than from
//! this renderer:
//!
//! - **OTSL merge tokens are never emitted.** `Table::cells` is a flat
//!   `Vec<Vec<String>>` with no span information, so a table can only be
//!   serialised as `<ched>`/`<fcel>`/`<ecel>` plus `<nl>` row separators.
//!   `<lcel>`/`<ucel>`/`<xcel>`/`<rhed>` require cell spans the model does not
//!   carry.
//!
//! Text is emitted verbatim. DocTags has no escaping mechanism — Docling's own
//! serialiser writes `&` and `<` literally, and its tokenizer only recognises
//! known tags — so escaping here would corrupt round-trips.

use crate::types::document_structure::{ContentLayer, RelationshipKind};
use crate::types::extraction::BoundingBox;
use crate::types::internal::{ElementKind, InternalDocument, RelationshipTarget};
use ahash::AHashMap;

use super::common::{get_admonition_kind, get_admonition_title, normalize_inline_text, parse_metadata_entries};

/// Language token emitted for a code block with no recorded language.
const UNKNOWN_LANGUAGE: &str = "<_unknown_>";

/// DocTags normalises bounding boxes onto a fixed square grid of this size.
const LOC_GRID: f64 = 500.0;

/// Render an `InternalDocument` to Docling DocTags.
pub(crate) fn render_doctags(doc: &InternalDocument) -> String {
    let mut out = String::with_capacity(doc.elements.len() * 96);
    let captions = collect_captions(doc);
    let dims = page_dimensions(doc);

    out.push_str("<doctag>");

    let mut state = ListState::default();

    for (index, elem) in doc.elements.iter().enumerate() {
        let index = index as u32;

        // Captions are emitted nested inside the element they describe.
        if captions.sources.contains_key(&index) {
            continue;
        }

        let loc = element_loc(elem, &dims);
        let loc = loc.as_deref();

        match elem.kind {
            ElementKind::ListItem { ordered } => {
                state.open(&mut out, ordered);
                push_element(&mut out, "list_item", loc, &normalize_inline_text(&elem.text), None);
                continue;
            }
            ElementKind::ListStart { ordered } => {
                state.open_explicit(&mut out, ordered);
                continue;
            }
            ElementKind::ListEnd => {
                state.close_explicit(&mut out);
                continue;
            }
            _ => state.close_implicit(&mut out),
        }

        match elem.kind {
            ElementKind::QuoteStart | ElementKind::QuoteEnd | ElementKind::GroupStart | ElementKind::GroupEnd => {}
            ElementKind::FootnoteRef => {}
            ElementKind::PageBreak => {
                out.push_str("<page_break>\n");
            }
            ElementKind::Table { table_index } => {
                if let Some(table) = doc.tables.get(table_index as usize) {
                    let caption = captions.caption_payload(doc, index, &dims);
                    push_otsl(&mut out, &table.cells, loc, caption.as_deref());
                }
            }
            ElementKind::Image { image_index } => {
                let described = doc
                    .images
                    .get(image_index as usize)
                    .and_then(|img| img.description.as_deref())
                    .filter(|desc| !desc.is_empty());
                let caption = captions
                    .caption_payload(doc, index, &dims)
                    .or_else(|| described.map(normalize_inline_text));
                push_element(&mut out, "picture", loc, "", caption.as_deref());
            }
            ElementKind::Code => {
                let body = code_body(super::common::get_language(elem), &elem.text);
                let caption = captions.caption_payload(doc, index, &dims);
                push_element(&mut out, "code", loc, &body, caption.as_deref());
            }
            ElementKind::Formula => {
                push_element(&mut out, "formula", loc, &normalize_inline_text(&elem.text), None);
            }
            ElementKind::Admonition => {
                let label = get_admonition_title(elem).unwrap_or_else(|| get_admonition_kind(elem));
                push_text_element(&mut out, elem.layer, loc, label);
                push_text_element(&mut out, elem.layer, loc, &normalize_inline_text(&elem.text));
            }
            ElementKind::MetadataBlock => {
                let entries = parse_metadata_entries(&elem.text);
                if entries.is_empty() {
                    push_text_element(&mut out, elem.layer, loc, &normalize_inline_text(&elem.text));
                } else {
                    for (key, value) in entries {
                        push_text_element(&mut out, elem.layer, loc, &format!("{}: {}", key, value));
                    }
                }
            }
            ElementKind::RawBlock => {
                let body = code_body(None, &elem.text);
                push_element(&mut out, "code", loc, &body, None);
            }
            ElementKind::Title => {
                push_labelled(&mut out, elem.layer, loc, "title", &normalize_inline_text(&elem.text));
            }
            ElementKind::Heading { level } => {
                let tag = format!("section_header_level_{}", level.max(1));
                push_labelled(&mut out, elem.layer, loc, &tag, &normalize_inline_text(&elem.text));
            }
            ElementKind::FootnoteDefinition => {
                push_element(&mut out, "footnote", loc, &normalize_inline_text(&elem.text), None);
            }
            ElementKind::Paragraph
            | ElementKind::Citation
            | ElementKind::Slide { .. }
            | ElementKind::DefinitionTerm
            | ElementKind::DefinitionDescription
            | ElementKind::OcrText { .. } => {
                push_text_element(&mut out, elem.layer, loc, &normalize_inline_text(&elem.text));
            }
            // Handled above.
            ElementKind::ListItem { .. } | ElementKind::ListStart { .. } | ElementKind::ListEnd => {}
        }
    }

    state.close_implicit(&mut out);
    state.close_all(&mut out);

    out.push_str("</doctag>");
    out
}

/// Tracks open `<ordered_list>` / `<unordered_list>` wrappers.
///
/// Extractors emit list items either wrapped in explicit `ListStart`/`ListEnd`
/// markers or bare. Bare items open an implicit wrapper that closes at the next
/// non-list element.
#[derive(Default)]
struct ListState {
    explicit: Vec<bool>,
    implicit: Option<bool>,
}

impl ListState {
    fn open(&mut self, out: &mut String, ordered: bool) {
        if self.explicit.is_empty() && self.implicit.is_none() {
            push_list_open(out, ordered);
            self.implicit = Some(ordered);
        }
    }

    fn open_explicit(&mut self, out: &mut String, ordered: bool) {
        self.close_implicit(out);
        push_list_open(out, ordered);
        self.explicit.push(ordered);
    }

    fn close_explicit(&mut self, out: &mut String) {
        if let Some(ordered) = self.explicit.pop() {
            push_list_close(out, ordered);
        }
    }

    fn close_implicit(&mut self, out: &mut String) {
        if let Some(ordered) = self.implicit.take() {
            push_list_close(out, ordered);
        }
    }

    fn close_all(&mut self, out: &mut String) {
        while let Some(ordered) = self.explicit.pop() {
            push_list_close(out, ordered);
        }
    }
}

fn push_list_open(out: &mut String, ordered: bool) {
    if ordered {
        out.push_str("<ordered_list>");
    } else {
        out.push_str("<unordered_list>");
    }
}

fn push_list_close(out: &mut String, ordered: bool) {
    if ordered {
        out.push_str("</ordered_list>\n");
    } else {
        out.push_str("</unordered_list>\n");
    }
}

/// Per-page dimensions in points, keyed by 1-indexed page number.
///
/// Only pages that recorded usable dimensions are included; the emptiness of
/// this map is what suppresses location tokens for formats that have none.
fn page_dimensions(doc: &InternalDocument) -> AHashMap<u32, (f64, f64)> {
    let mut dims = AHashMap::new();
    let Some(pages) = doc.metadata.pages.as_ref().and_then(|pages| pages.pages.as_ref()) else {
        return dims;
    };
    for page in pages {
        let Some((width, height)) = page.dimensions else {
            continue;
        };
        if width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0 {
            dims.insert(page.number, (width, height));
        }
    }
    dims
}

/// Render an element's `<loc_*>` tokens, or `None` when geometry is unusable.
///
/// Input boxes follow the `BoundingBox` contract — PDF user space, origin at
/// the bottom-left, `y0` the bottom edge and `y1` the top. DocTags counts from
/// the top-left, so the vertical axis is flipped here. Tokens are emitted as
/// left, top, right, bottom.
///
/// Formats whose bounding boxes do not follow that contract must not reach this
/// function. PPTX is the live example: `extraction::pptx` fills `y0` with the
/// *top* edge, so its geometry would be flipped. It is excluded structurally
/// rather than by a format check — PPTX never records page dimensions, so
/// `page_dimensions` yields nothing for it and no tokens are produced.
fn loc_tokens(bbox: &BoundingBox, (width, height): (f64, f64)) -> Option<String> {
    let coords = [bbox.x0, bbox.y0, bbox.x1, bbox.y1];
    if coords.iter().any(|value| !value.is_finite()) {
        return None;
    }

    // Tolerate boxes recorded with reversed corners.
    let left = bbox.x0.min(bbox.x1);
    let right = bbox.x0.max(bbox.x1);
    let bottom = bbox.y0.min(bbox.y1);
    let top = bbox.y0.max(bbox.y1);

    let to_grid = |value: f64, extent: f64| -> u32 {
        let scaled = (value / extent * LOC_GRID).round();
        scaled.clamp(0.0, LOC_GRID) as u32
    };

    Some(format!(
        "<loc_{}><loc_{}><loc_{}><loc_{}>",
        to_grid(left, width),
        to_grid(height - top, height),
        to_grid(right, width),
        to_grid(height - bottom, height),
    ))
}

/// Resolve an element's location tokens against its page's dimensions.
fn element_loc(elem: &crate::types::internal::InternalElement, dims: &AHashMap<u32, (f64, f64)>) -> Option<String> {
    let bbox = elem.bbox.as_ref()?;
    let page = elem.page?;
    loc_tokens(bbox, *dims.get(&page)?)
}

/// Build a `<code>` body: a language token followed by the flattened source.
fn code_body(language: Option<&str>, text: &str) -> String {
    let mut body = match language {
        Some(language) => format!("<_{}_>", language),
        None => UNKNOWN_LANGUAGE.to_string(),
    };
    body.push_str(&normalize_inline_text(text));
    body
}

/// Emit an element: `<tag>` then location tokens, body, and a nested
/// `<caption>` when present.
fn push_element(out: &mut String, tag: &str, loc: Option<&str>, body: &str, caption: Option<&str>) {
    out.push('<');
    out.push_str(tag);
    out.push('>');
    if let Some(loc) = loc {
        out.push_str(loc);
    }
    out.push_str(body);
    if let Some(caption) = caption {
        out.push_str("<caption>");
        out.push_str(caption);
        out.push_str("</caption>");
    }
    out.push_str("</");
    out.push_str(tag);
    out.push_str(">\n");
}

/// Emit a prose element, letting the content layer choose the tag.
fn push_text_element(out: &mut String, layer: ContentLayer, loc: Option<&str>, text: &str) {
    if text.is_empty() {
        return;
    }
    push_element(out, layer_tag(layer, "text"), loc, text, None);
}

/// Emit an element whose tag is overridden by a non-body content layer.
fn push_labelled(out: &mut String, layer: ContentLayer, loc: Option<&str>, tag: &str, text: &str) {
    if text.is_empty() {
        return;
    }
    push_element(out, layer_tag(layer, tag), loc, text, None);
}

/// Map a content layer onto its DocTags tag, falling back to `body_tag`.
fn layer_tag(layer: ContentLayer, body_tag: &str) -> &str {
    match layer {
        ContentLayer::Body => body_tag,
        ContentLayer::Header => "page_header",
        ContentLayer::Footer => "page_footer",
        ContentLayer::Footnote => "footnote",
    }
}

/// Serialise a flat cell grid as OTSL.
///
/// The first row is treated as the header (matching `render_table_markdown`).
/// Ragged rows are padded with `<ecel>` so every row has the same cell count,
/// which OTSL requires.
fn push_otsl(out: &mut String, cells: &[Vec<String>], loc: Option<&str>, caption: Option<&str>) {
    if cells.is_empty() {
        return;
    }
    let columns = cells.iter().map(|row| row.len()).max().unwrap_or(0);
    if columns == 0 {
        return;
    }

    out.push_str("<otsl>");
    if let Some(loc) = loc {
        out.push_str(loc);
    }
    for (row_index, row) in cells.iter().enumerate() {
        for column in 0..columns {
            let content = row.get(column).map(|s| s.trim()).unwrap_or("");
            if content.is_empty() {
                out.push_str("<ecel>");
            } else {
                out.push_str(if row_index == 0 { "<ched>" } else { "<fcel>" });
                out.push_str(&normalize_inline_text(content));
            }
        }
        out.push_str("<nl>");
    }
    if let Some(caption) = caption {
        out.push_str("<caption>");
        out.push_str(caption);
        out.push_str("</caption>");
    }
    out.push_str("</otsl>\n");
}

/// Caption relationships, indexed both ways.
struct Captions {
    /// Caption element index → the element it describes.
    sources: AHashMap<u32, u32>,
    /// Described element index → its caption element index.
    targets: AHashMap<u32, u32>,
}

impl Captions {
    /// The inner content of a `<caption>`: its own location tokens, then text.
    fn caption_payload(&self, doc: &InternalDocument, target: u32, dims: &AHashMap<u32, (f64, f64)>) -> Option<String> {
        let source = *self.targets.get(&target)?;
        let elem = doc.elements.get(source as usize)?;
        let text = normalize_inline_text(&elem.text);
        if text.is_empty() {
            return None;
        }
        Some(match element_loc(elem, dims) {
            Some(loc) => format!("{}{}", loc, text),
            None => text,
        })
    }
}

fn collect_captions(doc: &InternalDocument) -> Captions {
    let mut sources = AHashMap::new();
    let mut targets = AHashMap::new();

    for rel in &doc.relationships {
        if rel.kind != RelationshipKind::Caption {
            continue;
        }
        let RelationshipTarget::Index(target) = rel.target else {
            continue;
        };
        // Only tags that nest a `<caption>` may claim one. Captions are also
        // attached to plain paragraphs standing in for a figure (LaTeX
        // `figure` with placeholder injection); those must keep rendering as
        // their own element rather than being silently swallowed.
        let nests_caption = doc.elements.get(target as usize).is_some_and(|elem| {
            matches!(
                elem.kind,
                ElementKind::Table { .. } | ElementKind::Image { .. } | ElementKind::Code
            )
        });
        if !nests_caption {
            continue;
        }
        // First caption wins, so a described element never gains two captions.
        if targets.contains_key(&target) {
            continue;
        }
        sources.insert(rel.source, target);
        targets.insert(target, rel.source);
    }

    Captions { sources, targets }
}

/// Structural validation of a DocTags stream.
///
/// Kept as independent checks rather than one pass so each can be applied where
/// it is actually valid. The vendored Docling corpus predates OTSL in places and
/// still carries legacy `<table>`/`<tr>`/`<td>` markup, so vocabulary and OTSL
/// checks cannot run against it, while wrapper and location invariants can.
#[cfg(test)]
mod validate {
    /// Cell tokens that occupy one OTSL grid position.
    const OTSL_CELLS: &[&str] = &["fcel", "ched", "ecel", "lcel", "ucel", "xcel", "rhed"];

    /// Tokens that stand alone rather than wrapping content.
    const STANDALONE: &[&str] = &["nl", "page_break"];

    /// Tags that must open and close.
    ///
    /// `checkbox_*` wrap their label text in real Docling output, e.g.
    /// `<checkbox_unselected><loc_…>بلی</checkbox_unselected>`, so they pair.
    const PAIRED: &[&str] = &[
        "doctag",
        "checkbox_selected",
        "checkbox_unselected",
        "text",
        "title",
        "page_header",
        "page_footer",
        "footnote",
        "caption",
        "code",
        "formula",
        "otsl",
        "picture",
        "list_item",
        "ordered_list",
        "unordered_list",
    ];

    #[derive(Debug, PartialEq)]
    pub(super) enum Token<'a> {
        Open(&'a str),
        Close(&'a str),
    }

    /// Extract tag tokens in order. Text content between tags is ignored.
    ///
    /// Only *recognised* tag names count as tags. DocTags has no escaping, and
    /// real Docling output carries literal `<` in prose (the vendored
    /// `2203.01017v2` corpus discusses `< td >` in a caption). Scanning for the
    /// next `>` would swallow the real closing tag that follows such text, so
    /// anything that is not a known token is treated as content.
    pub(super) fn tags(doc: &str) -> Vec<Token<'_>> {
        let mut out = Vec::new();
        let bytes = doc.as_bytes();
        let mut i = 0;
        while i < bytes.len() {
            if bytes[i] != b'<' {
                i += 1;
                continue;
            }
            let Some(end) = doc[i..].find('>') else { break };
            let name = &doc[i + 1..i + end];
            let token = match name.strip_prefix('/') {
                Some(closing) => Token::Close(closing),
                None => Token::Open(name),
            };
            let recognised = match token {
                Token::Open(name) | Token::Close(name) => is_standalone(name) || is_paired(name),
            };
            if !recognised {
                i += 1;
                continue;
            }
            out.push(token);
            i += end + 1;
        }
        out
    }

    fn is_standalone(name: &str) -> bool {
        STANDALONE.contains(&name)
            || OTSL_CELLS.contains(&name)
            || name.starts_with("loc_")
            // Code language tokens, e.g. `<_rust_>` and `<_unknown_>`.
            || (name.starts_with('_') && name.ends_with('_') && name.len() > 1)
    }

    fn is_paired(name: &str) -> bool {
        PAIRED.contains(&name) || name.starts_with("section_header_level_")
    }

    /// Every angle-bracketed run is a tag this format defines.
    ///
    /// Unlike [`tags`], this scans raw and refuses to skip what it does not
    /// recognise, which is the point: it catches a tag we emitted by mistake.
    /// Only meaningful for output built from content with no stray `<`, since
    /// the format cannot distinguish the two.
    pub(super) fn vocabulary(doc: &str) -> Result<(), String> {
        let mut rest = doc;
        while let Some(start) = rest.find('<') {
            rest = &rest[start + 1..];
            let Some(end) = rest.find('>') else { break };
            let name = &rest[..end];
            let name = name.strip_prefix('/').unwrap_or(name);
            if !is_standalone(name) && !is_paired(name) {
                return Err(format!("unknown tag <{}>", name));
            }
            rest = &rest[end + 1..];
        }
        Ok(())
    }

    /// Paired tags nest correctly and the document is wrapped in `<doctag>`.
    pub(super) fn wrapper_and_nesting(doc: &str) -> Result<(), String> {
        if !doc.starts_with("<doctag>") {
            return Err("missing opening <doctag>".to_string());
        }
        if !doc.ends_with("</doctag>") {
            return Err("missing closing </doctag>".to_string());
        }

        let mut stack: Vec<&str> = Vec::new();
        for token in tags(doc) {
            match token {
                Token::Open(name) if is_paired(name) => stack.push(name),
                Token::Close(name) => match stack.pop() {
                    Some(open) if open == name => {}
                    Some(open) => return Err(format!("</{}> closes <{}>", name, open)),
                    None => return Err(format!("</{}> with nothing open", name)),
                },
                _ => {}
            }
        }
        if let Some(unclosed) = stack.pop() {
            return Err(format!("<{}> never closed", unclosed));
        }
        Ok(())
    }

    /// Location tokens come in fours, sit on the 0–500 grid, and are ordered
    /// left ≤ right and top ≤ bottom.
    pub(super) fn location_tokens(doc: &str) -> Result<(), String> {
        let mut group: Vec<u32> = Vec::with_capacity(4);
        for token in tags(doc) {
            let Token::Open(name) = token else { continue };
            let Some(value) = name.strip_prefix("loc_") else {
                if !group.is_empty() && group.len() < 4 {
                    return Err(format!("location group of {}, expected 4", group.len()));
                }
                group.clear();
                continue;
            };
            let value: u32 = value.parse().map_err(|_| format!("malformed <{}>", name))?;
            if value > 500 {
                return Err(format!("<{}> exceeds the 0-500 grid", name));
            }
            group.push(value);
            if group.len() == 4 {
                if group[0] > group[2] {
                    return Err(format!("left {} right {}", group[0], group[2]));
                }
                if group[1] > group[3] {
                    return Err(format!("top {} bottom {}", group[1], group[3]));
                }
                group.clear();
            }
        }
        if !group.is_empty() {
            return Err(format!("trailing location group of {}", group.len()));
        }
        Ok(())
    }

    /// Every OTSL row holds the same number of cells.
    pub(super) fn otsl_rows(doc: &str) -> Result<(), String> {
        let mut in_otsl = false;
        let mut row = 0usize;
        let mut expected: Option<usize> = None;
        for token in tags(doc) {
            match token {
                Token::Open("otsl") => {
                    in_otsl = true;
                    row = 0;
                    expected = None;
                }
                Token::Close("otsl") => {
                    if row != 0 {
                        return Err("OTSL row not terminated by <nl>".to_string());
                    }
                    in_otsl = false;
                }
                Token::Open("nl") if in_otsl => {
                    match expected {
                        Some(width) if width != row => {
                            return Err(format!("OTSL row of {} cells, expected {}", row, width));
                        }
                        None => expected = Some(row),
                        _ => {}
                    }
                    row = 0;
                }
                Token::Open(name) if in_otsl && OTSL_CELLS.contains(&name) => row += 1,
                _ => {}
            }
        }
        Ok(())
    }

    /// All checks, for output this renderer produced.
    pub(super) fn strict(doc: &str) -> Result<(), String> {
        vocabulary(doc)?;
        wrapper_and_nesting(doc)?;
        location_tokens(doc)?;
        otsl_rows(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal_builder::InternalDocumentBuilder;

    fn image(description: Option<&str>) -> crate::types::ExtractedImage {
        crate::types::ExtractedImage {
            data: bytes::Bytes::new(),
            format: std::borrow::Cow::Borrowed("png"),
            image_index: 0,
            page_number: None,
            width: None,
            height: None,
            colorspace: None,
            bits_per_component: None,
            is_mask: false,
            description: description.map(str::to_string),
            ocr_result: None,
            bounding_box: None,
            source_path: None,
            image_kind: None,
            kind_confidence: None,
            cluster_id: None,
            caption: None,
            qr_codes: None,
            data_base64: None,
        }
    }

    fn bbox(x0: f64, y0: f64, x1: f64, y1: f64) -> BoundingBox {
        BoundingBox { x0, y0, x1, y1 }
    }

    /// US Letter, chosen so the grid arithmetic lands on round numbers.
    const PAGE_W: f64 = 612.0;
    const PAGE_H: f64 = 792.0;

    fn with_page_dims(mut doc: InternalDocument, dimensions: Option<(f64, f64)>) -> InternalDocument {
        doc.metadata.pages = Some(crate::types::PageStructure {
            total_count: 1,
            unit_type: crate::types::PageUnitType::Page,
            boundaries: None,
            pages: Some(vec![crate::types::PageInfo {
                number: 1,
                title: None,
                dimensions,
                image_count: None,
                table_count: None,
                hidden: None,
                is_blank: None,
                has_vector_graphics: false,
            }]),
        });
        doc
    }

    #[test]
    fn test_loc_tokens_flip_the_vertical_axis() {
        // Box spanning the top-left quarter of the page.
        let top_left = loc_tokens(&bbox(61.2, 396.0, 306.0, 792.0), (PAGE_W, PAGE_H)).unwrap();
        assert_eq!(top_left, "<loc_50><loc_0><loc_250><loc_250>");

        // The same box moved to the bottom half: PDF y grows upward, DocTags
        // downward, so the tokens must increase rather than stay put.
        let bottom_left = loc_tokens(&bbox(61.2, 0.0, 306.0, 396.0), (PAGE_W, PAGE_H)).unwrap();
        assert_eq!(bottom_left, "<loc_50><loc_250><loc_250><loc_500>");
    }

    #[test]
    fn test_loc_tokens_clamp_out_of_page_boxes() {
        let out = loc_tokens(&bbox(-100.0, -100.0, PAGE_W * 2.0, PAGE_H * 2.0), (PAGE_W, PAGE_H)).unwrap();
        assert_eq!(out, "<loc_0><loc_0><loc_500><loc_500>");
    }

    #[test]
    fn test_loc_tokens_tolerate_reversed_corners() {
        let forward = loc_tokens(&bbox(61.2, 396.0, 306.0, 792.0), (PAGE_W, PAGE_H)).unwrap();
        let reversed = loc_tokens(&bbox(306.0, 792.0, 61.2, 396.0), (PAGE_W, PAGE_H)).unwrap();
        assert_eq!(forward, reversed);
    }

    #[test]
    fn test_loc_tokens_reject_non_finite_coordinates() {
        assert!(loc_tokens(&bbox(f64::NAN, 0.0, 10.0, 10.0), (PAGE_W, PAGE_H)).is_none());
        assert!(loc_tokens(&bbox(0.0, 0.0, f64::INFINITY, 10.0), (PAGE_W, PAGE_H)).is_none());
    }

    #[test]
    fn test_render_doctags_emits_loc_tokens_when_geometry_is_available() {
        let mut b = InternalDocumentBuilder::new("pdf");
        b.push_paragraph("Body text.", vec![], Some(1), Some(bbox(61.2, 396.0, 306.0, 792.0)));
        let doc = with_page_dims(b.build(), Some((PAGE_W, PAGE_H)));
        let out = render_doctags(&doc);
        assert_eq!(
            out,
            "<doctag><text><loc_50><loc_0><loc_250><loc_250>Body text.</text>\n</doctag>"
        );
    }

    /// PPTX fills `y0` with the top edge instead of the bottom, so its geometry
    /// must not be normalised as if it were PDF space. It is excluded by never
    /// recording page dimensions — this pins that gate.
    #[test]
    fn test_render_doctags_omits_loc_tokens_without_page_dimensions() {
        let mut b = InternalDocumentBuilder::new("pptx");
        b.push_paragraph("Slide text.", vec![], Some(1), Some(bbox(61.2, 396.0, 306.0, 792.0)));
        let doc = with_page_dims(b.build(), None);
        let out = render_doctags(&doc);
        assert_eq!(out, "<doctag><text>Slide text.</text>\n</doctag>");
        assert!(!out.contains("<loc_"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_omits_loc_tokens_without_bbox_or_page() {
        let mut b = InternalDocumentBuilder::new("pdf");
        b.push_paragraph("No bbox.", vec![], Some(1), None);
        b.push_paragraph("No page.", vec![], None, Some(bbox(0.0, 0.0, 10.0, 10.0)));
        let doc = with_page_dims(b.build(), Some((PAGE_W, PAGE_H)));
        let out = render_doctags(&doc);
        assert!(!out.contains("<loc_"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_rejects_degenerate_page_dimensions() {
        let mut b = InternalDocumentBuilder::new("pdf");
        b.push_paragraph("Text.", vec![], Some(1), Some(bbox(0.0, 0.0, 10.0, 10.0)));
        let doc = with_page_dims(b.build(), Some((0.0, PAGE_H)));
        assert!(!render_doctags(&doc).contains("<loc_"));
    }

    #[test]
    fn test_render_doctags_otsl_and_caption_carry_their_own_loc() {
        let mut b = InternalDocumentBuilder::new("pdf");
        let cells = vec![vec!["A".to_string()]];
        let table = b.push_table_from_cells(&cells, Some(1), Some(bbox(61.2, 396.0, 306.0, 792.0)));
        let caption = b.push_paragraph("Table 1.", vec![], Some(1), Some(bbox(61.2, 0.0, 306.0, 396.0)));
        b.push_relationship(caption, RelationshipTarget::Index(table), RelationshipKind::Caption);
        let doc = with_page_dims(b.build(), Some((PAGE_W, PAGE_H)));
        let out = render_doctags(&doc);
        assert!(
            out.contains("<otsl><loc_50><loc_0><loc_250><loc_250><ched>A<nl>"),
            "got: {}",
            out
        );
        assert!(
            out.contains("<caption><loc_50><loc_250><loc_250><loc_500>Table 1.</caption>"),
            "got: {}",
            out
        );
    }

    /// A document exercising every element kind the renderer handles, so the
    /// structural checks run against the full tag surface rather than a slice.
    fn kitchen_sink() -> InternalDocument {
        let mut b = InternalDocumentBuilder::new("pdf");
        b.push_title("Doc", Some(1), Some(bbox(61.2, 700.0, 306.0, 780.0)));
        b.push_heading(1, "Section", Some(1), Some(bbox(61.2, 650.0, 306.0, 690.0)));
        b.push_paragraph("Body.", vec![], Some(1), Some(bbox(61.2, 600.0, 306.0, 640.0)));
        b.push_list(true);
        b.push_list_item("First", true, vec![], Some(1), None);
        b.end_list();
        b.push_list_item("Bare", false, vec![], None, None);
        b.push_code("fn main() {}", Some("rust"), Some(1), None);
        b.push_formula("E = mc^2", Some(1), None);
        b.push_raw_block("tex", "\\LaTeX{}", None);
        b.push_admonition("warning", Some("Careful"), None);
        b.push_page_break();

        let cells = vec![
            vec!["H1".to_string(), "H2".to_string()],
            vec!["a".to_string(), String::new()],
            vec!["b".to_string()],
        ];
        let table = b.push_table_from_cells(&cells, Some(1), Some(bbox(61.2, 400.0, 306.0, 500.0)));
        let caption = b.push_paragraph("Table 1.", vec![], Some(1), Some(bbox(61.2, 380.0, 306.0, 395.0)));
        b.push_relationship(caption, RelationshipTarget::Index(table), RelationshipKind::Caption);

        b.push_image(Some("A photo"), image(Some("A photo")), Some(1), None);

        let header = b.push_paragraph("Running header", vec![], Some(1), None);
        b.set_layer(header, ContentLayer::Header);
        let footer = b.push_paragraph("Page 1", vec![], Some(1), None);
        b.set_layer(footer, ContentLayer::Footer);
        b.push_footnote_ref("1", "fn1", None);
        let def = b.push_footnote_definition("A note.", "fn1", Some(1));
        b.set_layer(def, ContentLayer::Footnote);

        with_page_dims(b.build(), Some((PAGE_W, PAGE_H)))
    }

    #[test]
    fn test_rendered_output_is_structurally_valid() {
        let out = render_doctags(&kitchen_sink());
        if let Err(problem) = validate::strict(&out) {
            panic!("invalid DocTags: {}\n{}", problem, out);
        }
    }

    #[test]
    fn test_rendered_output_is_valid_without_geometry() {
        let doc = kitchen_sink();
        let mut stripped = doc.clone();
        stripped.metadata.pages = None;
        let out = render_doctags(&stripped);
        assert!(!out.contains("<loc_"), "got: {}", out);
        if let Err(problem) = validate::strict(&out) {
            panic!("invalid DocTags: {}\n{}", problem, out);
        }
    }

    /// The validator must accept genuine Docling output, otherwise it is
    /// encoding our own idea of the format rather than the format itself.
    /// Vocabulary and OTSL checks are omitted: two corpus files predate OTSL
    /// and still carry legacy `<table>`/`<tr>`/`<td>` markup.
    #[test]
    fn test_validator_accepts_the_vendored_docling_corpus() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents");
        let mut checked = 0;
        for dir in ["vendored/docling/txt", "ground_truth/txt"] {
            let Ok(entries) = std::fs::read_dir(root.join(dir)) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.to_string_lossy().ends_with(".doctags.txt") {
                    continue;
                }
                let Ok(content) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let content = content.trim();
                if content.is_empty() {
                    continue;
                }
                if let Err(problem) = validate::wrapper_and_nesting(content) {
                    panic!("{}: {}", path.display(), problem);
                }
                if let Err(problem) = validate::location_tokens(content) {
                    panic!("{}: {}", path.display(), problem);
                }
                checked += 1;
            }
        }
        // Self-skips when the submodule is absent, matching the repo convention.
        if checked == 0 {
            eprintln!("test_documents not populated, skipping corpus validation");
        }
    }

    #[test]
    fn test_validator_rejects_malformed_streams() {
        assert!(validate::wrapper_and_nesting("<text>no wrapper</text>").is_err());
        assert!(validate::wrapper_and_nesting("<doctag><text>unclosed</doctag>").is_err());
        assert!(validate::vocabulary("<doctag><nonsense>x</nonsense></doctag>").is_err());
        assert!(validate::location_tokens("<doctag><text><loc_501>x</text></doctag>").is_err());
        // Only three tokens in the group.
        assert!(validate::location_tokens("<doctag><text><loc_1><loc_2><loc_3>x</text></doctag>").is_err());
        // Right edge left of the left edge.
        assert!(validate::location_tokens("<doctag><text><loc_9><loc_2><loc_1><loc_4>x</text></doctag>").is_err());
        assert!(validate::otsl_rows("<doctag><otsl><ched>a<ched>b<nl><fcel>c<nl></otsl></doctag>").is_err());
    }

    #[test]
    fn test_render_doctags_empty_document() {
        let doc = InternalDocumentBuilder::new("test").build();
        assert_eq!(render_doctags(&doc), "<doctag></doctag>");
    }

    #[test]
    fn test_render_doctags_wraps_in_doctag() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Hello world.", vec![], None, None);
        let out = render_doctags(&b.build());
        assert_eq!(out, "<doctag><text>Hello world.</text>\n</doctag>");
    }

    #[test]
    fn test_render_doctags_title_and_headings() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_title("My Document", None, None);
        b.push_heading(1, "Intro", None, None);
        b.push_heading(3, "Detail", None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<title>My Document</title>"), "got: {}", out);
        assert!(
            out.contains("<section_header_level_1>Intro</section_header_level_1>"),
            "got: {}",
            out
        );
        assert!(
            out.contains("<section_header_level_3>Detail</section_header_level_3>"),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_render_doctags_unordered_list_wrapped_and_closed() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_list_item("Alpha", false, vec![], None, None);
        b.push_list_item("Beta", false, vec![], None, None);
        b.end_list();
        let out = render_doctags(&b.build());
        assert!(
            out.contains(
                "<unordered_list><list_item>Alpha</list_item>\n<list_item>Beta</list_item>\n</unordered_list>"
            ),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_render_doctags_ordered_list_uses_matching_close_tag() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(true);
        b.push_list_item("First", true, vec![], None, None);
        b.end_list();
        let out = render_doctags(&b.build());
        assert!(out.contains("<ordered_list><list_item>First"), "got: {}", out);
        assert!(out.contains("</ordered_list>"), "got: {}", out);
        assert!(!out.contains("</unordered_list>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_bare_list_items_open_and_close_implicit_wrapper() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list_item("Alpha", false, vec![], None, None);
        b.push_paragraph("After the list.", vec![], None, None);
        let out = render_doctags(&b.build());
        assert!(
            out.contains(
                "<unordered_list><list_item>Alpha</list_item>\n</unordered_list>\n<text>After the list.</text>"
            ),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_render_doctags_unterminated_list_closes_at_end() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_list(false);
        b.push_list_item("Alpha", false, vec![], None, None);
        let out = render_doctags(&b.build());
        assert!(out.ends_with("</unordered_list>\n</doctag>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_table_as_otsl_with_header_row() {
        let mut b = InternalDocumentBuilder::new("test");
        let cells = vec![
            vec!["Name".to_string(), "Age".to_string()],
            vec!["Alice".to_string(), "30".to_string()],
        ];
        b.push_table_from_cells(&cells, None, None);
        let out = render_doctags(&b.build());
        assert!(
            out.contains("<otsl><ched>Name<ched>Age<nl><fcel>Alice<fcel>30<nl></otsl>"),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_render_doctags_table_pads_ragged_rows_with_ecel() {
        let mut b = InternalDocumentBuilder::new("test");
        let cells = vec![vec!["A".to_string(), "B".to_string()], vec!["only".to_string()]];
        b.push_table_from_cells(&cells, None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<fcel>only<ecel><nl>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_table_empty_cell_becomes_ecel() {
        let mut b = InternalDocumentBuilder::new("test");
        let cells = vec![
            vec!["A".to_string(), "B".to_string()],
            vec!["".to_string(), "value".to_string()],
        ];
        b.push_table_from_cells(&cells, None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<ecel><fcel>value<nl>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_empty_table_emits_nothing() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_table_from_cells(&[], None, None);
        let out = render_doctags(&b.build());
        assert_eq!(out, "<doctag></doctag>");
    }

    #[test]
    fn test_render_doctags_code_carries_language_token() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_code("fn main() {}", Some("rust"), None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<code><_rust_>fn main() {}</code>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_code_without_language_uses_unknown_token() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_code("echo hi", None, None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<code><_unknown_>echo hi</code>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_code_is_flattened_to_one_line() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_code("line one\nline two", None, None, None);
        let out = render_doctags(&b.build());
        assert!(
            out.contains("<code><_unknown_>line one line two</code>"),
            "got: {}",
            out
        );
        assert_eq!(out.lines().count(), 2, "got: {}", out);
    }

    #[test]
    fn test_render_doctags_formula() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_formula("E = mc^2", None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<formula>E = mc^2</formula>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_page_break() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Before", vec![], None, None);
        b.push_page_break();
        b.push_paragraph("After", vec![], None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("</text>\n<page_break>\n<text>After"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_image_becomes_picture() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_image(Some("A nice photo"), image(Some("A nice photo")), None, None);
        let out = render_doctags(&b.build());
        assert!(
            out.contains("<picture><caption>A nice photo</caption></picture>"),
            "got: {}",
            out
        );
    }

    #[test]
    fn test_render_doctags_image_without_description_has_no_caption() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_image(None, image(None), None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<picture></picture>"), "got: {}", out);
        assert!(!out.contains("<caption>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_content_layer_selects_tag() {
        let mut b = InternalDocumentBuilder::new("test");
        let header = b.push_paragraph("Running header", vec![], None, None);
        b.set_layer(header, ContentLayer::Header);
        let footer = b.push_paragraph("Page 3", vec![], None, None);
        b.set_layer(footer, ContentLayer::Footer);
        let out = render_doctags(&b.build());
        assert!(
            out.contains("<page_header>Running header</page_header>"),
            "got: {}",
            out
        );
        assert!(out.contains("<page_footer>Page 3</page_footer>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_footnote_definition() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("Main text", vec![], None, None);
        b.push_footnote_ref("1", "fn1", None);
        let def = b.push_footnote_definition("A note.", "fn1", None);
        b.set_layer(def, ContentLayer::Footnote);
        let out = render_doctags(&b.build());
        assert!(out.contains("<footnote>A note.</footnote>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_footnote_ref_is_not_emitted() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_footnote_ref("1", "fn1", None);
        let out = render_doctags(&b.build());
        assert_eq!(out, "<doctag></doctag>");
    }

    #[test]
    fn test_render_doctags_text_is_not_escaped() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("results & performance for a < b", vec![], None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("results & performance for a < b"), "got: {}", out);
        assert!(!out.contains("&amp;"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_paragraph_newlines_collapse_to_one_line() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("wrapped\nacross\nlines", vec![], None, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<text>wrapped across lines</text>"), "got: {}", out);
        assert_eq!(out.lines().count(), 2, "got: {}", out);
    }

    #[test]
    fn test_render_doctags_empty_paragraph_is_skipped() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_paragraph("", vec![], None, None);
        let out = render_doctags(&b.build());
        assert_eq!(out, "<doctag></doctag>");
    }

    #[test]
    fn test_render_doctags_quote_and_group_markers_are_transparent() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_quote_start();
        b.push_paragraph("Quoted text.", vec![], None, None);
        b.push_quote_end();
        let out = render_doctags(&b.build());
        assert_eq!(out, "<doctag><text>Quoted text.</text>\n</doctag>");
    }

    #[test]
    fn test_render_doctags_metadata_block_emits_one_text_per_entry() {
        let mut b = InternalDocumentBuilder::new("test");
        let entries = vec![
            ("Author".to_string(), "Alice".to_string()),
            ("Date".to_string(), "2026".to_string()),
        ];
        b.push_metadata_block(&entries, None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<text>Author: Alice</text>"), "got: {}", out);
        assert!(out.contains("<text>Date: 2026</text>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_admonition_emits_label_then_body() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_admonition("warning", Some("Be careful"), None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<text>Be careful</text>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_raw_block_becomes_code() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_raw_block("tex", "\\LaTeX{}", None);
        let out = render_doctags(&b.build());
        assert!(out.contains("<code><_unknown_>\\LaTeX{}</code>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_table_caption_nests_and_is_not_emitted_twice() {
        let mut b = InternalDocumentBuilder::new("test");
        let cells = vec![vec!["A".to_string()]];
        let table = b.push_table_from_cells(&cells, None, None);
        let caption = b.push_paragraph("Table 1. Results.", vec![], None, None);
        b.push_relationship(caption, RelationshipTarget::Index(table), RelationshipKind::Caption);
        let out = render_doctags(&b.build());
        assert!(
            out.contains("<caption>Table 1. Results.</caption></otsl>"),
            "got: {}",
            out
        );
        assert!(!out.contains("<text>Table 1. Results.</text>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_picture_caption_relationship_wins_over_description() {
        let mut b = InternalDocumentBuilder::new("test");
        let picture = b.push_image(Some("alt text"), image(Some("alt text")), None, None);
        let caption = b.push_paragraph("Figure 1. A diagram.", vec![], None, None);
        b.push_relationship(caption, RelationshipTarget::Index(picture), RelationshipKind::Caption);
        let out = render_doctags(&b.build());
        assert!(
            out.contains("<picture><caption>Figure 1. A diagram.</caption></picture>"),
            "got: {}",
            out
        );
        assert!(!out.contains("alt text"), "got: {}", out);
    }

    /// The renderer is reachable without an `OutputFormat` variant: an unknown
    /// format string parses to `Custom`, which `derive_extraction_result` routes
    /// through the renderer registry. This is what makes `output_format="doctags"`
    /// work from every language binding.
    #[test]
    fn test_doctags_is_reachable_via_custom_output_format() {
        use crate::core::config::OutputFormat;
        use std::str::FromStr;

        // `FromStr` is the API-handler path; serde is the path the language
        // bindings take (e.g. Python's `OutputFormat("doctags")`).
        let format = OutputFormat::from_str("doctags").unwrap();
        assert_eq!(format, OutputFormat::Custom("doctags".to_string()));
        assert_eq!(
            serde_json::from_str::<OutputFormat>("\"doctags\"").unwrap(),
            OutputFormat::Custom("doctags".to_string())
        );

        let mut b = InternalDocumentBuilder::new("test");
        b.push_title("Doc", None, None);
        b.push_paragraph("Body text.", vec![], None, None);

        let result = crate::extraction::derive::derive_extraction_result(b.build(), false, format);

        assert_eq!(
            result.formatted_content.as_deref(),
            Some("<doctag><title>Doc</title>\n<text>Body text.</text>\n</doctag>")
        );
    }

    /// LaTeX `figure` with placeholder injection attaches a caption to a plain
    /// paragraph. `<text>` cannot nest a `<caption>`, so the caption must still
    /// render as its own element instead of being dropped.
    #[test]
    fn test_render_doctags_caption_on_paragraph_target_is_not_dropped() {
        let mut b = InternalDocumentBuilder::new("test");
        let placeholder = b.push_paragraph("[image: diagram.png]", vec![], None, None);
        let caption = b.push_paragraph("Figure 1. A diagram.", vec![], None, None);
        b.push_relationship(
            caption,
            RelationshipTarget::Index(placeholder),
            RelationshipKind::Caption,
        );
        let out = render_doctags(&b.build());
        assert!(out.contains("<text>Figure 1. A diagram.</text>"), "got: {}", out);
        assert!(out.contains("<text>[image: diagram.png]</text>"), "got: {}", out);
    }

    #[test]
    fn test_render_doctags_every_element_is_on_its_own_line() {
        let mut b = InternalDocumentBuilder::new("test");
        b.push_title("Doc", None, None);
        b.push_paragraph("One", vec![], None, None);
        b.push_paragraph("Two", vec![], None, None);
        let out = render_doctags(&b.build());
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "got: {}", out);
        assert!(lines[0].starts_with("<doctag><title>"), "got: {}", out);
        assert_eq!(lines[3], "</doctag>", "got: {}", out);
    }
}
