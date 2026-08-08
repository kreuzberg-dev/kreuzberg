//! Redaction engine: orchestrates pattern matching, optional NER, span merging,
//! and the destructive rewrite of every textual field on [`ExtractedDocument`].
//!
//! The engine is invoked from the Late-stage post-processor at
//! [`crate::plugins::processor::builtin::redaction`].
//!
//! # One pass, every field
//!
//! A single `RedactionPass` carries the whole matcher set — pattern engine,
//! user-supplied terms and patterns, *and* the terms derived from NER
//! detections — and every text-bearing field is rewritten through it. The NER
//! backend only ever sees [`ExtractedDocument::content`], so its byte spans are
//! meaningless for a table cell or a metadata value; the detected mentions are
//! therefore also compiled into literal matchers that run against every field
//! and every occurrence (xberg-io/xberg#200, #202, #203).
//!
//! # Audit trail
//!
//! A [`RedactionFinding`] is recorded only once the replacement has actually
//! been applied to the field (xberg-io/xberg#201), and the report carries the
//! findings from *every* field, not just `content` (xberg-io/xberg#204).
//! `RedactionFinding::start`/`end` are byte offsets within the field the
//! finding came from — for `content` findings that is the original content, as
//! documented on the type; for a secondary field it is that field's own
//! pre-redaction text.

use std::collections::HashSet;

use crate::Result;
use crate::core::config::redaction::RedactionConfig;
use crate::types::ExtractedDocument;
use crate::types::entity::{Entity, EntityCategory};
use crate::types::metadata::FormatMetadata;
use crate::types::redaction::{PiiCategory, RedactionFinding, RedactionReport};
use crate::types::revisions::{DiffLine, RevisionAnchor};

use super::patterns::{PatternMatch, scan_text};
use super::strategy::{TokenCounter, apply_strategy};

/// Maximum nesting depth followed into embedded sub-documents (image OCR
/// results, archive members).
///
/// Redaction walks the same tree the extractor built; a hostile deeply-nested
/// archive must not be able to turn that walk into a stack overflow.
const MAX_NESTED_DOCUMENT_DEPTH: usize = 16;

/// Maximum nesting depth followed into djot block trees, for the same reason.
const MAX_BLOCK_NESTING_DEPTH: usize = 32;

/// Run pattern redaction (and optional NER-driven redaction) over `result` and
/// rewrite every textual field. Populates `result.redaction_report`.
pub async fn redact(result: &mut ExtractedDocument, config: &RedactionConfig) -> Result<()> {
    redact_counted(result, config).await.map(|_counter| ())
}

/// Like [`redact`], additionally returning the token to original-text map for
/// later rehydration. Only `RedactionStrategy::TokenReplace` allocations
/// appear in the map; `Mask`, `Hash`, and `Drop` are not reversible. The map
/// never touches disk here; encrypt it with
/// [`super::rehydration::encrypt_map`] and persistence stays with the caller.
#[cfg(feature = "redaction-rehydrate")]
#[cfg_attr(alef, alef(skip))]
pub async fn redact_capturing_rehydration_map(
    result: &mut ExtractedDocument,
    config: &RedactionConfig,
) -> Result<super::rehydration::RehydrationMap> {
    let counter = redact_counted(result, config).await?;
    Ok(counter.rehydration_map())
}

/// Redact `result` using `config` plus a caller-supplied entity stream, without
/// invoking a NER backend.
///
/// [`redact`] is this function with the entities the backend configured in
/// [`RedactionConfig::ner`] produced. Callers that already have an entity
/// stream — from `crate::enrich`, from the NER post-processor, or from their
/// own model — pass it here and skip the second inference pass. Entities of a
/// category the pattern engine already covers (email, phone, URL) are ignored:
/// the regex engine is the more reliable detector for those.
#[cfg_attr(alef, alef(skip))]
pub fn redact_with_entities(
    result: &mut ExtractedDocument,
    config: &RedactionConfig,
    entities: &[Entity],
) -> Result<()> {
    config.validate()?;
    redact_pass(result, config, entities);
    Ok(())
}

/// Shared body for [`redact`] and the map-capturing variant: runs the full
/// pass and hands back the token counter it used.
async fn redact_counted(result: &mut ExtractedDocument, config: &RedactionConfig) -> Result<TokenCounter> {
    config.validate()?;

    #[cfg(feature = "ner")]
    let entities: Vec<Entity> = match &config.ner {
        Some(ner_config) => collect_ner_entities(&result.content, ner_config, &active_categories(config)).await?,
        None => Vec::new(),
    };
    #[cfg(not(feature = "ner"))]
    let entities: Vec<Entity> = Vec::new();

    Ok(redact_pass(result, config, &entities))
}

/// Rewrite every text-bearing field on `result` and populate its audit report.
fn redact_pass(result: &mut ExtractedDocument, config: &RedactionConfig, entities: &[Entity]) -> TokenCounter {
    let active = active_categories(config);
    let categories: Vec<PiiCategory> = active.iter().cloned().collect();
    let custom_regexes = compile_custom(config);
    let ner_terms = compile_ner_terms(entities, config);

    let mut pass = RedactionPass {
        categories: &categories,
        config,
        custom_regexes: &custom_regexes,
        ner_terms: &ner_terms,
        counter: TokenCounter::new(),
        findings: Vec::new(),
    };

    pass.redact_document(result, 0);

    let findings = std::mem::take(&mut pass.findings);
    let total_redacted = findings.len() as u32;
    result.redaction_report = Some(RedactionReport {
        findings,
        total_redacted,
    });

    pass.counter
}

/// One document-wide redaction pass.
///
/// Owns the compiled matcher set, the strategy counter, and the audit findings
/// so that every field is rewritten by exactly the same matchers. Holding the
/// matchers here is what makes NER-derived terms reach fields other than
/// `content` (xberg-io/xberg#202).
struct RedactionPass<'a> {
    categories: &'a [PiiCategory],
    config: &'a RedactionConfig,
    custom_regexes: &'a [(String, regex::Regex)],
    ner_terms: &'a [(PiiCategory, regex::Regex)],
    counter: TokenCounter,
    findings: Vec<RedactionFinding>,
}

