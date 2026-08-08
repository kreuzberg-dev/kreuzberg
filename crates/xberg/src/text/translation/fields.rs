//! Translate every remaining text-bearing field of an [`ExtractedDocument`]
//! beyond `content`, `formatted_content`, and chunk content (which
//! [`super::llm::translate_result`] already handles directly).
//!
//! Before this module existed, `translate_result` only rewrote `content`,
//! `formatted_content`, and `Chunk::content`; tables, pages, metadata, the
//! structured document tree, and semantic elements were left in the source
//! language, so a "translated" document silently returned untranslated text
//! in most of its fields (xberg-io/xberg#254).
//!
//! # Call-volume discipline
//!
//! Translation is LLM-backed, so one call per string field would make call
//! count scale with field count (hundreds of table cells, elements, or
//! structure nodes). Every collection here is instead flattened into a list
//! of strings and translated through [`translate_batch`], which groups up to
//! [`MAX_BATCH_ITEMS`] segments into a single JSON-schema-constrained LLM
//! call (`complete_with_json_schema`, asking for a same-length JSON array
//! back). Concretely, for one document this module issues:
//! - 1 call per table (cells, batched) + 1 call per table (markdown) — tables
//!   are typically few and markdown must preserve table syntax, which the
//!   plain-text cell batch call does not.
//! - 1 call per page (page content) + 1 call per page for the small
//!   speaker-notes/section-name/sheet-name bundle (only when at least one is
//!   present) + 1 call per page for hierarchy blocks (batched) + 2 calls per
//!   page-level table (same table treatment as above).
//! - `ceil(element_count / MAX_BATCH_ITEMS)` calls for `elements`.
//! - `ceil(node_count / MAX_BATCH_ITEMS)` calls for the structured document
//!   tree (only when the `redaction` feature is enabled — see the note on
//!   [`translate_document_structure`]).
//! - 1 call for the metadata title/subject/category/abstract bundle, plus 1
//!   call each for `keywords` and `tags` (only when non-empty).
//!
//! Fields left untranslated on purpose: `metadata.authors`,
//! `metadata.created_by`, and `metadata.modified_by` are person names, not
//! prose — translating a name is not a meaningful operation and risks
//! corrupting an identity field. Format-specific metadata (email addresses,
//! archive file paths, spreadsheet sheet identifiers under
//! `Metadata::format`) is likewise left alone as it is not narrative text.

use crate::core::config::TranslationConfig;
use crate::types::{DocumentStructure, Element, ExtractedDocument, LlmUsage, Metadata, PageContent, Table};

use super::llm::translate_segment;

/// Maximum number of text segments sent to the LLM in a single translation
/// batch call.
///
/// Bounds prompt size when translating many short strings (table cells,
/// elements, structure-tree leaves) so a single [`translate_batch`] call
/// never grows the prompt unboundedly on a large document.
const MAX_BATCH_ITEMS: usize = 25;

/// Default Jinja2 template for batched LLM translation: translates a JSON
/// array of independent segments and expects a same-length JSON array back.
const DEFAULT_TRANSLATION_BATCH_TEMPLATE: &str = "\
You are a precise translation engine. You will receive a JSON array of independent text segments.
Translate each segment {% if source_lang and source_lang != 'auto' %}from {{ source_lang }} {% endif %}\
into {{ target_lang }}.

Rules:
- Preserve the original meaning of each segment exactly.
- Translate every array element independently; never merge, reorder, or drop elements.
- Return a JSON array with exactly the same number of elements, in the same order.
- Do not add commentary or explanations — respond with the JSON array only.
{% if preserve_markup %}- Preserve Markdown formatting (headings, lists, emphasis, links, code blocks) and HTML tags exactly as they appear.\
{% else %}- Return plain text only for each segment.{% endif %}
- If a segment is already in {{ target_lang }}, return it unchanged.
- If a segment is empty, return an empty string for it.

Segments (JSON array):
{{ segments }}";

/// Render the batch-translation prompt for `texts`.
fn render_batch_prompt(config: &TranslationConfig, texts: &[String], preserve_markup: bool) -> crate::Result<String> {
    let segments = serde_json::to_string(texts)
        .map_err(|e| crate::XbergError::parsing(format!("failed to encode translation batch as JSON: {e}")))?;
    let ctx = minijinja::context! {
        target_lang => &config.target_lang,
        source_lang => config.source_lang.as_deref().unwrap_or("auto"),
        preserve_markup => preserve_markup,
        segments => segments,
    };
    crate::llm::prompts::render_template(DEFAULT_TRANSLATION_BATCH_TEMPLATE, &ctx)
}

