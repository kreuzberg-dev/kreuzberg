//! DocBook document extractor supporting both 4.x and 5.x formats.
//!
//! This extractor handles DocBook XML documents in both traditional (4.x, no namespace)
//! and modern (5.x, with <http://docbook.org/ns/docbook> namespace) formats.
//!
//! Single-pass architecture that extracts in one document traversal:
//! - Document metadata (title, author, date, abstract)
//! - Section hierarchy and content
//! - Paragraphs and text content
//! - Lists (itemizedlist, orderedlist)
//! - Code blocks (programlisting, screen)
//! - Blockquotes
//! - Figures and mediaobjects
//! - Footnotes
//! - Tables
//! - Cross-references and links

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::{cells_to_markdown, cells_to_text};
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::text::utf8_validation;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::uri::ExtractedUri;
use crate::types::{Metadata, Table};
use async_trait::async_trait;
use quick_xml::events::Event;

use crate::utils::xml_utils::EntityReader;
use std::collections::HashMap;
#[cfg(feature = "tokio-runtime")]
use std::path::Path;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const DOCBOOK_WARNING_SOURCE: &str = "docbook";

/// Strip namespace prefix from XML tag names.
/// Converts "{http://docbook.org/ns/docbook}title" to "title"
/// and leaves non-namespaced "title" unchanged.
fn strip_namespace(tag: &str) -> &str {
    if tag.starts_with('{')
        && let Some(pos) = tag.find('}')
    {
        return &tag[pos + 1..];
    }
    tag
}

/// State machine for tracking nested elements during extraction
#[derive(Debug, Clone, Copy)]
struct ParsingState {
    in_info: bool,
    in_table: bool,
    in_tgroup: bool,
    in_thead: bool,
    in_tbody: bool,
    in_row: bool,
    in_list: bool,
    in_list_item: bool,
}
#[cfg_attr(alef, alef(skip))]
/// DocBook document extractor.
///
/// Supports both DocBook 4.x (no namespace) and 5.x (with namespace) formats.
pub struct DocbookExtractor;

impl Default for DocbookExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl DocbookExtractor {
    pub(crate) fn new() -> Self {
        Self
    }
}

/// Type alias for DocBook parsing results: (content, title, author, date, tables, publisher, copyright)
type DocBookParseResult = (
    String,
    String,
    Option<String>,
    Option<String>,
    Vec<Table>,
    Option<String>,
    Option<String>,
);

/// Wrap DocBook content in a synthetic root element if it lacks one.
///
/// Some DocBook fragments (e.g. tables-only files) have multiple sibling
/// top-level elements without a single root. XML parsers require a single root,
/// so we wrap the content in `<_root>...</_root>` when needed.
fn ensure_root_element(content: &str) -> std::borrow::Cow<'_, str> {
    let trimmed = content.trim_start();
    let body = if trimmed.starts_with("<?xml") {
        trimmed
            .find("?>")
            .map(|pos| trimmed[pos + 2..].trim_start())
            .unwrap_or(trimmed)
    } else {
        trimmed
    };
    let body = if body.starts_with("<!DOCTYPE") {
        body.find('>').map(|pos| body[pos + 1..].trim_start()).unwrap_or(body)
    } else {
        body
    };
    let has_root = body.starts_with("<article")
        || body.starts_with("<book")
        || body.starts_with("<chapter")
        || body.starts_with("<section")
        || body.starts_with("<part")
        || body.starts_with("<set")
        || body.starts_with("<reference")
        || body.starts_with("<preface")
        || body.starts_with("<appendix")
        || body.starts_with("<glossary")
        || body.starts_with("<bibliography")
        || body.starts_with("<index")
        || body.starts_with("<colophon")
        || body.starts_with("<dedication")
        || body.starts_with("<acknowledgements")
        || body.starts_with("<_root");
    if has_root {
        std::borrow::Cow::Borrowed(content)
    } else {
        std::borrow::Cow::Owned(format!("<_root>{}</_root>", content))
    }
}

/// Pre-scan DocBook content to build a map of element `id` (or `xml:id`) to the
/// title text of the nearest enclosing element that declares that id.
///
/// Used to resolve `<xref linkend="...">` and empty `<link linkend="...">`
/// elements to human-readable text: DocBook cross-references carry no text of
/// their own, so the reader is expected to see the target's title (or the
/// `xreflabel` override) substituted in at render time.
fn collect_id_titles(content: &str, budget: &mut SecurityBudget) -> Result<HashMap<String, String>> {
    let wrapped = ensure_root_element(content);
    let mut reader = EntityReader::from_str(&wrapped);
    let mut map = HashMap::new();
    let mut id_stack: Vec<Option<String>> = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow).to_string();

                let mut id_attr = None;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                    if key == "id" || key == "xml:id" {
                        id_attr = Some(val.to_string());
                    }
                }

                if tag == "title" {
                    let text = extract_element_text(&mut reader, budget)?;
                    if !text.is_empty()
                        && let Some(id) = id_stack.iter().rev().flatten().next()
                    {
                        map.entry(id.clone()).or_insert(text);
                    }
                } else {
                    id_stack.push(id_attr);
                }
            }
            Ok(Event::Empty(e)) => {
                budget.enter()?;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                }
                budget.leave();
            }
            Ok(Event::End(_)) => {
                budget.leave();
                id_stack.pop();
            }
            Ok(Event::Text(t)) => {
                let s = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&s)?;
                budget.account_text(s.trim().len())?;
            }
            Ok(Event::CData(t)) => {
                let s = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&s)?;
                budget.account_text(s.trim().len())?;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(map)
}