impl RedactionPass<'_> {
    /// Every match in `text`, deduped and in ascending byte order.
    fn matches_for(&self, text: &str) -> Vec<PatternMatch> {
        let mut matches = scan_text(text, self.categories);

        let custom = self
            .custom_regexes
            .iter()
            .map(|(label, regex)| (PiiCategory::Custom(label.clone()), regex));
        matches.extend(scan_regexes(text, custom));

        let detected = self.ner_terms.iter().map(|(category, regex)| (category.clone(), regex));
        matches.extend(scan_regexes(text, detected));

        if !self.config.categories.is_empty() {
            let requested = &self.config.categories;
            matches.retain(|m| matches!(m.category, PiiCategory::Custom(_)) || requested.contains(&m.category));
        }
        // A zero-length or inverted span carries no PII and would either insert a
        // stray token or underflow the interval dedupe. ~keep
        matches.retain(|m| m.start < m.end);
        dedupe_overlaps(matches)
    }

    /// Rewrite `text`, recording one finding per replacement that was actually
    /// applied.
    ///
    /// A match whose span is not a valid slice of `text`, or whose slice no
    /// longer holds the matched text, is skipped and produces no finding. The
    /// audit trail must never claim a redaction that did not happen — a false
    /// audit trail is worse than a visibly missing one (xberg-io/xberg#201).
    fn redact(&mut self, text: &str) -> String {
        let matches = self.matches_for(text);
        if matches.is_empty() {
            return text.to_string();
        }

        // Forward pass allocates replacements in ascending order, so TokenReplace
        // numbering still follows reading order; the rewrite then runs in reverse
        // so earlier byte offsets stay valid. ~keep
        let mut applied: Vec<(usize, usize, String)> = Vec::with_capacity(matches.len());
        for m in &matches {
            if !is_applicable(text, m) {
                continue;
            }
            let replacement = apply_strategy(self.config.strategy, &m.text, &m.category, &mut self.counter);
            self.findings.push(RedactionFinding {
                start: m.start as u32,
                end: m.end as u32,
                category: m.category.clone(),
                strategy: self.config.strategy,
                replacement_token: replacement.clone(),
            });
            applied.push((m.start, m.end, replacement));
        }

        let mut out = text.to_string();
        for (start, end, replacement) in applied.iter().rev() {
            out.replace_range(*start..*end, replacement);
        }
        out
    }

    /// Rewrite a string field in place.
    fn redact_in_place(&mut self, text: &mut String) {
        let redacted = self.redact(text);
        *text = redacted;
    }

    /// Rewrite an optional string field in place.
    fn redact_optional(&mut self, text: &mut Option<String>) {
        if let Some(text) = text.as_mut() {
            self.redact_in_place(text);
        }
    }

    /// Redact every string in a JSON value tree in place. Keys are left alone;
    /// only values are masked.
    fn redact_json_value(&mut self, value: &mut serde_json::Value) {
        match value {
            serde_json::Value::String(text) => self.redact_in_place(text),
            serde_json::Value::Array(items) => {
                for item in items.iter_mut() {
                    self.redact_json_value(item);
                }
            }
            serde_json::Value::Object(map) => {
                for value in map.values_mut() {
                    self.redact_json_value(value);
                }
            }
            _ => {}
        }
    }

    /// Rewrite every text surface of `doc`, recursing into embedded
    /// sub-documents up to [`MAX_NESTED_DOCUMENT_DEPTH`].
    fn redact_document(&mut self, doc: &mut ExtractedDocument, depth: usize) {
        let content = std::mem::take(&mut doc.content);
        doc.content = self.redact(&content);
        drop(content);

        if let Some(formatted) = doc.formatted_content.take() {
            doc.formatted_content = Some(self.redact(&formatted));
        }

        self.redact_chunks(doc);

        if let Some(entities) = doc.entities.as_mut() {
            for entity in entities.iter_mut() {
                self.redact_in_place(&mut entity.text);
            }
        }

        if let Some(summary) = doc.summary.as_mut() {
            self.redact_in_place(&mut summary.text);
        }

        if let Some(translation) = doc.translation.as_mut() {
            self.redact_in_place(&mut translation.content);
            self.redact_optional(&mut translation.formatted_content);
        }

        if let Some(pages) = doc.page_classifications.as_mut() {
            for page in pages.iter_mut() {
                for label in page.labels.iter_mut() {
                    self.redact_in_place(&mut label.label);
                }
            }
        }

        self.redact_secondary_text_fields(doc, depth);
    }

    /// Rewrite chunk text, keeping chunk byte ranges consistent when the
    /// caller asked for it.
    fn redact_chunks(&mut self, doc: &mut ExtractedDocument) {
        let Some(chunks) = doc.chunks.as_mut() else {
            return;
        };
        for chunk in chunks.iter_mut() {
            let original_len = chunk.content.len();
            self.redact_in_place(&mut chunk.content);
            let new_len = chunk.content.len();

            if self.config.preserve_offsets && new_len != original_len {
                let delta = new_len as isize - original_len as isize;
                let new_end = (chunk.metadata.byte_end as isize + delta).max(chunk.metadata.byte_start as isize);
                chunk.metadata.byte_end = new_end as usize;
            }

            for heading in chunk.metadata.heading_path.iter_mut() {
                self.redact_in_place(heading);
            }
            if let Some(context) = chunk.metadata.heading_context.as_mut() {
                for heading in context.headings.iter_mut() {
                    self.redact_in_place(&mut heading.text);
                }
            }
        }
    }

    /// Mask PII in every text-bearing output field beyond the primary
    /// content/chunk/entity set.
    ///
    /// This is an allowlist of *fields*, but it is meant to be exhaustive over
    /// the text surfaces of [`ExtractedDocument`]; when a new text field is
    /// added there it must be added here too.
    fn redact_secondary_text_fields(&mut self, doc: &mut ExtractedDocument, depth: usize) {
        self.redact_tables(doc);
        self.redact_pages(doc);
        self.redact_elements(doc);
        self.redact_djot(doc);
        self.redact_document_structure(doc);
        self.redact_revisions(doc);
        self.redact_nested_documents(doc, depth);
        self.redact_references(doc);
        self.redact_metadata(doc);
        self.redact_processing_warnings(doc);

        if let Some(structured) = doc.structured_output.as_mut() {
            self.redact_json_value(structured);
        }
        #[cfg(feature = "tree-sitter")]
        if let Some(code) = doc.code_intelligence.as_mut() {
            self.redact_json_value(code);
        }
    }

    /// Rewrite the document-level table cells and their rendered markdown.
    fn redact_tables(&mut self, doc: &mut ExtractedDocument) {
        for table in doc.tables.iter_mut() {
            for row in table.cells.iter_mut() {
                for cell in row.iter_mut() {
                    self.redact_in_place(cell);
                }
            }
            self.redact_in_place(&mut table.markdown);
        }
    }

    /// Rewrite per-page text, slide/sheet names, hierarchy blocks, and page tables.
    fn redact_pages(&mut self, doc: &mut ExtractedDocument) {
        let Some(pages) = doc.pages.as_mut() else {
            return;
        };
        for page in pages.iter_mut() {
            self.redact_in_place(&mut page.content);
            self.redact_optional(&mut page.speaker_notes);
            self.redact_optional(&mut page.section_name);
            self.redact_optional(&mut page.sheet_name);
            if let Some(hierarchy) = page.hierarchy.as_mut() {
                for block in hierarchy.blocks.iter_mut() {
                    self.redact_in_place(&mut block.text);
                }
            }
            for table in page.tables.iter_mut() {
                let table = std::sync::Arc::make_mut(table);
                for row in table.cells.iter_mut() {
                    for cell in row.iter_mut() {
                        self.redact_in_place(cell);
                    }
                }
                self.redact_in_place(&mut table.markdown);
            }
        }
    }

    /// Rewrite semantic elements, OCR elements, and formula source.
    fn redact_elements(&mut self, doc: &mut ExtractedDocument) {
        if let Some(elements) = doc.elements.as_mut() {
            for element in elements.iter_mut() {
                self.redact_in_place(&mut element.text);
                self.redact_optional(&mut element.metadata.filename);
                for value in element.metadata.additional.values_mut() {
                    self.redact_in_place(value);
                }
            }
        }
        if let Some(ocr_elements) = doc.ocr_elements.as_mut() {
            for element in ocr_elements.iter_mut() {
                self.redact_in_place(&mut element.text);
            }
        }
        for formula in doc.formulas.iter_mut() {
            self.redact_in_place(&mut formula.latex);
        }
    }

    /// Rewrite the structured document tree (`ExtractedDocument::document`).
    ///
    /// `NodeContent` carries the document body verbatim across every text-bearing
    /// node type; without this, a redaction pass leaves the structured tree
    /// holding everything the caller just asked to have hidden from `content`
    /// (xberg-io/xberg#298).
    fn redact_document_structure(&mut self, doc: &mut ExtractedDocument) {
        let Some(structure) = doc.document.as_mut() else {
            return;
        };
        for node in structure.nodes.iter_mut() {
            node.content.for_each_text_field_mut(|text| self.redact_in_place(text));
        }
    }

    /// Rewrite the djot representation: plain text, block tree, links, images,
    /// and footnotes.
    fn redact_djot(&mut self, doc: &mut ExtractedDocument) {
        let Some(djot) = doc.djot_content.as_mut() else {
            return;
        };
        self.redact_in_place(&mut djot.plain_text);
        for block in djot.blocks.iter_mut() {
            self.redact_djot_block(block, 0);
        }
        for link in djot.links.iter_mut() {
            self.redact_in_place(&mut link.url);
            self.redact_in_place(&mut link.text);
            self.redact_optional(&mut link.title);
        }
        for image in djot.images.iter_mut() {
            self.redact_in_place(&mut image.src);
            self.redact_in_place(&mut image.alt);
            self.redact_optional(&mut image.title);
        }
        for footnote in djot.footnotes.iter_mut() {
            self.redact_in_place(&mut footnote.label);
            for block in footnote.content.iter_mut() {
                self.redact_djot_block(block, 0);
            }
        }
    }

    /// Rewrite tracked-change text: the diff lines carry the document body
    /// verbatim, so an unredacted revision hands back what `content` just hid.
    fn redact_revisions(&mut self, doc: &mut ExtractedDocument) {
        let Some(revisions) = doc.revisions.as_mut() else {
            return;
        };
        for revision in revisions.iter_mut() {
            self.redact_optional(&mut revision.author);
            if let Some(RevisionAnchor::Sheet { name, .. }) = revision.anchor.as_mut() {
                self.redact_optional(name);
            }
            for line in revision.delta.content.iter_mut() {
                match line {
                    DiffLine::Context(text) | DiffLine::Added(text) | DiffLine::Removed(text) => {
                        self.redact_in_place(text);
                    }
                }
            }
            for change in revision.delta.table_changes.iter_mut() {
                self.redact_in_place(&mut change.from);
                self.redact_in_place(&mut change.to);
            }
            for change in revision.delta.property_changes.iter_mut() {
                self.redact_optional(&mut change.from);
                self.redact_optional(&mut change.to);
            }
        }
    }

    /// Rewrite embedded sub-documents: image OCR results and archive members.
    fn redact_nested_documents(&mut self, doc: &mut ExtractedDocument, depth: usize) {
        if let Some(images) = doc.images.as_mut() {
            for image in images.iter_mut() {
                self.redact_optional(&mut image.caption);
                self.redact_optional(&mut image.description);
                if let Some(ocr_doc) = image.ocr_result.as_mut()
                    && depth < MAX_NESTED_DOCUMENT_DEPTH
                {
                    self.redact_document(ocr_doc, depth + 1);
                }
            }
        }

        if let Some(children) = doc.children.as_mut() {
            for child in children.iter_mut() {
                self.redact_in_place(&mut child.path);
                if depth < MAX_NESTED_DOCUMENT_DEPTH {
                    self.redact_document(&mut child.result, depth + 1);
                }
            }
        }
    }

    /// Rewrite URIs, annotations, form fields, and extracted keywords.
    fn redact_references(&mut self, doc: &mut ExtractedDocument) {
        if let Some(uris) = doc.uris.as_mut() {
            for uri in uris.iter_mut() {
                self.redact_in_place(&mut uri.url);
                self.redact_optional(&mut uri.label);
            }
        }

        if let Some(annotations) = doc.annotations.as_mut() {
            for annotation in annotations.iter_mut() {
                self.redact_optional(&mut annotation.content);
            }
        }

        for field in doc.form_fields.iter_mut() {
            self.redact_in_place(&mut field.name);
            self.redact_optional(&mut field.value);
            self.redact_optional(&mut field.default_value);
            self.redact_optional(&mut field.tooltip);
        }

        #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
        if let Some(keywords) = doc.extracted_keywords.as_mut() {
            for keyword in keywords.iter_mut() {
                self.redact_in_place(&mut keyword.text);
            }
        }
    }

    /// Rewrite the free-text metadata surfaces.
    fn redact_metadata(&mut self, doc: &mut ExtractedDocument) {
        self.redact_optional(&mut doc.metadata.title);
        self.redact_optional(&mut doc.metadata.subject);
        self.redact_optional(&mut doc.metadata.created_by);
        self.redact_optional(&mut doc.metadata.modified_by);
        self.redact_optional(&mut doc.metadata.category);
        self.redact_optional(&mut doc.metadata.abstract_text);

        if let Some(authors) = doc.metadata.authors.as_mut() {
            for author in authors.iter_mut() {
                self.redact_in_place(author);
            }
        }
        if let Some(keywords) = doc.metadata.keywords.as_mut() {
            for keyword in keywords.iter_mut() {
                self.redact_in_place(keyword);
            }
        }
        if let Some(tags) = doc.metadata.tags.as_mut() {
            for tag in tags.iter_mut() {
                self.redact_in_place(tag);
            }
        }

        if let Some(format) = doc.metadata.format.as_mut() {
            self.redact_format_metadata(format);
        }

        // Format-specific metadata lands here as untyped JSON for several
        // extractors, so it has to be walked as a value tree. ~keep
        for value in doc.metadata.additional.values_mut() {
            self.redact_json_value(value);
        }
    }

    /// Rewrite the free-text surfaces of `Metadata::format`.
    ///
    /// `FormatMetadata` is a ~20-variant discriminated union and was not visited by
    /// any redaction matcher at all: `EmailMetadata` alone carries sender/recipient
    /// addresses and names verbatim (`from_email`, `from_name`, `to_emails`,
    /// `cc_emails`, `bcc_emails`), and several other variants carry free text or
    /// names (sheet names, archive file paths, HTML page metadata, bibliographic
    /// author lists, source code chunks) (xberg-io/xberg#299). Variants with no
    /// free-text field (page/row counts, codecs, PDF page geometry, etc.) are
    /// intentionally left as no-ops.
    fn redact_format_metadata(&mut self, format: &mut FormatMetadata) {
        match format {
            FormatMetadata::Excel(excel) => {
                if let Some(sheet_names) = excel.sheet_names.as_mut() {
                    for name in sheet_names.iter_mut() {
                        self.redact_in_place(name);
                    }
                }
            }
            FormatMetadata::Email(email) => {
                self.redact_optional(&mut email.from_email);
                self.redact_optional(&mut email.from_name);
                self.redact_optional(&mut email.message_id);
                for address in email.to_emails.iter_mut() {
                    self.redact_in_place(address);
                }
                for address in email.cc_emails.iter_mut() {
                    self.redact_in_place(address);
                }
                for address in email.bcc_emails.iter_mut() {
                    self.redact_in_place(address);
                }
                for attachment in email.attachments.iter_mut() {
                    self.redact_in_place(attachment);
                }
            }
            FormatMetadata::Archive(archive) => {
                for path in archive.file_list.iter_mut() {
                    self.redact_in_place(path);
                }
            }
            FormatMetadata::Text(text) => {
                self.redact_text_metadata_fields(text);
            }
            #[cfg(feature = "office")]
            FormatMetadata::Docx(docx) => {
                if let Some(core) = docx.core_properties.as_mut() {
                    self.redact_optional(&mut core.title);
                    self.redact_optional(&mut core.subject);
                    self.redact_optional(&mut core.creator);
                    self.redact_optional(&mut core.keywords);
                    self.redact_optional(&mut core.description);
                    self.redact_optional(&mut core.last_modified_by);
                }
                if let Some(app) = docx.app_properties.as_mut() {
                    self.redact_optional(&mut app.company);
                }
                if let Some(custom) = docx.custom_properties.as_mut() {
                    for value in custom.values_mut() {
                        self.redact_json_value(value);
                    }
                }
            }
            #[cfg(feature = "office")]
            FormatMetadata::Bibtex(bibtex) => {
                for author in bibtex.authors.iter_mut() {
                    self.redact_in_place(author);
                }
            }
            #[cfg(feature = "office")]
            FormatMetadata::Citation(citation) => {
                for author in citation.authors.iter_mut() {
                    self.redact_in_place(author);
                }
                for keyword in citation.keywords.iter_mut() {
                    self.redact_in_place(keyword);
                }
            }
            #[cfg(feature = "office")]
            FormatMetadata::FictionBook(fiction_book) => {
                self.redact_optional(&mut fiction_book.annotation);
            }
            #[cfg(feature = "xml")]
            FormatMetadata::Jats(jats) => {
                self.redact_optional(&mut jats.copyright);
                for contributor in jats.contributor_roles.iter_mut() {
                    self.redact_in_place(&mut contributor.name);
                }
            }
            FormatMetadata::Html(html) => {
                self.redact_html_metadata_fields(html);
            }
            #[cfg(feature = "tree-sitter")]
            FormatMetadata::Code(code) => {
                for chunk in code.chunks.iter_mut() {
                    self.redact_in_place(&mut chunk.text);
                }
                if let Some(data) = code.data.as_mut() {
                    self.redact_code_data_node(data);
                }
            }
            #[cfg(feature = "pdf")]
            FormatMetadata::Pdf(_) => {}
            // Slide titles are the direct analogue of `ExcelMetadata::sheet_names`
            // handled above — a deck routinely names people in them
            // ("Performance review — J. Smith").
            FormatMetadata::Pptx(pptx) => {
                for name in pptx.slide_names.iter_mut() {
                    self.redact_in_place(name);
                }
            }
            // EXIF values carry Artist, Copyright, camera-owner and GPS tags. Only the
            // values are redacted: the keys are EXIF tag names from a fixed vocabulary,
            // and rewriting them would corrupt the map without hiding anything.
            FormatMetadata::Image(image) => {
                for value in image.exif.values_mut() {
                    self.redact_in_place(value);
                }
            }
            // No string-bearing fields, or only format descriptors (delimiter, column
            // types, codec) that cannot carry document content.
            FormatMetadata::Xml(_) | FormatMetadata::Ocr(_) | FormatMetadata::Csv(_) | FormatMetadata::Pst(_) => {}
            #[cfg(feature = "office")]
            FormatMetadata::Dbf(_) | FormatMetadata::Epub(_) => {}
            #[cfg(feature = "transcription-types")]
            FormatMetadata::Audio(_) => {}
        }
    }

    /// Rewrite the free-text surfaces of `TextMetadata` (Markdown headers/links/code).
    fn redact_text_metadata_fields(&mut self, text: &mut crate::types::metadata::TextMetadata) {
        if let Some(headers) = text.headers.as_mut() {
            for header in headers.iter_mut() {
                self.redact_in_place(header);
            }
        }
        if let Some(links) = text.links.as_mut() {
            for (link_text, url) in links.iter_mut() {
                self.redact_in_place(link_text);
                self.redact_in_place(url);
            }
        }
        if let Some(code_blocks) = text.code_blocks.as_mut() {
            for (_language, code) in code_blocks.iter_mut() {
                self.redact_in_place(code);
            }
        }
    }

    /// Rewrite the free-text surfaces of `HtmlMetadata`: page-level metadata,
    /// social-card metadata, and every extracted header/link/image/structured-data
    /// element.
    fn redact_html_metadata_fields(&mut self, html: &mut crate::types::metadata::HtmlMetadata) {
        self.redact_optional(&mut html.title);
        self.redact_optional(&mut html.description);
        self.redact_optional(&mut html.author);
        self.redact_optional(&mut html.canonical_url);
        self.redact_optional(&mut html.base_href);
        for keyword in html.keywords.iter_mut() {
            self.redact_in_place(keyword);
        }
        for value in html.meta_tags.values_mut() {
            self.redact_in_place(value);
        }
        for value in html.open_graph.values_mut() {
            self.redact_in_place(value);
        }
        for value in html.twitter_card.values_mut() {
            self.redact_in_place(value);
        }
        for header in html.headers.iter_mut() {
            self.redact_in_place(&mut header.text);
        }
        for link in html.links.iter_mut() {
            self.redact_in_place(&mut link.href);
            self.redact_in_place(&mut link.text);
            self.redact_optional(&mut link.title);
        }
        for image in html.images.iter_mut() {
            self.redact_in_place(&mut image.src);
            self.redact_optional(&mut image.alt);
            self.redact_optional(&mut image.title);
        }
        for structured in html.structured_data.iter_mut() {
            self.redact_in_place(&mut structured.raw_json);
        }
    }

    /// Rewrite a code-format data-tree node (JSON/YAML/TOML/XML/CSV structural
    /// tree) and its children, bounded by [`MAX_BLOCK_NESTING_DEPTH`].
    #[cfg(feature = "tree-sitter")]
    fn redact_code_data_node(&mut self, node: &mut crate::types::metadata::CodeDataNode) {
        self.redact_code_data_node_at_depth(node, 0);
    }

    #[cfg(feature = "tree-sitter")]
    fn redact_code_data_node_at_depth(&mut self, node: &mut crate::types::metadata::CodeDataNode, depth: usize) {
        self.redact_optional(&mut node.value);
        if depth >= MAX_BLOCK_NESTING_DEPTH {
            return;
        }
        for child in node.children.iter_mut() {
            self.redact_code_data_node_at_depth(child, depth + 1);
        }
    }

    /// Rewrite processing-warning messages: they can embed source paths or other
    /// diagnostic detail carrying a person's name (e.g. a home-directory path).
    fn redact_processing_warnings(&mut self, doc: &mut ExtractedDocument) {
        for warning in doc.processing_warnings.iter_mut() {
            let redacted = self.redact(&warning.message);
            warning.message = std::borrow::Cow::Owned(redacted);
        }
    }

    /// Rewrite a djot block and its children, bounded by
    /// [`MAX_BLOCK_NESTING_DEPTH`].
    fn redact_djot_block(&mut self, block: &mut crate::types::djot::FormattedBlock, depth: usize) {
        for inline in block.inline_content.iter_mut() {
            self.redact_in_place(&mut inline.content);
        }
        self.redact_optional(&mut block.code);
        if depth >= MAX_BLOCK_NESTING_DEPTH {
            return;
        }
        for child in block.children.iter_mut() {
            self.redact_djot_block(child, depth + 1);
        }
    }
}