/// Translate one group of at most [`MAX_BATCH_ITEMS`] non-empty segments with
/// a single LLM call, and verify the response carries exactly as many
/// segments back as were sent.
async fn translate_batch_call(
    config: &TranslationConfig,
    texts: &[String],
    preserve_markup: bool,
    source_label: &str,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<Vec<String>> {
    let prompt = render_batch_prompt(config, texts, preserve_markup)?;
    let schema = serde_json::json!({
        "type": "array",
        "items": { "type": "string" },
        "minItems": texts.len(),
        "maxItems": texts.len(),
    });

    let (value, usage) = crate::llm::structured::complete_with_json_schema(
        &config.llm,
        &prompt,
        "translated_segments",
        &schema,
        source_label,
    )
    .await?;
    if let Some(u) = usage {
        usages.push(u);
    }

    let translated: Vec<String> = serde_json::from_value(value).map_err(|e| {
        crate::XbergError::parsing(format!(
            "LLM translation batch ({source_label}) did not return a JSON array of strings: {e}"
        ))
    })?;
    if translated.len() != texts.len() {
        return Err(crate::XbergError::parsing(format!(
            "LLM translation batch ({source_label}) returned {} segment(s), expected {}",
            translated.len(),
            texts.len()
        )));
    }
    Ok(translated)
}

/// Translate `texts` in place order, skipping empty/whitespace-only entries
/// (left unchanged, never sent to the LLM) and grouping the rest into
/// [`MAX_BATCH_ITEMS`]-sized calls.
async fn translate_batch(
    config: &TranslationConfig,
    texts: &[String],
    preserve_markup: bool,
    source_label: &str,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<Vec<String>> {
    let mut output = texts.to_vec();
    let non_empty_indices: Vec<usize> = texts
        .iter()
        .enumerate()
        .filter(|(_, text)| !text.trim().is_empty())
        .map(|(index, _)| index)
        .collect();

    for group in non_empty_indices.chunks(MAX_BATCH_ITEMS) {
        let group_texts: Vec<String> = group.iter().map(|&index| texts[index].clone()).collect();
        let translated = translate_batch_call(config, &group_texts, preserve_markup, source_label, usages).await?;
        for (&index, translated_text) in group.iter().zip(translated) {
            output[index] = translated_text;
        }
    }

    Ok(output)
}

/// Translate a fixed set of optional string fields (e.g. metadata's
/// title/subject/category/abstract) as a single batch call, skipping fields
/// that are `None` entirely (as opposed to translating an empty string).
async fn translate_optional_fields(
    config: &TranslationConfig,
    mut fields: Vec<&mut Option<String>>,
    source_label: &str,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    let present: Vec<usize> = fields
        .iter()
        .enumerate()
        .filter(|(_, field)| field.is_some())
        .map(|(index, _)| index)
        .collect();
    if present.is_empty() {
        return Ok(());
    }

    let texts: Vec<String> = present
        .iter()
        .map(|&index| fields[index].clone().unwrap_or_default())
        .collect();
    let translated = translate_batch(config, &texts, false, source_label, usages).await?;

    for (slot, translated_text) in present.into_iter().zip(translated) {
        *fields[slot] = Some(translated_text);
    }
    Ok(())
}

/// Translate every element of a `Vec<String>` metadata field (`keywords`,
/// `tags`) as a single batch call, a no-op when absent or empty.
async fn translate_string_list(
    config: &TranslationConfig,
    list: &mut Option<Vec<String>>,
    source_label: &str,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    let Some(items) = list.as_mut() else {
        return Ok(());
    };
    if items.is_empty() {
        return Ok(());
    }
    let translated = translate_batch(config, items, false, source_label, usages).await?;
    *items = translated;
    Ok(())
}

/// Translate one table's cells (batched, plain text) and its rendered
/// markdown (its own call, with markup preservation forced on since the
/// markdown IS table syntax that must survive verbatim).
async fn translate_table(
    table: &mut Table,
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    let mut positions: Vec<(usize, usize)> = Vec::new();
    let mut texts: Vec<String> = Vec::new();
    for (row_index, row) in table.cells.iter().enumerate() {
        for (col_index, cell) in row.iter().enumerate() {
            positions.push((row_index, col_index));
            texts.push(cell.clone());
        }
    }
    if !texts.is_empty() {
        let translated = translate_batch(config, &texts, false, "translation_table_cells", usages).await?;
        for ((row_index, col_index), value) in positions.into_iter().zip(translated) {
            table.cells[row_index][col_index] = value;
        }
    }

    table.markdown = translate_segment(config, &table.markdown, true, "translation_table_markdown", usages).await?;
    Ok(())
}

async fn translate_tables(
    tables: &mut [Table],
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    for table in tables.iter_mut() {
        translate_table(table, config, usages).await?;
    }
    Ok(())
}

/// Translate per-page content, the speaker-notes/section-name/sheet-name
/// bundle, hierarchy block text, and any page-level tables.
async fn translate_pages(
    pages: Option<&mut Vec<PageContent>>,
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    let Some(pages) = pages else {
        return Ok(());
    };

    for page in pages.iter_mut() {
        page.content = translate_segment(config, &page.content, false, "translation_page_content", usages).await?;

        translate_optional_fields(
            config,
            vec![&mut page.speaker_notes, &mut page.section_name, &mut page.sheet_name],
            "translation_page_labels",
            usages,
        )
        .await?;

        if let Some(hierarchy) = page.hierarchy.as_mut() {
            let texts: Vec<String> = hierarchy.blocks.iter().map(|block| block.text.clone()).collect();
            if !texts.is_empty() {
                let translated = translate_batch(config, &texts, false, "translation_page_hierarchy", usages).await?;
                for (block, value) in hierarchy.blocks.iter_mut().zip(translated) {
                    block.text = value;
                }
            }
        }

        for table in page.tables.iter_mut() {
            let table = std::sync::Arc::make_mut(table);
            translate_table(table, config, usages).await?;
        }
    }
    Ok(())
}

/// Translate every semantic element's `text` field as a single batched pass.
async fn translate_elements(
    elements: Option<&mut Vec<Element>>,
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    let Some(elements) = elements else {
        return Ok(());
    };
    let texts: Vec<String> = elements.iter().map(|element| element.text.clone()).collect();
    if texts.is_empty() {
        return Ok(());
    }
    let translated = translate_batch(config, &texts, false, "translation_elements", usages).await?;
    for (element, value) in elements.iter_mut().zip(translated) {
        element.text = value;
    }
    Ok(())
}

/// Translate the narrative metadata fields (title, subject, category,
/// abstract) and the `keywords`/`tags` lists.
///
/// `authors`, `created_by`, and `modified_by` are intentionally skipped — see
/// the module-level doc comment.
async fn translate_metadata(
    metadata: &mut Metadata,
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    translate_optional_fields(
        config,
        vec![
            &mut metadata.title,
            &mut metadata.subject,
            &mut metadata.category,
            &mut metadata.abstract_text,
        ],
        "translation_metadata",
        usages,
    )
    .await?;
    translate_string_list(config, &mut metadata.keywords, "translation_metadata_keywords", usages).await?;
    translate_string_list(config, &mut metadata.tags, "translation_metadata_tags", usages).await?;
    Ok(())
}

/// Translate every leaf text field of the structured document tree
/// (`ExtractedDocument::document`), reusing
/// [`crate::types::document_structure::NodeContent::for_each_text_field_mut`] —
/// the same exhaustive-field walker the redaction engine uses — rather than
/// hand-rolling a second copy of the `NodeContent` match.
///
async fn translate_document_structure(
    document: Option<&mut DocumentStructure>,
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    let Some(document) = document else {
        return Ok(());
    };

    let mut texts: Vec<String> = Vec::new();
    for node in document.nodes.iter_mut() {
        node.content.for_each_text_field_mut(|text| texts.push(text.clone()));
    }
    if texts.is_empty() {
        return Ok(());
    }

    let translated = translate_batch(config, &texts, false, "translation_document_structure", usages).await?;
    let mut translated = translated.into_iter();
    for node in document.nodes.iter_mut() {
        node.content.for_each_text_field_mut(|text| {
            if let Some(value) = translated.next() {
                *text = value;
            }
        });
    }
    Ok(())
}

/// Translate every text-bearing field of `result` beyond `content`,
/// `formatted_content`, and chunk content (xberg-io/xberg#254).
pub(super) async fn translate_secondary_fields(
    result: &mut ExtractedDocument,
    config: &TranslationConfig,
    usages: &mut Vec<LlmUsage>,
) -> crate::Result<()> {
    translate_tables(&mut result.tables, config, usages).await?;
    translate_pages(result.pages.as_mut(), config, usages).await?;
    translate_elements(result.elements.as_mut(), config, usages).await?;
    translate_metadata(&mut result.metadata, config, usages).await?;
    translate_document_structure(result.document.as_mut(), config, usages).await?;
    Ok(())
}