/// Resolve a `<xref>` / empty `<link>` cross-reference to display text.
///
/// Priority: an explicit `xreflabel` attribute wins, then the target
/// element's title (from `xref_titles`), then a `[linkend]` fallback so the
/// reference is never silently dropped.
fn resolve_xref_text(linkend: Option<&str>, xreflabel: Option<&str>, xref_titles: &HashMap<String, String>) -> String {
    if let Some(label) = xreflabel {
        return label.to_string();
    }
    if let Some(id) = linkend {
        if let Some(title) = xref_titles.get(id) {
            return title.clone();
        }
        return format!("[{}]", id);
    }
    String::new()
}

/// Build an `InternalDocument` from DocBook XML content.
fn build_docbook_internal_document(
    content: &str,
    inject_placeholders: bool,
    budget: &mut SecurityBudget,
) -> Result<InternalDocument> {
    let mut id_budget = budget.clone();
    let xref_titles = collect_id_titles(content, &mut id_budget)?;

    let wrapped = ensure_root_element(content);
    let mut reader = EntityReader::from_str(&wrapped);
    let mut builder = InternalDocumentBuilder::new("docbook");

    let mut title_extracted = false;
    let mut in_info = false;
    let mut in_table = false;
    let mut in_tgroup = false;
    let mut in_thead = false;
    let mut in_tbody = false;
    let mut in_row = false;
    let mut current_table: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut section_depth: u8 = 0;
    let mut title_depth: u8 = 0;
    let mut footnote_counter: u32 = 0;
    let mut in_list = false;
    let mut list_ordered = false;
    let mut in_variablelist = false;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                let mut language_attr: Option<String> = None;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                    if key == "language" {
                        language_attr = Some(val.to_string());
                    }
                }

                match tag {
                    "info" | "articleinfo" | "bookinfo" | "chapterinfo" => {
                        in_info = true;
                    }
                    "chapter" | "sect1" | "sect2" | "sect3" | "sect4" | "sect5" | "section" => {
                        section_depth = section_depth.saturating_add(1);
                    }
                    "title" if !title_extracted && in_info => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_heading(1, &text, None, None);
                            title_depth = section_depth;
                            title_extracted = true;
                        }
                    }
                    "title" if !title_extracted => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_heading(1, &text, None, None);
                            title_depth = section_depth;
                            title_extracted = true;
                        }
                    }
                    "title" if title_extracted => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            let relative = section_depth.saturating_sub(title_depth);
                            let level = std::cmp::min(relative.saturating_add(1), 6);
                            builder.push_heading(level, &text, None, None);
                        }
                    }
                    "para" | "simpara" => {
                        let (text, annotations) = extract_para_with_annotations(&mut reader, budget, &xref_titles)?;
                        if !text.is_empty() {
                            for ann in &annotations {
                                if let crate::types::document_structure::AnnotationKind::Link { url, .. } = &ann.kind
                                    && !url.is_empty()
                                {
                                    let label = text.get(ann.start as usize..ann.end as usize).map(|s| s.to_string());
                                    builder.push_uri(ExtractedUri::hyperlink(url, label));
                                }
                            }
                            builder.push_paragraph(&text, annotations, None, None);
                        }
                    }
                    "programlisting" | "screen" => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_code(&text, language_attr.as_deref(), None, None);
                        }
                    }
                    "itemizedlist" => {
                        in_list = true;
                        list_ordered = false;
                        builder.push_list(false);
                    }
                    "orderedlist" => {
                        in_list = true;
                        list_ordered = true;
                        builder.push_list(true);
                    }
                    "listitem" if in_list => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_list_item(&text, list_ordered, vec![], None, None);
                        }
                    }
                    "variablelist" => {
                        in_variablelist = true;
                    }
                    "term" if in_variablelist => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_definition_term(&text, None);
                        }
                    }
                    "listitem" if in_variablelist => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_definition_description(&text, None);
                        }
                    }
                    "blockquote" => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            builder.push_quote_start();
                            builder.push_paragraph(&text, vec![], None, None);
                            builder.push_quote_end();
                        }
                    }
                    "note" | "warning" | "tip" | "caution" | "important" => {
                        let admonition_text = extract_element_text(&mut reader, budget)?;
                        if !admonition_text.is_empty() {
                            builder.push_admonition(tag, None, None);
                            builder.push_paragraph(&admonition_text, vec![], None, None);
                        }
                    }
                    "figure" => {
                        let caption = extract_figure_with_caption(&mut reader, budget)?;
                        if inject_placeholders {
                            if !caption.is_empty() {
                                builder.push_paragraph(&format!("[Figure: {}]", caption), vec![], None, None);
                            } else {
                                builder.push_paragraph("[Figure]", vec![], None, None);
                            }
                        }
                    }
                    "footnote" => {
                        let text = extract_element_text(&mut reader, budget)?;
                        if !text.is_empty() {
                            footnote_counter += 1;
                            let key = format!("fn-{}", footnote_counter);
                            builder.push_footnote_definition(&text, &key, None);
                        }
                    }
                    "table" | "informaltable" => {
                        in_table = true;
                        current_table.clear();
                    }
                    "tgroup" if in_table => {
                        in_tgroup = true;
                    }
                    "thead" if in_tgroup => {
                        in_thead = true;
                    }
                    "tbody" if in_tgroup => {
                        in_tbody = true;
                    }
                    "row" if (in_thead || in_tbody) && in_tgroup => {
                        in_row = true;
                        current_row.clear();
                    }
                    "entry" if in_row => {
                        let text = extract_element_text(&mut reader, budget)?;
                        current_row.push(text);
                    }
                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                match tag {
                    "info" | "articleinfo" | "bookinfo" | "chapterinfo" => {
                        in_info = false;
                    }
                    "chapter" | "sect1" | "sect2" | "sect3" | "sect4" | "sect5" | "section" => {
                        section_depth = section_depth.saturating_sub(1);
                    }
                    "itemizedlist" | "orderedlist" if in_list => {
                        builder.end_list();
                        in_list = false;
                    }
                    "variablelist" if in_variablelist => {
                        in_variablelist = false;
                    }
                    "table" | "informaltable" if in_table => {
                        if !current_table.is_empty() {
                            builder.push_table_from_cells(&current_table, None, None);
                            current_table.clear();
                        }
                        in_table = false;
                    }
                    "tgroup" if in_tgroup => {
                        in_tgroup = false;
                    }
                    "thead" if in_thead => {
                        in_thead = false;
                    }
                    "tbody" if in_tbody => {
                        in_tbody = false;
                    }
                    "row" if in_row => {
                        if !current_row.is_empty() {
                            current_table.push(std::mem::take(&mut current_row));
                        }
                        in_row = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                let s = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&s)?;
                budget.account_text(s.trim().len())?;
            }
            Ok(Event::CData(t)) => {
                let s = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&s)?;
                budget.account_text(s.trim().len())?;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(builder.build())
}