/// A match is applied only when its span is a valid slice of `text` that still
/// holds exactly the matched text.
///
/// NER backends report offsets the engine did not compute itself; a stale or
/// shifted span must be skipped rather than rewriting an unrelated region of
/// the document — and must not be reported as redacted either.
fn is_applicable(text: &str, m: &PatternMatch) -> bool {
    m.start < m.end
        && m.end <= text.len()
        && text.is_char_boundary(m.start)
        && text.is_char_boundary(m.end)
        && &text[m.start..m.end] == m.text.as_str()
}

/// Compute the set of categories the engine will consider during this run.
fn active_categories(config: &RedactionConfig) -> HashSet<PiiCategory> {
    if config.categories.is_empty() {
        let mut s: HashSet<PiiCategory> = [
            PiiCategory::Email,
            PiiCategory::Phone,
            PiiCategory::Ssn,
            PiiCategory::CreditCard,
            PiiCategory::PostalCode,
            PiiCategory::IpAddress,
            PiiCategory::Iban,
            PiiCategory::SwiftBic,
        ]
        .into_iter()
        .collect();
        if config.ner.is_some() {
            s.insert(PiiCategory::Person);
            s.insert(PiiCategory::Organization);
            s.insert(PiiCategory::Location);
        }
        s
    } else {
        config.categories.clone()
    }
}

/// Compile every user-supplied term and pattern once. Returns `(label, regex)`
/// tuples in declaration order — terms first, then patterns.
///
/// Regex compilation has already been validated by
/// [`RedactionConfig::validate`]; this function silently skips malformed inputs
/// so a residual stray pattern can't crash the engine.
fn compile_custom(config: &RedactionConfig) -> Vec<(String, regex::Regex)> {
    let mut out: Vec<(String, regex::Regex)> =
        Vec::with_capacity(config.custom_terms.len() + config.custom_patterns.len());

    for term in &config.custom_terms {
        if term.value.is_empty() {
            continue;
        }
        let escaped = regex::escape(&term.value);
        let pattern_str = if term.case_sensitive {
            escaped
        } else {
            format!("(?i){escaped}")
        };
        if let Ok(regex) = regex::Regex::new(&pattern_str) {
            out.push((term.label.clone(), regex));
        }
    }

    for pattern in &config.custom_patterns {
        if pattern.pattern.is_empty() {
            continue;
        }
        let pattern_str = if pattern.case_sensitive {
            pattern.pattern.clone()
        } else {
            format!("(?i){}", pattern.pattern)
        };
        if let Ok(regex) = regex::Regex::new(&pattern_str) {
            out.push((pattern.label.clone(), regex));
        }
    }

    out
}