/// Single-pass DocBook parser that extracts all content in one document traversal.
/// Returns: (content, title, author, date, tables, publisher, copyright)
fn parse_docbook_single_pass(content: &str, plain: bool, budget: &mut SecurityBudget) -> Result<DocBookParseResult> {
    let mut id_budget = budget.clone();
    let xref_titles = collect_id_titles(content, &mut id_budget)?;

    let wrapped = ensure_root_element(content);
    let mut reader = EntityReader::from_str(&wrapped);
    let mut output = String::new();
    let mut title = String::new();
    let mut author = Option::None;
    let mut date = Option::None;
    let mut publisher = Option::None;
    let mut copyright = Option::None;
    let mut tables = Vec::new();
    let mut table_index = 0;

    let mut state = ParsingState {
        in_info: false,
        in_table: false,
        in_tgroup: false,
        in_thead: false,
        in_tbody: false,
        in_row: false,
        in_list: false,
        in_list_item: false,
    };

    let mut title_extracted = false;
    let mut current_table: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut list_type = "";
    let mut in_variablelist = false;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                let mut language_attr: Option<String> = None;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                    if key == "language" {
                        language_attr = Some(val.to_string());
                    }
                }

                match tag {
                    "info" | "articleinfo" | "bookinfo" | "chapterinfo" => {
                        state.in_info = true;
                    }
                    "title" if !title_extracted && state.in_info => {
                        title = extract_element_text(&mut reader, budget)?;
                        title_extracted = true;
                    }
                    "title" if !title_extracted => {
                        title = extract_element_text(&mut reader, budget)?;
                        title_extracted = true;
                    }
                    "title" if title_extracted => {
                        let section_title = extract_element_text(&mut reader, budget)?;
                        if !section_title.is_empty() {
                            if !plain {
                                output.push_str("## ");
                            }
                            output.push_str(&section_title);
                            output.push_str("\n\n");
                        }
                    }
                    "author" | "personname" if state.in_info && author.is_none() => {
                        author = Some(extract_element_text(&mut reader, budget)?);
                    }
                    "date" if state.in_info && date.is_none() => {
                        let date_text = extract_element_text(&mut reader, budget)?;
                        if !date_text.is_empty() {
                            date = Some(date_text);
                        }
                    }
                    "publishername" if state.in_info && publisher.is_none() => {
                        let pub_text = extract_element_text(&mut reader, budget)?;
                        if !pub_text.is_empty() {
                            publisher = Some(pub_text);
                        }
                    }
                    "publisher" if state.in_info && publisher.is_none() => {
                        let pub_text = extract_element_text(&mut reader, budget)?;
                        if !pub_text.is_empty() {
                            publisher = Some(pub_text);
                        }
                    }
                    "copyright" if state.in_info && copyright.is_none() => {
                        let cr_text = extract_element_text(&mut reader, budget)?;
                        if !cr_text.is_empty() {
                            copyright = Some(cr_text);
                        }
                    }

                    "para" | "simpara" => {
                        let para_text = extract_element_text_with_inline(&mut reader, plain, budget, &xref_titles)?;
                        if !para_text.is_empty() {
                            output.push_str(&para_text);
                            output.push_str("\n\n");
                        }
                    }

                    "programlisting" | "screen" => {
                        let code_text = extract_element_text(&mut reader, budget)?;
                        if !code_text.is_empty() {
                            if !plain {
                                output.push_str("```");
                                if let Some(lang) = language_attr.as_deref() {
                                    output.push_str(lang);
                                }
                                output.push('\n');
                            }
                            output.push_str(&code_text);
                            if !plain {
                                output.push_str("\n```");
                            }
                            output.push_str("\n\n");
                        }
                    }

                    "itemizedlist" => {
                        state.in_list = true;
                        list_type = "itemized";
                    }
                    "orderedlist" => {
                        state.in_list = true;
                        list_type = "ordered";
                    }
                    "listitem" if state.in_list => {
                        state.in_list_item = true;
                        if !plain {
                            let prefix = if list_type == "ordered" { "1. " } else { "- " };
                            output.push_str(prefix);
                        }
                        let item_text = extract_element_text(&mut reader, budget)?;
                        if !item_text.is_empty() {
                            output.push_str(&item_text);
                        }
                        output.push('\n');
                        state.in_list_item = false;
                    }
                    "variablelist" => {
                        in_variablelist = true;
                    }
                    "term" if in_variablelist => {
                        let term_text = extract_element_text(&mut reader, budget)?;
                        if !term_text.is_empty() {
                            if !plain {
                                output.push_str("**");
                                output.push_str(&term_text);
                                output.push_str("**: ");
                            } else {
                                output.push_str(&term_text);
                                output.push_str(": ");
                            }
                        }
                    }
                    "listitem" if in_variablelist => {
                        let def_text = extract_element_text(&mut reader, budget)?;
                        if !def_text.is_empty() {
                            output.push_str(&def_text);
                        }
                        output.push_str("\n\n");
                    }

                    "blockquote" => {
                        if !plain {
                            output.push_str("> ");
                        }
                        let quote_text = extract_element_text(&mut reader, budget)?;
                        if !quote_text.is_empty() {
                            output.push_str(&quote_text);
                        }
                        output.push_str("\n\n");
                    }

                    "note" | "warning" | "tip" | "caution" | "important" => {
                        let admonition_type = tag.to_string();
                        let admonition_text = extract_element_text(&mut reader, budget)?;
                        if !admonition_text.is_empty() {
                            if !plain {
                                let label = admonition_type[..1].to_uppercase() + &admonition_type[1..];
                                output.push_str(&format!("**{}:** ", label));
                            }
                            output.push_str(&admonition_text);
                            output.push_str("\n\n");
                        }
                    }

                    "figure" => {
                        let figure_text = extract_figure_with_caption(&mut reader, budget)?;
                        if !figure_text.is_empty() {
                            if !plain {
                                output.push_str("**Figure:** ");
                            } else {
                                output.push_str("Figure: ");
                            }
                            output.push_str(&figure_text);
                            output.push_str("\n\n");
                        }
                    }

                    "footnote" => {
                        output.push('[');
                        let footnote_text = extract_element_text(&mut reader, budget)?;
                        if !footnote_text.is_empty() {
                            output.push_str(&footnote_text);
                        }
                        output.push(']');
                    }

                    "table" | "informaltable" => {
                        state.in_table = true;
                        current_table.clear();
                    }
                    "tgroup" if state.in_table => {
                        state.in_tgroup = true;
                    }
                    "thead" if state.in_tgroup => {
                        state.in_thead = true;
                    }
                    "tbody" if state.in_tgroup => {
                        state.in_tbody = true;
                    }
                    "row" if (state.in_thead || state.in_tbody) && state.in_tgroup => {
                        state.in_row = true;
                        current_row.clear();
                    }
                    "entry" if state.in_row => {
                        let entry_text = extract_element_text(&mut reader, budget)?;
                        current_row.push(entry_text);
                    }

                    _ => {}
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                match tag {
                    "info" | "articleinfo" | "bookinfo" | "chapterinfo" => {
                        state.in_info = false;
                    }
                    "itemizedlist" | "orderedlist" if state.in_list => {
                        output.push('\n');
                        state.in_list = false;
                    }
                    "variablelist" if in_variablelist => {
                        in_variablelist = false;
                    }
                    "table" | "informaltable" if state.in_table => {
                        if !current_table.is_empty() {
                            let markdown = cells_to_markdown(&current_table);
                            if plain {
                                output.push_str(&cells_to_text(&current_table));
                            } else {
                                output.push_str(&markdown);
                            }
                            output.push('\n');
                            tables.push(Table {
                                cells: std::mem::take(&mut current_table),
                                markdown,
                                page_number: table_index + 1,
                                bounding_box: None,
                                ..Default::default()
                            });
                            table_index += 1;
                        }
                        state.in_table = false;
                    }
                    "tgroup" if state.in_tgroup => {
                        state.in_tgroup = false;
                    }
                    "thead" if state.in_thead => {
                        state.in_thead = false;
                    }
                    "tbody" if state.in_tbody => {
                        state.in_tbody = false;
                    }
                    "row" if state.in_row => {
                        if !current_row.is_empty() {
                            current_table.push(std::mem::take(&mut current_row));
                        }
                        state.in_row = false;
                    }
                    _ => {}
                }
            }
            Ok(Event::Text(t)) => {
                let s = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&s)?;
                budget.account_text(s.trim().len())?;
            }
            Ok(Event::CData(t)) => {
                let s = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&s)?;
                budget.account_text(s.trim().len())?;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    let mut final_output = output;
    if !title.is_empty() {
        final_output = format!("{}\n\n{}", title, final_output);
    }

    Ok((
        final_output.trim().to_string(),
        title,
        author,
        date,
        tables,
        publisher,
        copyright,
    ))
}