/// Compile NER detections into literal matchers applied to every field.
///
/// The backend only sees `ExtractedDocument::content`, so its byte spans cannot
/// address a table cell, a chunk, or a metadata value, and matching by span
/// alone also stops at the *first* mention inside `content` itself. Matching
/// the detected mention as a word-boundary-anchored literal instead redacts it
/// wherever it appears (xberg-io/xberg#200, #202).
///
/// Custom NER labels are honoured (xberg-io/xberg#203), but only when the
/// caller actually asked for that label: a backend is free to invent a category
/// string, and an allowlist keeps an invented one from silently widening
/// redaction.
fn compile_ner_terms(entities: &[Entity], config: &RedactionConfig) -> Vec<(PiiCategory, regex::Regex)> {
    let mut allowed_custom: HashSet<String> = config
        .ner
        .as_ref()
        .map(|ner| {
            ner.custom_labels
                .iter()
                .map(|l| l.trim().to_ascii_lowercase())
                .collect()
        })
        .unwrap_or_default();
    for category in &config.categories {
        if let PiiCategory::Custom(label) = category {
            allowed_custom.insert(label.trim().to_ascii_lowercase());
        }
    }

    let mut seen: HashSet<(PiiCategory, String)> = HashSet::new();
    let mut out: Vec<(PiiCategory, regex::Regex)> = Vec::new();
    for entity in entities {
        let Some(category) = redactable_category(&entity.category, &allowed_custom) else {
            continue;
        };
        let mention = entity.text.trim();
        if mention.is_empty() {
            continue;
        }
        if !seen.insert((category.clone(), mention.to_string())) {
            continue;
        }
        if let Some(regex) = literal_regex(mention) {
            out.push((category, regex));
        }
    }
    out
}