/// Extract text content with inline formatting from a DocBook element.
/// Handles `<emphasis>`, `<emphasis role="bold">`, `<literal>`, `<command>`,
/// `<link>`, and `<ulink>` elements.
fn extract_element_text_with_inline(
    reader: &mut EntityReader<'_>,
    plain: bool,
    budget: &mut SecurityBudget,
    xref_titles: &HashMap<String, String>,
) -> Result<String> {
    let mut text = String::new();
    let mut depth = 0;
    let mut emphasis_bold_stack: Vec<bool> = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                if !plain {
                    match tag {
                        "emphasis" => {
                            let mut is_bold = false;
                            for attr in e.attributes().flatten() {
                                let attr_name = String::from_utf8_lossy(attr.key.as_ref());
                                let val = String::from_utf8_lossy(attr.value.as_ref());
                                budget.check_attr(&attr_name, &val)?;
                                if attr_name == "role" && (val == "bold" || val == "strong") {
                                    is_bold = true;
                                }
                            }
                            emphasis_bold_stack.push(is_bold);
                            if is_bold {
                                text.push_str("**");
                            } else {
                                text.push('*');
                            }
                        }
                        "literal" | "command" => {
                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.as_ref());
                                let val = String::from_utf8_lossy(attr.value.as_ref());
                                budget.check_attr(&key, &val)?;
                            }
                            text.push('`');
                        }
                        "link" | "ulink" => {
                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.as_ref());
                                let val = String::from_utf8_lossy(attr.value.as_ref());
                                budget.check_attr(&key, &val)?;
                            }
                            text.push('[');
                        }
                        _ => {
                            for attr in e.attributes().flatten() {
                                let key = String::from_utf8_lossy(attr.key.as_ref());
                                let val = String::from_utf8_lossy(attr.value.as_ref());
                                budget.check_attr(&key, &val)?;
                            }
                        }
                    }
                } else {
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref());
                        let val = String::from_utf8_lossy(attr.value.as_ref());
                        budget.check_attr(&key, &val)?;
                    }
                }
                depth += 1;
            }
            Ok(Event::End(e)) => {
                if depth == 0 {
                    break;
                }
                budget.leave();
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                if !plain {
                    match tag {
                        "emphasis" => {
                            let is_bold = emphasis_bold_stack.pop().unwrap_or(false);
                            if is_bold {
                                text.push_str("**");
                            } else {
                                text.push('*');
                            }
                        }
                        "literal" | "command" => {
                            text.push('`');
                        }
                        "link" | "ulink" => {
                            text.push(']');
                        }
                        _ => {}
                    }
                }
                depth -= 1;
            }
            Ok(Event::Empty(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                let mut linkend: Option<String> = None;
                let mut xreflabel: Option<String> = None;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                    if key == "linkend" {
                        linkend = Some(val.to_string());
                    } else if key == "xreflabel" {
                        xreflabel = Some(val.to_string());
                    }
                }

                if tag == "xref" || tag == "link" {
                    let resolved = resolve_xref_text(linkend.as_deref(), xreflabel.as_deref(), xref_titles);
                    if !resolved.is_empty() {
                        budget.account_text(resolved.len())?;
                        if !text.is_empty() && !text.ends_with(' ') && !text.ends_with('\n') {
                            text.push(' ');
                        }
                        if !plain && tag == "link" {
                            text.push('[');
                            text.push_str(&resolved);
                            text.push(']');
                        } else {
                            text.push_str(&resolved);
                        }
                    }
                }
                budget.leave();
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&decoded)?;
                let trimmed = decoded.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    if !text.is_empty()
                        && !text.ends_with(' ')
                        && !text.ends_with('\n')
                        && !text.ends_with('*')
                        && !text.ends_with('`')
                        && !text.ends_with('[')
                    {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
            }
            Ok(Event::CData(t)) => {
                let decoded_str = utf8_validation::from_utf8(t.as_ref()).unwrap_or("");
                budget.check_entity(decoded_str)?;
                let trimmed = decoded_str.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

/// Extract figure element, capturing the `<title>` as the caption.
fn extract_figure_with_caption(reader: &mut EntityReader<'_>, budget: &mut SecurityBudget) -> Result<String> {
    let mut caption = String::new();
    let mut depth = 0;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                }

                if tag == "title" && depth == 0 {
                    caption = extract_element_text(reader, budget)?;
                } else {
                    depth += 1;
                }
            }
            Ok(Event::End(e)) => {
                budget.leave();
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                if tag == "figure" && depth == 0 {
                    break;
                }
                if depth > 0 {
                    depth -= 1;
                }
            }
            Ok(Event::Text(t)) if caption.is_empty() => {
                let decoded = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&decoded)?;
                let trimmed = decoded.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    caption.push_str(trimmed);
                }
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&decoded)?;
                budget.account_text(decoded.trim().len())?;
            }
            Ok(Event::CData(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&decoded)?;
                budget.account_text(decoded.trim().len())?;
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(caption)
}