/// Map an NER category onto the PII category it redacts as, or `None` when the
/// category is not NER-redactable.
///
/// Email / Phone / Url deliberately return `None`: the pattern engine detects
/// those more reliably and already covers every field.
fn redactable_category(category: &EntityCategory, allowed_custom: &HashSet<String>) -> Option<PiiCategory> {
    match category {
        EntityCategory::Person => Some(PiiCategory::Person),
        EntityCategory::Organization => Some(PiiCategory::Organization),
        EntityCategory::Location => Some(PiiCategory::Location),
        EntityCategory::Custom(label) => {
            let label = label.trim();
            if label.is_empty() || !allowed_custom.contains(&label.to_ascii_lowercase()) {
                return None;
            }
            Some(PiiCategory::Custom(label.to_string()))
        }
        EntityCategory::Date
        | EntityCategory::Time
        | EntityCategory::Money
        | EntityCategory::Percent
        | EntityCategory::Email
        | EntityCategory::Phone
        | EntityCategory::Url => None,
    }
}

/// Word-boundary-anchored literal matcher for a detected mention.
///
/// Anchors are only added where the mention actually starts or ends on a word
/// character, so mentions wrapped in punctuation still match. Matching is
/// case-sensitive on purpose: a case-insensitive match on a short name would
/// redact ordinary words ("Bill" would eat every "bill"), and destroying the
/// document is not an acceptable price for redacting it.
fn literal_regex(mention: &str) -> Option<regex::Regex> {
    let escaped = regex::escape(mention);
    let prefix = if mention.chars().next().is_some_and(is_word_char) {
        r"\b"
    } else {
        ""
    };
    let suffix = if mention.chars().next_back().is_some_and(is_word_char) {
        r"\b"
    } else {
        ""
    };
    regex::Regex::new(&format!("{prefix}{escaped}{suffix}")).ok()
}

fn is_word_char(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Scan `text` with pre-compiled `(category, regex)` pairs.
fn scan_regexes<'a>(text: &str, matchers: impl Iterator<Item = (PiiCategory, &'a regex::Regex)>) -> Vec<PatternMatch> {
    let mut out = Vec::new();
    for (category, regex) in matchers {
        for m in regex.find_iter(text) {
            out.push(PatternMatch {
                start: m.start(),
                end: m.end(),
                category: category.clone(),
                text: m.as_str().to_string(),
            });
        }
    }
    out
}