/// Extract text and inline annotations from a DocBook `<para>` element.
///
/// Recognizes:
/// - `<emphasis>` → italic (or bold if `role="bold"` / `role="strong"`)
/// - `<literal>` / `<command>` → code
/// - `<link>` / `<ulink>` with href → link
/// - `<subscript>` → subscript
/// - `<superscript>` → superscript
fn extract_para_with_annotations(
    reader: &mut EntityReader<'_>,
    budget: &mut SecurityBudget,
    xref_titles: &HashMap<String, String>,
) -> Result<(String, Vec<crate::types::document_structure::TextAnnotation>)> {
    use crate::types::builder;

    let mut text = String::new();
    let mut annotations = Vec::new();
    let mut depth: u32 = 0;

    let mut inline_stack: Vec<(&'static str, u32, u32, Option<String>)> = Vec::new();

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                depth += 1;

                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                match tag {
                    "emphasis" => {
                        let mut role = String::new();
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "role" {
                                role = val.to_string();
                            }
                        }
                        let kind = if role == "bold" || role == "strong" {
                            "bold"
                        } else {
                            "italic"
                        };
                        inline_stack.push((kind, depth, text.len() as u32, None));
                    }
                    "literal" | "command" => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                        }
                        inline_stack.push(("code", depth, text.len() as u32, None));
                    }
                    "link" | "ulink" => {
                        let mut href = None;
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                            if key == "url" || key == "href" || key.ends_with(":href") || key == "linkend" {
                                href = Some(val.to_string());
                            }
                        }
                        inline_stack.push(("link", depth, text.len() as u32, href));
                    }
                    "subscript" => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                        }
                        inline_stack.push(("subscript", depth, text.len() as u32, None));
                    }
                    "superscript" => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                        }
                        inline_stack.push(("superscript", depth, text.len() as u32, None));
                    }
                    _ => {
                        for attr in e.attributes().flatten() {
                            let key = String::from_utf8_lossy(attr.key.as_ref());
                            let val = String::from_utf8_lossy(attr.value.as_ref());
                            budget.check_attr(&key, &val)?;
                        }
                    }
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break;
                }
                budget.leave();

                if let Some(&(kind, open_depth, start, ref href)) = inline_stack.last()
                    && open_depth == depth
                {
                    let end = text.len() as u32;
                    let actual_start = if (start as usize) < text.len() {
                        let span = &text[start as usize..end as usize];
                        let trimmed = span.trim_start();
                        end - trimmed.len() as u32
                    } else {
                        start
                    };
                    if end > actual_start {
                        let href_clone = href.clone();
                        let annotation = match kind {
                            "bold" => builder::bold(actual_start, end),
                            "italic" => builder::italic(actual_start, end),
                            "code" => builder::code(actual_start, end),
                            "subscript" => builder::subscript(actual_start, end),
                            "superscript" => builder::superscript(actual_start, end),
                            "link" => {
                                let url = href_clone.as_deref().unwrap_or("");
                                builder::link(actual_start, end, url, None)
                            }
                            _ => unreachable!(),
                        };
                        annotations.push(annotation);
                    }
                    inline_stack.pop();
                }

                depth -= 1;
            }
            Ok(Event::Empty(e)) => {
                budget.enter()?;
                let name = e.name();
                let tag_cow = crate::utils::xml_tag_name(name.as_ref());
                let tag = strip_namespace(&tag_cow);

                let mut linkend: Option<String> = None;
                let mut xreflabel: Option<String> = None;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                    if key == "linkend" {
                        linkend = Some(val.to_string());
                    } else if key == "xreflabel" {
                        xreflabel = Some(val.to_string());
                    }
                }

                if tag == "xref" || tag == "link" {
                    let resolved = resolve_xref_text(linkend.as_deref(), xreflabel.as_deref(), xref_titles);
                    if !resolved.is_empty() {
                        budget.account_text(resolved.len())?;
                        if !text.is_empty() && !text.ends_with(' ') && !text.ends_with('\n') {
                            text.push(' ');
                        }
                        let start = text.len() as u32;
                        text.push_str(&resolved);
                        let end = text.len() as u32;
                        if tag == "link" {
                            let url = linkend.clone().unwrap_or_default();
                            annotations.push(builder::link(start, end, &url, None));
                        }
                    }
                }
                budget.leave();
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&decoded)?;
                let trimmed = decoded.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    if !text.is_empty() && !text.ends_with(' ') && !text.ends_with('\n') {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
            }
            Ok(Event::CData(t)) => {
                let decoded_str = utf8_validation::from_utf8(t.as_ref()).unwrap_or("");
                budget.check_entity(decoded_str)?;
                let trimmed = decoded_str.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok((text.trim().to_string(), annotations))
}