/// Pick the highest-priority match among overlapping spans.
///
/// Strategy: walk matches in (start, -length) order; keep a match only if its
/// start is at or after the previously-kept end. This is a standard interval
/// dedupe that prefers earlier and longer spans.
fn dedupe_overlaps(mut matches: Vec<PatternMatch>) -> Vec<PatternMatch> {
    if matches.is_empty() {
        return matches;
    }
    matches.sort_by(|a, b| a.start.cmp(&b.start).then((b.end - b.start).cmp(&(a.end - a.start))));
    let mut kept: Vec<PatternMatch> = Vec::with_capacity(matches.len());
    for m in matches {
        if let Some(last) = kept.last()
            && m.start < last.end
        {
            continue;
        }
        kept.push(m);
    }
    kept
}

/// Detect entities in `text` with the configured NER backend.
///
/// Custom labels alone are enough to justify the call: a caller can ask for
/// `custom_labels` without asking for PERSON / ORGANIZATION / LOCATION, and
/// skipping the backend in that case meant custom labels were never redacted
/// (xberg-io/xberg#203).
#[cfg(feature = "ner")]
async fn collect_ner_entities(
    text: &str,
    ner_config: &crate::core::config::ner::NerConfig,
    active: &HashSet<PiiCategory>,
) -> Result<Vec<Entity>> {
    let want_person = active.contains(&PiiCategory::Person);
    let want_organization = active.contains(&PiiCategory::Organization);
    let want_location = active.contains(&PiiCategory::Location);
    let want_custom = ner_config.custom_labels.iter().any(|label| !label.trim().is_empty());
    if !(want_person || want_organization || want_location || want_custom) {
        return Ok(Vec::new());
    }

    let mut categories: Vec<EntityCategory> = Vec::new();
    if want_person {
        categories.push(EntityCategory::Person);
    }
    if want_organization {
        categories.push(EntityCategory::Organization);
    }
    if want_location {
        categories.push(EntityCategory::Location);
    }

    let backend = make_ner_backend(ner_config)?;
    backend
        .detect_with_custom(text, &categories, &ner_config.custom_labels)
        .await
}