/// Extract text content from a DocBook element and its children.
/// Used for extracting nested content within elements.
fn extract_element_text(reader: &mut EntityReader<'_>, budget: &mut SecurityBudget) -> Result<String> {
    let mut text = String::new();
    let mut depth = 0;

    loop {
        budget.step()?;
        match reader.read_event() {
            Ok(Event::Start(e)) => {
                budget.enter()?;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref());
                    let val = String::from_utf8_lossy(attr.value.as_ref());
                    budget.check_attr(&key, &val)?;
                }
                depth += 1;
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break;
                }
                budget.leave();
                depth -= 1;
            }
            Ok(Event::Text(t)) => {
                let decoded = String::from_utf8_lossy(t.as_ref());
                budget.check_entity(&decoded)?;
                let trimmed = decoded.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    if !text.is_empty() && !text.ends_with(' ') && !text.ends_with('\n') {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
            }
            Ok(Event::CData(t)) => {
                let decoded_str = utf8_validation::from_utf8(t.as_ref()).unwrap_or("");
                budget.check_entity(decoded_str)?;
                let trimmed = decoded_str.trim();
                if !trimmed.is_empty() {
                    budget.account_text(trimmed.len())?;
                    if !text.is_empty() {
                        text.push(' ');
                    }
                    text.push_str(trimmed);
                }
            }
            Ok(Event::Eof) => break,
            Err(e) => {
                return Err(crate::error::XbergError::parsing(format!("XML parsing error: {}", e)));
            }
            _ => {}
        }
    }

    Ok(text.trim().to_string())
}