#[cfg(feature = "ner")]
fn make_ner_backend(
    config: &crate::core::config::ner::NerConfig,
) -> Result<std::sync::Arc<dyn crate::text::ner::NerBackend>> {
    use crate::core::config::ner::NerBackendKind;

    match config.backend {
        NerBackendKind::Onnx => {
            #[cfg(feature = "ner-onnx")]
            {
                Ok(crate::text::ner::gline::get_or_init_backend(config.model.as_deref())?)
            }
            #[cfg(not(feature = "ner-onnx"))]
            {
                Err(crate::XbergError::MissingDependency(
                    "ner-onnx feature is not enabled — rebuild xberg with --features ner-onnx".to_string(),
                ))
            }
        }
        NerBackendKind::Llm => {
            #[cfg(all(feature = "ner-llm", not(all(target_os = "android", target_arch = "x86_64"))))]
            {
                let llm = config.llm.clone().ok_or_else(|| {
                    crate::XbergError::validation("Llm NER backend selected but NerConfig.llm is None".to_string())
                })?;
                let backend = crate::text::ner::llm::LlmBackend::new(llm);
                Ok(std::sync::Arc::new(backend))
            }
            #[cfg(not(all(feature = "ner-llm", not(all(target_os = "android", target_arch = "x86_64")))))]
            {
                Err(crate::XbergError::MissingDependency(
                    "ner-llm feature is not enabled — rebuild xberg with --features ner-llm".to_string(),
                ))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(category: EntityCategory, text: &str, start: u32, end: u32) -> Entity {
        Entity {
            category,
            text: text.to_string(),
            start,
            end,
            confidence: Some(0.99),
        }
    }

    #[test]
    fn test_dedupe_overlaps_keeps_longer_first() {
        let matches = vec![
            PatternMatch {
                start: 0,
                end: 10,
                category: PiiCategory::Email,
                text: "long@x.com".into(),
            },
            PatternMatch {
                start: 5,
                end: 8,
                category: PiiCategory::Phone,
                text: "555".into(),
            },
        ];
        let kept = dedupe_overlaps(matches);
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].category, PiiCategory::Email);
    }

    #[test]
    fn is_applicable_rejects_a_span_that_no_longer_holds_its_text() {
        let text = "Contact Ada Lovelace today";
        let stale = PatternMatch {
            start: 0,
            end: 7,
            category: PiiCategory::Person,
            text: "Ada Lovelace".into(),
        };
        assert!(!is_applicable(text, &stale));

        let out_of_range = PatternMatch {
            start: 100,
            end: 112,
            category: PiiCategory::Person,
            text: "Ada Lovelace".into(),
        };
        assert!(!is_applicable(text, &out_of_range));

        let good = PatternMatch {
            start: 8,
            end: 20,
            category: PiiCategory::Person,
            text: "Ada Lovelace".into(),
        };
        assert!(is_applicable(text, &good));
    }

    #[test]
    fn literal_regex_anchors_on_word_boundaries() {
        let regex = literal_regex("Bill").expect("compiles");
        assert!(regex.is_match("Bill signed"));
        assert!(!regex.is_match("Billingsgate"));
        assert!(!regex.is_match("bill"), "matching must stay case-sensitive");
    }

    #[test]
    fn redactable_category_requires_an_allowlisted_custom_label() {
        let allowed: HashSet<String> = ["treatment".to_string()].into_iter().collect();
        assert_eq!(
            redactable_category(&EntityCategory::Custom("Treatment".into()), &allowed),
            Some(PiiCategory::Custom("Treatment".into()))
        );
        assert_eq!(
            redactable_category(&EntityCategory::Custom("Invented".into()), &allowed),
            None
        );
        assert_eq!(
            redactable_category(&EntityCategory::Person, &allowed),
            Some(PiiCategory::Person)
        );
        assert_eq!(redactable_category(&EntityCategory::Email, &allowed), None);
    }

    /// The capturing variant returns exactly the TokenReplace substitutions:
    /// every token in the rewritten content maps back to the original PII.
    #[cfg(feature = "redaction-rehydrate")]
    #[tokio::test]
    async fn capture_returns_token_to_original_map() {
        let email = "alice@example.com";
        let phone = "+1-555-123-4567";
        let mut doc = ExtractedDocument {
            content: format!("Contact {email} or call {phone}. Again: {email}."),
            ..Default::default()
        };
        let config = RedactionConfig {
            strategy: crate::types::redaction::RedactionStrategy::TokenReplace,
            ..Default::default()
        };

        let map = redact_capturing_rehydration_map(&mut doc, &config)
            .await
            .expect("capture must succeed");

        assert!(
            !doc.content.contains(email),
            "content still holds the email: {}",
            doc.content
        );
        assert_eq!(
            map.values().filter(|v| v.as_str() == email).count(),
            1,
            "repeated originals must dedupe to one token: {map:?}"
        );
        let mut rehydrated = doc.content.clone();
        for (token, original) in &map {
            rehydrated = rehydrated.replace(token, original);
        }
        assert!(
            rehydrated.contains(email) && rehydrated.contains(phone),
            "rehydrated: {rehydrated}"
        );
    }

    /// Regression for xberg-io/xberg#1223: redaction must mask PII on every
    /// structured surface, not just `content`.
    #[tokio::test]
    async fn redacts_every_text_bearing_field() {
        use crate::types::form_field::PdfFormField;
        use crate::types::uri::{ExtractedUri, UriKind};

        let email = "alice@example.com";
        let mut doc = ExtractedDocument {
            content: format!("Contact {email} for details."),
            tables: vec![crate::types::tables::Table {
                cells: vec![vec!["Name".into(), email.into()]],
                markdown: format!("| Name | {email} |"),
                page_number: 1,
                bounding_box: None,
                ..Default::default()
            }],
            pages: Some(vec![crate::types::PageContent {
                page_number: 1,
                content: format!("Page mentions {email}."),
                tables: Vec::new(),
                image_indices: Vec::new(),
                hierarchy: None,
                is_blank: None,
                layout_regions: None,
                speaker_notes: None,
                section_name: None,
                sheet_name: None,
            }]),
            uris: Some(vec![ExtractedUri {
                url: format!("mailto:{email}"),
                label: Some(email.into()),
                page: None,
                kind: UriKind::Email,
            }]),
            form_fields: vec![PdfFormField {
                name: "applicant_email".into(),
                full_name: "form.applicant_email".into(),
                field_type: crate::types::form_field::FormFieldType::Text,
                value: Some(email.into()),
                default_value: None,
                flags: 0,
                page: None,
                bbox: None,
                max_length: None,
                tooltip: None,
            }],
            structured_output: Some(serde_json::json!({ "email": email })),
            ..Default::default()
        };
        doc.metadata.subject = Some(format!("Re: {email}"));
        doc.metadata.created_by = Some(email.to_string());

        let config = RedactionConfig::default();
        redact(&mut doc, &config).await.expect("redaction must succeed");

        let mut leaks: Vec<&str> = Vec::new();
        if doc.content.contains(email) {
            leaks.push("content");
        }
        if doc.tables[0].cells.iter().flatten().any(|c| c.contains(email)) || doc.tables[0].markdown.contains(email) {
            leaks.push("tables");
        }
        if doc.pages.as_ref().unwrap()[0].content.contains(email) {
            leaks.push("pages");
        }
        let uri = &doc.uris.as_ref().unwrap()[0];
        if uri.url.contains(email) || uri.label.as_deref().unwrap_or("").contains(email) {
            leaks.push("uris");
        }
        if doc.form_fields[0].value.as_deref().unwrap_or("").contains(email) {
            leaks.push("form_fields");
        }
        if doc.metadata.subject.as_deref().unwrap_or("").contains(email) {
            leaks.push("metadata.subject");
        }
        if doc.metadata.created_by.as_deref().unwrap_or("").contains(email) {
            leaks.push("metadata.created_by");
        }
        if doc.structured_output.as_ref().unwrap().to_string().contains(email) {
            leaks.push("structured_output");
        }
        assert!(leaks.is_empty(), "PII leaked on fields: {leaks:?}");
    }

    /// xberg-io/xberg#200 — every occurrence of an NER mention must be redacted,
    /// not just the first.
    #[test]
    fn redacts_every_occurrence_of_an_ner_mention() {
        let mut doc = ExtractedDocument {
            content: "Zarnak Quorlim signed. Later Zarnak Quorlim paid. Zarnak Quorlim left.".to_string(),
            ..Default::default()
        };
        let entities = vec![entity(EntityCategory::Person, "Zarnak Quorlim", 0, 14)];

        redact_with_entities(&mut doc, &RedactionConfig::default(), &entities).expect("redaction must succeed");

        assert_eq!(doc.content.matches("[REDACTED]").count(), 3, "content: {}", doc.content);
        assert!(!doc.content.contains("Zarnak Quorlim"), "content: {}", doc.content);
    }

    /// xberg-io/xberg#204 — the report must count findings from every field.
    #[test]
    fn report_counts_findings_from_secondary_fields() {
        let mut doc = ExtractedDocument {
            content: "Zarnak Quorlim signed.".to_string(),
            ..Default::default()
        };
        doc.metadata.title = Some("Zarnak Quorlim".to_string());
        let entities = vec![entity(EntityCategory::Person, "Zarnak Quorlim", 0, 14)];

        redact_with_entities(&mut doc, &RedactionConfig::default(), &entities).expect("redaction must succeed");

        let report = doc.redaction_report.expect("report present");
        assert_eq!(report.total_redacted, 2);
        assert_eq!(report.findings.len(), 2);
    }
}