impl Plugin for DocbookExtractor {
    fn name(&self) -> &str {
        "docbook-extractor"
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
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for DocbookExtractor {
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
        // Track the fallback: a non-UTF-8 source is decoded lossily and every byte the
        // decoder could not read is already U+FFFD before parsing starts (#171).
        let (docbook_content, decoded_lossily) = match utf8_validation::from_utf8(content) {
            Ok(valid) => (valid.to_string(), false),
            Err(_) => (String::from_utf8_lossy(content).into_owned(), true),
        };

        let mut budget = SecurityBudget::from_config(config);
        let (_extracted_content, title, author, date, _tables, publisher, copyright) =
            parse_docbook_single_pass(&docbook_content, true, &mut budget)?;

        let mut metadata = Metadata::default();
        let mut subject_parts = Vec::new();

        if !title.is_empty() {
            metadata.title = Some(title.clone());
            subject_parts.push(format!("Title: {}", title));
        }
        if let Some(ref author) = author {
            metadata.authors = Some(vec![author.clone()]);
            subject_parts.push(format!("Author: {}", author));
        }

        if !subject_parts.is_empty() {
            metadata.subject = Some(subject_parts.join("; "));
        }

        if let Some(date_val) = date {
            metadata.created_at = Some(date_val);
        }

        if let Some(pub_val) = publisher {
            metadata
                .additional
                .insert(std::borrow::Cow::Borrowed("publisher"), serde_json::json!(pub_val));
        }

        if let Some(cr_val) = copyright {
            metadata
                .additional
                .insert(std::borrow::Cow::Borrowed("copyright"), serde_json::json!(cr_val));
        }

        let inject_placeholders = config
            .images
            .as_ref()
            .map(|img| img.inject_placeholders)
            .unwrap_or(true);
        let mut budget2 = SecurityBudget::from_config(config);
        let mut doc = build_docbook_internal_document(&docbook_content, inject_placeholders, &mut budget2)?;
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;

        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut doc.processing_warnings,
                DOCBOOK_WARNING_SOURCE,
                "DocBook source",
            );
        }

        Ok(doc)
    }

    #[cfg(feature = "tokio-runtime")]
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, path, config),
            fields(
                extractor.name = self.name(),
            )
        )
    )]
    async fn extract_path(&self, path: &Path, mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        crate::core::path_resolver::extract_file_with_image_resolution(self, path, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/docbook+xml", "text/docbook"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docbook_extractor_plugin_interface() {
        let extractor = DocbookExtractor::new();
        assert_eq!(extractor.name(), "docbook-extractor");
        assert!(extractor.initialize().is_ok());
        assert!(extractor.shutdown().is_ok());
    }

    #[test]
    fn test_docbook_extractor_supported_mime_types() {
        let extractor = DocbookExtractor::new();
        let mime_types = extractor.supported_mime_types();
        assert_eq!(mime_types.len(), 2);
        assert!(mime_types.contains(&"application/docbook+xml"));
        assert!(mime_types.contains(&"text/docbook"));
    }

    #[test]
    fn test_docbook_extractor_priority() {
        let extractor = DocbookExtractor::new();
        assert_eq!(extractor.priority(), 50);
    }

    #[test]
    fn test_parse_simple_docbook() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE article PUBLIC "-//OASIS//DTD DocBook XML V4.4//EN"
"http://www.oasis-open.org/docbook/xml/4.4/docbookx.dtd">
<article>
  <title>Test Article</title>
  <para>Test content.</para>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (content, title, _, _, _, _, _) =
            parse_docbook_single_pass(docbook, false, &mut budget).expect("Parse failed");
        assert_eq!(title, "Test Article");
        assert!(content.contains("Test content"));
    }

    #[test]
    fn test_extract_docbook_tables_basic() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <table>
    <tgroup cols="2">
      <thead>
        <row>
          <entry>Col1</entry>
          <entry>Col2</entry>
        </row>
      </thead>
      <tbody>
        <row>
          <entry>Data1</entry>
          <entry>Data2</entry>
        </row>
      </tbody>
    </tgroup>
  </table>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (_, _, _, _, tables, _, _) =
            parse_docbook_single_pass(docbook, false, &mut budget).expect("Table extraction failed");
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].cells.len(), 2);
        assert_eq!(tables[0].cells[0], vec!["Col1", "Col2"]);
    }

    #[test]
    fn test_docbook_inline_formatting() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Test</title>
  <para>This has <emphasis>italic</emphasis> and <emphasis role="bold">bold</emphasis> text.</para>
  <para>Use <literal>code_here</literal> and <command>ls -la</command> commands.</para>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (content, _, _, _, _, _, _) = parse_docbook_single_pass(docbook, false, &mut budget).expect("Parse failed");
        assert!(content.contains("*italic*"), "expected italic markup, got: {content}");
        assert!(content.contains("**bold**"), "expected bold markup, got: {content}");
        assert!(content.contains("`code_here`"), "expected code markup, got: {content}");
        assert!(content.contains("`ls -la`"), "expected command markup, got: {content}");
    }

    #[test]
    fn test_docbook_admonitions() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Test</title>
  <note><para>This is a note.</para></note>
  <warning><para>This is a warning.</para></warning>
  <tip><para>This is a tip.</para></tip>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (content, _, _, _, _, _, _) = parse_docbook_single_pass(docbook, false, &mut budget).expect("Parse failed");
        assert!(
            content.contains("**Note:**"),
            "expected note admonition, got: {content}"
        );
        assert!(
            content.contains("**Warning:**"),
            "expected warning admonition, got: {content}"
        );
        assert!(content.contains("**Tip:**"), "expected tip admonition, got: {content}");
    }

    #[test]
    fn test_docbook_publisher_copyright() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <info>
    <title>Test Article</title>
    <author><personname>John Doe</personname></author>
    <publishername>O'Reilly Media</publishername>
    <copyright><year>2024</year><holder>John Doe</holder></copyright>
  </info>
  <para>Content.</para>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (_, _, _, _, _, publisher, copyright) =
            parse_docbook_single_pass(docbook, false, &mut budget).expect("Parse failed");
        assert_eq!(publisher, Some("O'Reilly Media".to_string()));
        assert!(copyright.is_some());
        assert!(copyright.unwrap().contains("2024"));
    }

    #[test]
    fn test_docbook_figure_caption() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Test</title>
  <figure>
    <title>Architecture Diagram</title>
    <mediaobject><imageobject><imagedata fileref="arch.png"/></imageobject></mediaobject>
  </figure>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (content, _, _, _, _, _, _) = parse_docbook_single_pass(docbook, false, &mut budget).expect("Parse failed");
        assert!(
            content.contains("Architecture Diagram"),
            "expected figure caption, got: {content}"
        );
    }

    #[test]
    fn test_docbook_links() {
        let docbook = r#"<?xml version="1.0" encoding="UTF-8"?>
<article>
  <title>Test</title>
  <para>Visit <ulink url="http://example.com">the site</ulink> for details.</para>
</article>"#;

        let mut budget = SecurityBudget::with_defaults();
        let (content, _, _, _, _, _, _) = parse_docbook_single_pass(docbook, false, &mut budget).expect("Parse failed");
        assert!(content.contains("[the site]"), "expected link markup, got: {content}");
    }

    #[test]
    fn test_docbook_inject_placeholders_true() {
        let docbook = r#"<article>
  <figure>
    <title>Architecture Diagram</title>
    <mediaobject><imageobject><imagedata fileref="arch.png"/></imageobject></mediaobject>
  </figure>
</article>"#;
        let mut budget = SecurityBudget::with_defaults();
        let doc = build_docbook_internal_document(docbook, true, &mut budget).expect("parse failed");
        let has_figure = doc.elements.iter().any(|e| e.text.contains("[Figure"));
        assert!(has_figure, "expected figure placeholder with inject_placeholders=true");
    }

    #[test]
    fn test_docbook_inject_placeholders_false() {
        let docbook = r#"<article>
  <figure>
    <title>Architecture Diagram</title>
    <mediaobject><imageobject><imagedata fileref="arch.png"/></imageobject></mediaobject>
  </figure>
</article>"#;
        let mut budget = SecurityBudget::with_defaults();
        let doc = build_docbook_internal_document(docbook, false, &mut budget).expect("parse failed");
        let has_figure = doc.elements.iter().any(|e| e.text.contains("[Figure"));
        assert!(
            !has_figure,
            "expected no figure placeholder with inject_placeholders=false"
        );
    }
}
