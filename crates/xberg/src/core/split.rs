//! Split-and-extract for multi-document PDFs.
//!
//! A single PDF often concatenates several logical documents (a scanned batch of
//! invoices, letters, or forms). [`split_and_extract`] detects sub-document
//! boundaries and returns one [`ExtractedDocument`] per segment from a **single
//! parse** — the caller does not have to slice the PDF externally and re-parse
//! each slice.
//!
//! Two strategies are supported (see [`SplitStrategy`]):
//!
//! - [`SplitStrategy::PageRanges`] — caller-supplied 1-indexed inclusive page
//!   ranges. Deterministic; use when the boundaries are already known.
//! - [`SplitStrategy::Auto`] — heuristic boundary detection via
//!   [`crate::heuristics::multidoc`] (blank/letterhead/density/page-one signals).
//!   Requires the `heuristics` feature.
//!
//! This module is **not** part of the language-binding surface; it is core-Rust
//! only (see `split_and_extract` in `alef.toml` `[crates.exclude]`).

use std::borrow::Cow;
use std::ops::RangeInclusive;

use crate::core::config::{ExtractInput, ExtractionConfig};
use crate::error::XbergError;
use crate::types::{Chunk, DocumentCounts, ExtractedDocument, ProcessingWarning};
use crate::{Result, core};

/// How to partition a multi-document PDF before extraction.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub enum SplitStrategy {
    /// Heuristic boundary detection (page-one markers, letterhead resets, text
    /// density shifts). Requires the `heuristics` feature.
    #[default]
    Auto,
    /// Caller-supplied 1-indexed, inclusive page ranges.
    PageRanges(Vec<RangeInclusive<u32>>),
}

/// Configuration for [`split_and_extract`].
///
/// Construct with struct-update syntax, e.g.
/// `SplitConfig { strategy: SplitStrategy::PageRanges(vec![1..=3]), ..Default::default() }`,
/// or take the `Auto` default via `SplitConfig::default()`.
#[derive(Debug, Clone, Default)]
pub struct SplitConfig {
    /// Boundary-detection strategy.
    pub strategy: SplitStrategy,
    /// Extraction config applied to every segment. Per-page content extraction
    /// is forced on internally regardless of this value.
    pub extraction: ExtractionConfig,
}

/// One extracted sub-document plus the page span it came from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct SplitSegment {
    /// 1-indexed inclusive page range in the original document.
    pub page_range: RangeInclusive<u32>,
    /// The sub-document extracted from `page_range`.
    pub document: ExtractedDocument,
}

/// Detect boundaries in a single PDF and extract each segment from one parse.
///
/// The PDF is parsed once (with per-page content extraction forced on), then
/// partitioned into segments according to `config.strategy`. Page/table/image
/// attribution is preserved in the original page coordinate system.
///
/// # Errors
///
/// - [`XbergError::UnsupportedFormat`] if `bytes` is not a PDF (PDF is the only
///   page-addressable format supported today).
/// - [`XbergError::Validation`] for an empty document, invalid page ranges, or
///   `Auto` requested without the `heuristics` feature.
/// - Any error propagated from the underlying single-document extraction.
pub async fn split_and_extract(bytes: &[u8], config: &SplitConfig) -> Result<Vec<SplitSegment>> {
    let mime = core::mime::detect_mime_type_from_bytes(bytes)?;
    if mime != "application/pdf" {
        return Err(XbergError::UnsupportedFormat(format!(
            "split_and_extract currently supports PDF only, got {mime}"
        )));
    }

    let mut extraction = config.extraction.clone();
    extraction
        .pages
        .get_or_insert_with(core::config::PageConfig::default)
        .extract_pages = true;

    let input = ExtractInput::from_bytes(bytes.to_vec(), mime, None);
    let result = crate::extract(input, &extraction).await?;
    let doc = result
        .results
        .into_iter()
        .next()
        .ok_or_else(|| XbergError::validation("split_and_extract: extraction produced no document"))?;

    let total_pages = doc.pages.as_ref().map_or(0, Vec::len) as u32;
    if total_pages == 0 {
        return Err(XbergError::validation(
            "split_and_extract requires a page-addressable PDF with at least one page",
        ));
    }

    let ranges = match &config.strategy {
        SplitStrategy::PageRanges(ranges) => ranges_from_page_ranges(ranges, total_pages)?,
        SplitStrategy::Auto => auto_ranges(&doc, total_pages)?,
    };

    // Enrichment stages (chunking, keywords, NER, summarisation, quality scoring, LLM
    // calls, ...) all ran once, on the whole (pre-split) document. `sub_document_for_range`
    // decides, field by field, whether that single result can be partitioned by page or
    // must be treated as describing the whole source document; see its doc comment.
    let warnings = deduped_split_warnings(&doc.processing_warnings);

    Ok(ranges
        .iter()
        .enumerate()
        .map(|(index, range)| sub_document_for_range(&doc, range, index == 0, &warnings))
        .collect())
}

/// Validate and normalize caller-supplied page ranges (1-indexed, inclusive).
fn ranges_from_page_ranges(ranges: &[RangeInclusive<u32>], total_pages: u32) -> Result<Vec<RangeInclusive<u32>>> {
    if ranges.is_empty() {
        return Err(XbergError::validation(
            "SplitStrategy::PageRanges requires at least one page range",
        ));
    }
    let mut out = Vec::with_capacity(ranges.len());
    for range in ranges {
        let (start, end) = (*range.start(), *range.end());
        if start < 1 || start > end || end > total_pages {
            return Err(XbergError::validation(format!(
                "invalid page range {start}..={end} for a {total_pages}-page document \
                 (expected 1 <= start <= end <= {total_pages})"
            )));
        }
        out.push(start..=end);
    }
    Ok(out)
}

/// Derive segment ranges from heuristic boundary detection.
#[cfg(feature = "heuristics")]
fn auto_ranges(doc: &ExtractedDocument, total_pages: u32) -> Result<Vec<RangeInclusive<u32>>> {
    use crate::heuristics::multidoc::{MultidocThresholds, boundaries_from_extraction_result};

    let boundaries = boundaries_from_extraction_result(doc, &MultidocThresholds::default());
    Ok(fold_boundaries_into_ranges(&boundaries, total_pages))
}

/// Fold detected boundaries into contiguous, non-overlapping page ranges.
///
/// Each real boundary's `start_page` is the page where a new sub-document
/// begins; `Start`/`End` are sentinels and the leading `1` seeds the first
/// segment.
#[cfg(feature = "heuristics")]
fn fold_boundaries_into_ranges(
    boundaries: &[crate::heuristics::multidoc::DocumentBoundary],
    total_pages: u32,
) -> Vec<RangeInclusive<u32>> {
    use crate::heuristics::multidoc::BoundaryReason;

    let mut starts: Vec<u32> = boundaries
        .iter()
        .filter(|b| {
            matches!(
                b.reason,
                BoundaryReason::PageOneMarker | BoundaryReason::LetterheadReset | BoundaryReason::DensityShift
            )
        })
        .map(|b| b.start_page)
        .filter(|&p| (1..=total_pages).contains(&p))
        .collect();
    starts.push(1);
    starts.sort_unstable();
    starts.dedup();

    let mut ranges = Vec::with_capacity(starts.len());
    for (index, &start) in starts.iter().enumerate() {
        let end = starts.get(index + 1).map_or(total_pages, |&next| next - 1);
        if start <= end {
            ranges.push(start..=end);
        }
    }
    ranges
}

/// `Auto` requires the `heuristics` feature; fail clearly when it is absent.
#[cfg(not(feature = "heuristics"))]
fn auto_ranges(_doc: &ExtractedDocument, _total_pages: u32) -> Result<Vec<RangeInclusive<u32>>> {
    Err(XbergError::validation(
        "SplitStrategy::Auto requires the 'heuristics' feature; use SplitStrategy::PageRanges instead",
    ))
}

/// Build one sub-document for `range` by partitioning the already-parsed data by
/// page attribution. No re-parse: pages, tables, and images are filtered from
/// the single whole-document parse.
///
/// Every enrichment stage (chunking, keywords, NER, summarisation, quality scoring,
/// classification, LLM calls, ...) already ran exactly once against the whole
/// (pre-split) document, inside the single `crate::extract` call in
/// [`split_and_extract`]. There is no per-split re-run of any post-processor. So for
/// every field on [`ExtractedDocument`] this function has to decide how a *single*
/// whole-document result should be distributed across `N` segments:
///
/// - **Page-attributed collections** (`tables`, `images`, `annotations`, `formulas`,
///   `ocr_elements`, `page_classifications`, `chunks`, `elements`, `uris`,
///   `form_fields`) are filtered to the items whose page falls in `range`, mirroring
///   the pre-existing `tables`/`images` behaviour. Items whose page attribution is
///   optional and unset (e.g. a `form_field` with `page: None`) cannot be excluded
///   safely, so they are included in *every* segment rather than dropped.
/// - **Whole-document descriptive/scalar fields** (`detected_languages`,
///   `quality_score`, `summary`, `extraction_confidence`, `translation`,
///   `structured_output`, `document`, `djot_content`, `code_intelligence`,
///   `children`) describe the source document as a whole and cannot be
///   meaningfully split by page range (there is, for example, no sub-tree of
///   `DocumentStructure` scoped to a page range without walking that module's own
///   node/page mapping). They are cloned into every segment unchanged, the same way
///   `mime_type` already was.
/// - **Whole-document countable/resource lists** (`entities`, `extracted_keywords`,
///   `llm_usage`, `redaction_report`, `revisions`) carry byte offsets into the
///   *original* `content` (or, for `llm_usage`, represent LLM calls billed once for
///   the whole document) that would misrepresent the split's own `content` or double
///   count cost/PII findings if duplicated across segments. They are attached only to
///   the first segment (`is_first`); later segments get `None` rather than an
///   inflated or misattributed copy.
/// - **`processing_warnings`** are deduplicated once (see [`deduped_split_warnings`])
///   and attached to every segment, tagged to make clear they originate from the
///   pre-split extraction rather than per-segment processing.
/// - Pipeline-internal fields (`formatted_content`, `ocr_internal_document`,
///   `internal_document`) are transient implementation state already consumed by the
///   time `crate::extract` returns; they are intentionally left at their `Default`.
fn sub_document_for_range(
    source: &ExtractedDocument,
    range: &RangeInclusive<u32>,
    is_first: bool,
    warnings: &[ProcessingWarning],
) -> SplitSegment {
    let (start, end) = (*range.start(), *range.end());
    let in_range = |page: u32| (start..=end).contains(&page);

    let pages: Vec<crate::types::PageContent> = source
        .pages
        .as_deref()
        .unwrap_or(&[])
        .iter()
        .filter(|page| in_range(page.page_number))
        .cloned()
        .collect();

    let content = pages
        .iter()
        .map(|page| page.content.as_str())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");

    let tables = filter_by_page(&source.tables, start, end, |table| table.page_number);

    let images: Option<Vec<crate::types::ExtractedImage>> = source
        .images
        .as_ref()
        .map(|images| filter_by_optional_page(images, start, end, |image| image.page_number));

    let image_index_map = build_image_index_map(source.images.as_deref().unwrap_or(&[]), start, end);

    let mut metadata = source.metadata.clone();
    if let Some(page_structure) = metadata.pages.as_mut() {
        page_structure.total_count = pages.len() as u32;
    }

    let counts = DocumentCounts {
        pages: pages.len(),
        tables: tables.len(),
        images: images.as_ref().map_or(0, Vec::len),
    };

    let document = ExtractedDocument {
        content,
        mime_type: source.mime_type.clone(),
        metadata,
        extraction_method: source.extraction_method,
        tables,
        counts,
        // Whole-document descriptive/scalar fields: identical across every segment.
        detected_languages: source.detected_languages.clone(),
        quality_score: source.quality_score,
        summary: source.summary.clone(),
        #[cfg(feature = "heuristics")]
        extraction_confidence: source.extraction_confidence,
        translation: source.translation.clone(),
        structured_output: source.structured_output.clone(),
        document: source.document.clone(),
        djot_content: source.djot_content.clone(),
        #[cfg(feature = "tree-sitter")]
        code_intelligence: source.code_intelligence.clone(),
        children: source.children.clone(),
        // Deduplicated and tagged once for the whole document; identical on every segment.
        processing_warnings: warnings.to_vec(),
        // Whole-document countable/resource lists: first segment only, see doc comment.
        #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
        extracted_keywords: is_first.then(|| source.extracted_keywords.clone()).flatten(),
        entities: is_first.then(|| source.entities.clone()).flatten(),
        llm_usage: is_first.then(|| source.llm_usage.clone()).flatten(),
        redaction_report: is_first.then(|| source.redaction_report.clone()).flatten(),
        revisions: is_first.then(|| source.revisions.clone()).flatten(),
        // Page-attributed collections: filtered to `range`.
        images,
        pages: source.pages.is_some().then_some(pages),
        elements: source
            .elements
            .as_ref()
            .map(|elements| filter_by_optional_page(elements, start, end, |element| element.metadata.page_number)),
        ocr_elements: source
            .ocr_elements
            .as_ref()
            .map(|elements| filter_by_page(elements, start, end, |element| element.page_number)),
        annotations: source
            .annotations
            .as_ref()
            .map(|items| filter_by_page(items, start, end, |item| item.page_number)),
        uris: source
            .uris
            .as_ref()
            .map(|items| filter_by_optional_page(items, start, end, |item| item.page)),
        page_classifications: source
            .page_classifications
            .as_ref()
            .map(|items| filter_by_page(items, start, end, |item| item.page_number)),
        formulas: filter_by_page(&source.formulas, start, end, |formula| formula.page),
        form_fields: filter_by_optional_page(&source.form_fields, start, end, |field| field.page),
        chunks: source
            .chunks
            .as_ref()
            .map(|chunks| split_chunks(chunks, start, end, &image_index_map)),
        // Pipeline-internal/transient fields: not part of the enrichment contract, left default.
        ..Default::default()
    };

    SplitSegment {
        page_range: range.clone(),
        document,
    }
}

/// Keep only the items whose (required) page falls within `start..=end`.
fn filter_by_page<T: Clone>(items: &[T], start: u32, end: u32, page_of: impl Fn(&T) -> u32) -> Vec<T> {
    items
        .iter()
        .filter(|item| (start..=end).contains(&page_of(item)))
        .cloned()
        .collect()
}

/// Keep only the items whose page falls within `start..=end`. Items whose page
/// attribution is `None` cannot be excluded safely and are kept in every segment.
fn filter_by_optional_page<T: Clone>(items: &[T], start: u32, end: u32, page_of: impl Fn(&T) -> Option<u32>) -> Vec<T> {
    items
        .iter()
        .filter(|item| page_of(item).is_none_or(|page| (start..=end).contains(&page)))
        .cloned()
        .collect()
}

/// Map each image's position in the whole-document `images` vector to its position in
/// the filtered, split-local `images` vector (`None` if it did not survive filtering),
/// so that `Chunk::metadata::image_indices` can be remapped onto valid indices.
///
/// The survival predicate MUST stay identical to [`filter_by_optional_page`], which is what
/// actually builds the split-local vector: an image with no page attribution is kept in every
/// segment, so it occupies a slot here too. Numbering it `None` instead would leave every
/// later image's remapped index one too low, silently pointing each chunk at the wrong image.
fn build_image_index_map(images: &[crate::types::ExtractedImage], start: u32, end: u32) -> Vec<Option<u32>> {
    let mut next_index = 0u32;
    images
        .iter()
        .map(|image| {
            image
                .page_number
                .is_none_or(|page| (start..=end).contains(&page))
                .then(|| {
                    let assigned = next_index;
                    next_index += 1;
                    assigned
                })
        })
        .collect()
}

/// Whether a chunk overlaps `start..=end`. Chunks without page-tracking metadata
/// (`first_page`/`last_page` unset) cannot be attributed to a range and are treated
/// as overlapping every range rather than being silently dropped.
fn chunk_overlaps_range(chunk: &Chunk, start: u32, end: u32) -> bool {
    match (chunk.metadata.first_page, chunk.metadata.last_page) {
        (Some(first), Some(last)) => first <= end && last >= start,
        _ => true,
    }
}

/// Partition chunks by page overlap with `start..=end`, remap each surviving chunk's
/// `image_indices` onto the split-local `images` vector via `image_index_map`, drop
/// `page_spans` outside the range, and renumber `chunk_index`/`total_chunks` so they
/// describe this chunk's position within the split rather than the whole document.
fn split_chunks(chunks: &[Chunk], start: u32, end: u32, image_index_map: &[Option<u32>]) -> Vec<Chunk> {
    let mut out: Vec<Chunk> = chunks
        .iter()
        .filter(|chunk| chunk_overlaps_range(chunk, start, end))
        .cloned()
        .collect();

    let total_chunks = out.len();
    for (index, chunk) in out.iter_mut().enumerate() {
        chunk.metadata.chunk_index = index;
        chunk.metadata.total_chunks = total_chunks;
        chunk.metadata.image_indices = chunk
            .metadata
            .image_indices
            .iter()
            .filter_map(|&old_index| image_index_map.get(old_index as usize).copied().flatten())
            .collect();
        chunk
            .metadata
            .page_spans
            .retain(|span| (start..=end).contains(&span.page));
    }
    out
}

/// Deduplicate `processing_warnings` by `(source, message)` and tag each survivor's
/// `source` to make clear it was surfaced by the pre-split whole-document extraction
/// that `split_and_extract` partitions, not by per-segment processing.
fn deduped_split_warnings(warnings: &[ProcessingWarning]) -> Vec<ProcessingWarning> {
    let mut seen = std::collections::HashSet::new();
    warnings
        .iter()
        .filter(|warning| seen.insert((warning.source.clone(), warning.message.clone())))
        .map(|warning| ProcessingWarning {
            source: Cow::Owned(format!("split:{}", warning.source)),
            message: warning.message.clone(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::page::{PageContent, PageStructure, PageUnitType};
    use crate::types::{ExtractedImage, Metadata, Table};

    fn page(page_number: u32, content: &str) -> PageContent {
        PageContent {
            page_number,
            content: content.to_string(),
            tables: Vec::new(),
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        }
    }

    fn table(page_number: u32) -> Table {
        Table {
            page_number,
            ..Default::default()
        }
    }

    fn image(page_number: u32) -> ExtractedImage {
        ExtractedImage {
            page_number: Some(page_number),
            ..Default::default()
        }
    }

    /// A 5-page document: pages carry text; tables on pages 1 and 4; images on
    /// pages 2 and 5.
    fn sample_doc() -> ExtractedDocument {
        ExtractedDocument {
            content: "whole".to_string(),
            mime_type: "application/pdf".into(),
            metadata: Metadata {
                pages: Some(PageStructure {
                    total_count: 5,
                    unit_type: PageUnitType::Page,
                    boundaries: None,
                    pages: None,
                }),
                ..Default::default()
            },
            tables: vec![table(1), table(4)],
            images: Some(vec![image(2), image(5)]),
            pages: Some((1..=5).map(|n| page(n, &format!("page {n} text"))).collect()),
            ..Default::default()
        }
    }

    #[test]
    #[allow(clippy::reversed_empty_ranges)]
    fn page_ranges_validation_rejects_out_of_bounds() {
        assert!(ranges_from_page_ranges(&[1..=6], 5).is_err(), "end beyond page count");
        assert!(ranges_from_page_ranges(&[0..=2], 5).is_err(), "start below 1");
        assert!(ranges_from_page_ranges(&[3..=2], 5).is_err(), "start after end");
        assert!(ranges_from_page_ranges(&[], 5).is_err(), "empty range list");
        assert!(ranges_from_page_ranges(&[1..=3, 4..=5], 5).is_ok());
    }

    #[test]
    fn sub_document_partitions_pages_tables_images_by_range() {
        let doc = sample_doc();
        let segment = sub_document_for_range(&doc, &(1..=3), true, &[]);

        assert_eq!(segment.page_range, 1..=3);
        let out = &segment.document;
        let page_numbers: Vec<u32> = out.pages.as_ref().unwrap().iter().map(|p| p.page_number).collect();
        assert_eq!(page_numbers, vec![1, 2, 3]);
        assert_eq!(out.tables.len(), 1);
        assert_eq!(out.tables[0].page_number, 1);
        assert_eq!(out.images.as_ref().unwrap().len(), 1);
        assert_eq!(out.images.as_ref().unwrap()[0].page_number, Some(2));
        assert_eq!(out.content, "page 1 text\n\npage 2 text\n\npage 3 text");
        assert_eq!(out.counts.pages, 3);
        assert_eq!(out.counts.tables, 1);
        assert_eq!(out.counts.images, 1);
        assert_eq!(out.metadata.pages.as_ref().unwrap().total_count, 3);
    }

    #[test]
    fn sub_document_second_segment_covers_remaining_pages() {
        let doc = sample_doc();
        let segment = sub_document_for_range(&doc, &(4..=5), false, &[]);
        let out = &segment.document;
        assert_eq!(out.counts.pages, 2);
        assert_eq!(out.tables[0].page_number, 4);
        assert_eq!(out.images.as_ref().unwrap()[0].page_number, Some(5));
    }

    #[cfg(feature = "heuristics")]
    #[test]
    fn auto_ranges_single_document_yields_one_full_range() {
        let doc = sample_doc();
        let ranges = auto_ranges(&doc, 5).unwrap();
        assert_eq!(ranges, vec![1..=5]);
    }

    #[cfg(feature = "heuristics")]
    #[test]
    fn auto_ranges_folds_boundaries_into_contiguous_ranges() {
        use crate::heuristics::multidoc::{BoundaryReason, DocumentBoundary};

        let boundaries = vec![
            DocumentBoundary {
                start_page: 1,
                end_page: 1,
                confidence: 1.0,
                reason: BoundaryReason::Start,
            },
            DocumentBoundary {
                start_page: 3,
                end_page: 3,
                confidence: 0.9,
                reason: BoundaryReason::PageOneMarker,
            },
            DocumentBoundary {
                start_page: 5,
                end_page: 5,
                confidence: 0.85,
                reason: BoundaryReason::LetterheadReset,
            },
            DocumentBoundary {
                start_page: 8,
                end_page: 8,
                confidence: 1.0,
                reason: BoundaryReason::End,
            },
        ];
        let ranges = fold_boundaries_into_ranges(&boundaries, 8);
        assert_eq!(ranges, vec![1..=2, 3..=4, 5..=8]);
    }
}

/// Issue #210 — every enrichment field must survive `split_and_extract`.
///
/// These are unit tests rather than integration tests because the enrichment
/// fields under test (`entities`, `summary`, `translation`, `redaction_report`,
/// `llm_usage`, `page_classifications`, `code_intelligence`, …) are populated by
/// backends that need ONNX models, network access, or an LLM provider. They cannot
/// be produced deterministically from a fixture PDF through the public
/// `split_and_extract` entry point, and `sub_document_for_range` — the function that
/// owns the field-by-field distribution decision — is private to this module.
/// The reachable subset is covered end-to-end in
/// `tests/issue_210_split_and_extract_enrichment.rs`.
#[cfg(test)]
mod enrichment_preservation_tests {
    use std::collections::HashMap;

    use super::*;
    use crate::types::annotations::{PdfAnnotation, PdfAnnotationType};
    use crate::types::classification::{ClassificationLabel, PageClassification};
    use crate::types::djot::DjotContent;
    use crate::types::document_structure::DocumentStructure;
    use crate::types::entity::{Entity, EntityCategory};
    use crate::types::extraction::{
        ArchiveEntry, BoundingBox, Chunk, ChunkMetadata, ChunkType, Element, ElementId, ElementMetadata, ElementType,
        ExtractionMethod, LlmUsage, PageSpan,
    };
    use crate::types::form_field::{FormFieldType, PdfFormField};
    use crate::types::formula::Formula;
    use crate::types::ocr_elements::OcrElement;
    use crate::types::page::{PageContent, PageStructure, PageUnitType};
    use crate::types::redaction::{PiiCategory, RedactionFinding, RedactionReport, RedactionStrategy};
    use crate::types::revisions::{DocumentRevision, RevisionDelta, RevisionKind};
    use crate::types::summary::{DocumentSummary, SummaryStrategy};
    use crate::types::translation::Translation;
    use crate::types::uri::{ExtractedUri, UriKind};
    use crate::types::{ExtractedImage, Metadata, Table};

    fn page(page_number: u32) -> PageContent {
        PageContent {
            page_number,
            content: format!("page {page_number} text"),
            tables: Vec::new(),
            image_indices: Vec::new(),
            hierarchy: None,
            is_blank: None,
            layout_regions: None,
            speaker_notes: None,
            section_name: None,
            sheet_name: None,
        }
    }

    fn table(page_number: u32) -> Table {
        Table {
            page_number,
            ..Default::default()
        }
    }

    fn image(image_index: u32, page_number: Option<u32>) -> ExtractedImage {
        ExtractedImage {
            image_index,
            page_number,
            ..Default::default()
        }
    }

    fn element(text: &str, page_number: Option<u32>) -> Element {
        Element {
            element_id: ElementId::default(),
            element_type: ElementType::NarrativeText,
            text: text.to_string(),
            metadata: ElementMetadata {
                page_number,
                filename: None,
                coordinates: None,
                element_index: None,
                additional: HashMap::new(),
            },
        }
    }

    fn ocr_element(text: &str, page_number: u32) -> OcrElement {
        OcrElement {
            text: text.to_string(),
            page_number,
            ..Default::default()
        }
    }

    fn annotation(page_number: u32) -> PdfAnnotation {
        PdfAnnotation {
            annotation_type: PdfAnnotationType::Text,
            content: Some(format!("note on {page_number}")),
            page_number,
            bounding_box: None,
            author: None,
            modified: None,
            color: None,
            subject: None,
            quad_points: None,
            marked_text: None,
        }
    }

    fn formula(latex: &str, page: u32) -> Formula {
        Formula {
            latex: latex.to_string(),
            bbox: BoundingBox::default(),
            page,
        }
    }

    fn form_field(name: &str, page: Option<u32>) -> PdfFormField {
        PdfFormField {
            name: name.to_string(),
            full_name: format!("form.{name}"),
            field_type: FormFieldType::default(),
            value: None,
            default_value: None,
            flags: 0,
            page,
            bbox: None,
            max_length: None,
            tooltip: None,
        }
    }

    fn uri(url: &str, page: Option<u32>) -> ExtractedUri {
        ExtractedUri {
            url: url.to_string(),
            label: None,
            page,
            kind: UriKind::Hyperlink,
        }
    }

    fn classification(page_number: u32, label: &str) -> PageClassification {
        PageClassification {
            page_number,
            labels: vec![ClassificationLabel {
                label: label.to_string(),
                confidence: Some(0.5),
            }],
        }
    }

    fn warning(source: &'static str, message: &'static str) -> ProcessingWarning {
        ProcessingWarning {
            source: Cow::Borrowed(source),
            message: Cow::Borrowed(message),
        }
    }

    fn chunk(content: &str, pages: Option<(u32, u32)>, image_indices: Vec<u32>, spans: Vec<u32>) -> Chunk {
        Chunk {
            content: content.to_string(),
            chunk_type: ChunkType::default(),
            embedding: Some(vec![0.5, 0.25]),
            sparse_embedding: None,
            late_interaction: None,
            metadata: ChunkMetadata {
                byte_start: 0,
                byte_end: content.len(),
                token_count: Some(7),
                // Deliberately bogus whole-document numbering: the split must renumber it.
                chunk_index: 99,
                total_chunks: 99,
                first_page: pages.map(|(first, _)| first),
                last_page: pages.map(|(_, last)| last),
                heading_context: None,
                heading_path: Vec::new(),
                image_indices,
                node_ids: Vec::new(),
                page_spans: spans.into_iter().map(|page| PageSpan { page, bbox: None }).collect(),
                classifications: Vec::new(),
            },
        }
    }

    /// A 6-page document with *every* enrichment field populated.
    ///
    /// Page attribution (used by the per-segment assertions below):
    /// - tables: 1, 6            - images: 2, 4          - annotations: 1, 4
    /// - formulas: 2, 6          - ocr_elements: 3, 6    - classifications: 2, 4
    /// - uris: 1, 6, none        - form_fields: 3, none  - elements: 1, 4, none
    /// - chunks: 1-2, 3-4, 5-6, and one with no page attribution
    ///
    /// Page 5 deliberately carries no page-attributed enrichment at all.
    fn enriched_doc() -> ExtractedDocument {
        ExtractedDocument {
            content: "whole document".to_string(),
            mime_type: "application/pdf".into(),
            metadata: Metadata {
                pages: Some(PageStructure {
                    total_count: 6,
                    unit_type: PageUnitType::Page,
                    boundaries: None,
                    pages: None,
                }),
                ..Default::default()
            },
            extraction_method: Some(ExtractionMethod::Native),
            tables: vec![table(1), table(6)],
            counts: DocumentCounts {
                pages: 6,
                tables: 2,
                images: 2,
            },
            detected_languages: Some(vec!["en".to_string(), "de".to_string()]),
            chunks: Some(vec![
                chunk("alpha", Some((1, 2)), vec![0], vec![1, 2]),
                chunk("beta", Some((3, 4)), vec![1], vec![3, 4]),
                chunk("gamma", Some((5, 6)), Vec::new(), vec![5, 6]),
                chunk("pageless", None, Vec::new(), Vec::new()),
            ]),
            images: Some(vec![image(0, Some(2)), image(1, Some(4))]),
            pages: Some((1..=6).map(page).collect()),
            elements: Some(vec![
                element("e1", Some(1)),
                element("e2", Some(4)),
                element("e3", None),
            ]),
            djot_content: Some(DjotContent {
                plain_text: "djot body".to_string(),
                blocks: Vec::new(),
                metadata: Metadata::default(),
                tables: Vec::new(),
                images: Vec::new(),
                links: Vec::new(),
                footnotes: Vec::new(),
                attributes: Vec::new(),
            }),
            ocr_elements: Some(vec![ocr_element("o3", 3), ocr_element("o6", 6)]),
            document: Some(DocumentStructure {
                nodes: Vec::new(),
                source_format: Some("pdf".to_string()),
                relationships: Vec::new(),
                node_types: vec!["paragraph".to_string()],
            }),
            #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
            extracted_keywords: Some(vec![crate::keywords::Keyword {
                text: "invoice".to_string(),
                score: 0.5,
                algorithm: crate::keywords::KeywordAlgorithm::default(),
                positions: Some(vec![3]),
            }]),
            quality_score: Some(0.75),
            processing_warnings: vec![
                warning("chunking", "tokenizer fallback"),
                warning("chunking", "tokenizer fallback"),
                warning("language_detection", "low confidence"),
            ],
            annotations: Some(vec![annotation(1), annotation(4)]),
            children: Some(vec![ArchiveEntry {
                path: "inner.txt".to_string(),
                mime_type: "text/plain".to_string(),
                result: Box::new(ExtractedDocument::default()),
            }]),
            uris: Some(vec![
                uri("https://one.example", Some(1)),
                uri("https://six.example", Some(6)),
                uri("https://pageless.example", None),
            ]),
            revisions: Some(vec![DocumentRevision {
                revision_id: "r1".to_string(),
                author: Some("Ada".to_string()),
                timestamp: None,
                kind: RevisionKind::Insertion,
                anchor: None,
                delta: RevisionDelta::default(),
            }]),
            structured_output: Some(serde_json::json!({ "invoice_total": 42 })),
            #[cfg(feature = "tree-sitter")]
            code_intelligence: Some(serde_json::json!({ "functions": 3 })),
            llm_usage: Some(vec![LlmUsage {
                model: "gpt-test".to_string(),
                source: "vlm-ocr".to_string(),
                input_tokens: Some(11),
                output_tokens: Some(5),
                total_tokens: Some(16),
                estimated_cost: Some(0.25),
                finish_reason: Some("stop".to_string()),
            }]),
            entities: Some(vec![Entity {
                category: EntityCategory::Person,
                text: "Ada".to_string(),
                start: 0,
                end: 3,
                confidence: Some(0.9),
            }]),
            summary: Some(DocumentSummary {
                text: "a summary".to_string(),
                strategy: SummaryStrategy::Extractive,
                token_count: Some(3),
            }),
            #[cfg(feature = "heuristics")]
            extraction_confidence: Some(crate::heuristics::confidence::ExtractionConfidence {
                text_coverage: 0.8,
                ocr_aggregate: None,
                schema_compliance: crate::heuristics::confidence::SchemaCompliance::AllValid,
                combined: 0.9,
            }),
            translation: Some(Translation {
                target_lang: "de".to_string(),
                source_lang: Some("en".to_string()),
                content: "ganzes dokument".to_string(),
                formatted_content: None,
            }),
            page_classifications: Some(vec![classification(2, "invoice"), classification(4, "letter")]),
            redaction_report: Some(RedactionReport {
                findings: vec![RedactionFinding {
                    start: 0,
                    end: 3,
                    category: PiiCategory::Email,
                    strategy: RedactionStrategy::Mask,
                    replacement_token: "[REDACTED]".to_string(),
                }],
                total_redacted: 1,
            }),
            formulas: vec![formula("e=mc^2", 2), formula("a+b", 6)],
            form_fields: vec![form_field("f3", Some(3)), form_field("fnone", None)],
            ..Default::default()
        }
    }

    /// Split [`enriched_doc`] into the three segments 1..=2, 3..=4, 5..=6.
    fn three_segments(source: &ExtractedDocument) -> Vec<SplitSegment> {
        let warnings = deduped_split_warnings(&source.processing_warnings);
        [1..=2u32, 3..=4u32, 5..=6u32]
            .iter()
            .enumerate()
            .map(|(index, range)| sub_document_for_range(source, range, index == 0, &warnings))
            .collect()
    }

    fn segment(source: &ExtractedDocument, range: RangeInclusive<u32>, is_first: bool) -> SplitSegment {
        let warnings = deduped_split_warnings(&source.processing_warnings);
        sub_document_for_range(source, &range, is_first, &warnings)
    }

    fn uri_urls(document: &ExtractedDocument) -> Vec<&str> {
        document
            .uris
            .as_ref()
            .expect("uris must survive the split")
            .iter()
            .map(|item| item.url.as_str())
            .collect()
    }

    fn element_texts(document: &ExtractedDocument) -> Vec<&str> {
        document
            .elements
            .as_ref()
            .expect("elements must survive the split")
            .iter()
            .map(|item| item.text.as_str())
            .collect()
    }

    fn form_field_names(document: &ExtractedDocument) -> Vec<&str> {
        document.form_fields.iter().map(|item| item.name.as_str()).collect()
    }

    fn chunk_contents(document: &ExtractedDocument) -> Vec<&str> {
        document
            .chunks
            .as_ref()
            .expect("chunks must survive the split")
            .iter()
            .map(|item| item.content.as_str())
            .collect()
    }

    #[test]
    fn should_copy_whole_document_scalar_enrichment_onto_every_segment_when_split_into_three() {
        let source = enriched_doc();
        let segments = three_segments(&source);
        assert_eq!(segments.len(), 3, "three ranges must yield three segments");

        for (index, split) in segments.iter().enumerate() {
            let out = &split.document;
            assert_eq!(out.mime_type.as_ref(), "application/pdf", "segment {index}");
            assert_eq!(out.extraction_method, Some(ExtractionMethod::Native), "segment {index}");
            assert_eq!(
                out.detected_languages,
                Some(vec!["en".to_string(), "de".to_string()]),
                "segment {index}"
            );
            assert_eq!(out.quality_score, Some(0.75), "segment {index}");

            let summary = out
                .summary
                .as_ref()
                .unwrap_or_else(|| panic!("summary on segment {index}"));
            assert_eq!(summary.text, "a summary");
            assert_eq!(summary.strategy, SummaryStrategy::Extractive);
            assert_eq!(summary.token_count, Some(3));

            let translation = out
                .translation
                .as_ref()
                .unwrap_or_else(|| panic!("translation on segment {index}"));
            assert_eq!(translation.target_lang, "de");
            assert_eq!(translation.source_lang.as_deref(), Some("en"));
            assert_eq!(translation.content, "ganzes dokument");
            assert_eq!(translation.formatted_content, None);

            assert_eq!(
                out.structured_output,
                Some(serde_json::json!({ "invoice_total": 42 })),
                "segment {index}"
            );

            let structure = out
                .document
                .as_ref()
                .unwrap_or_else(|| panic!("document structure on segment {index}"));
            assert_eq!(structure.source_format.as_deref(), Some("pdf"));
            assert_eq!(structure.node_types, vec!["paragraph".to_string()]);

            assert_eq!(
                out.djot_content
                    .as_ref()
                    .unwrap_or_else(|| panic!("djot content on segment {index}"))
                    .plain_text,
                "djot body"
            );

            let children = out
                .children
                .as_ref()
                .unwrap_or_else(|| panic!("children on segment {index}"));
            assert_eq!(children.len(), 1);
            assert_eq!(children[0].path, "inner.txt");
            assert_eq!(children[0].mime_type, "text/plain");

            #[cfg(feature = "heuristics")]
            {
                let confidence = out
                    .extraction_confidence
                    .as_ref()
                    .unwrap_or_else(|| panic!("extraction confidence on segment {index}"));
                assert_eq!(confidence.text_coverage, 0.8);
                assert_eq!(confidence.combined, 0.9);
                assert_eq!(confidence.ocr_aggregate, None);
            }

            #[cfg(feature = "tree-sitter")]
            assert_eq!(
                out.code_intelligence,
                Some(serde_json::json!({ "functions": 3 })),
                "segment {index}"
            );
        }
    }

    #[test]
    fn should_attach_whole_document_resource_lists_to_the_first_segment_only_when_split_into_three() {
        let source = enriched_doc();
        let segments = three_segments(&source);

        let first = &segments[0].document;

        let entities = first.entities.as_ref().expect("entities on the first segment");
        assert_eq!(entities.len(), 1);
        assert_eq!(entities[0].category, EntityCategory::Person);
        assert_eq!(entities[0].text, "Ada");
        assert_eq!(entities[0].start, 0);
        assert_eq!(entities[0].end, 3);
        assert_eq!(entities[0].confidence, Some(0.9));

        let report = first
            .redaction_report
            .as_ref()
            .expect("redaction report on first segment");
        assert_eq!(report.total_redacted, 1);
        assert_eq!(report.findings.len(), 1);
        assert_eq!(report.findings[0].category, PiiCategory::Email);
        assert_eq!(report.findings[0].replacement_token, "[REDACTED]");

        let usage = first.llm_usage.as_ref().expect("llm usage on first segment");
        assert_eq!(usage.len(), 1);
        assert_eq!(usage[0].model, "gpt-test");
        assert_eq!(usage[0].source, "vlm-ocr");
        assert_eq!(usage[0].total_tokens, Some(16));
        assert_eq!(usage[0].estimated_cost, Some(0.25));

        let revisions = first.revisions.as_ref().expect("revisions on first segment");
        assert_eq!(revisions.len(), 1);
        assert_eq!(revisions[0].revision_id, "r1");
        assert_eq!(revisions[0].kind, RevisionKind::Insertion);

        #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
        {
            let keywords = first
                .extracted_keywords
                .as_ref()
                .expect("extracted keywords on first segment");
            assert_eq!(keywords.len(), 1);
            assert_eq!(keywords[0].text, "invoice");
            assert_eq!(keywords[0].score, 0.5);
            assert_eq!(keywords[0].positions, Some(vec![3]));
        }

        for (index, split) in segments.iter().enumerate().skip(1) {
            let out = &split.document;
            assert_eq!(out.entities, None, "segment {index} must not duplicate entities");
            assert_eq!(
                out.redaction_report, None,
                "segment {index} must not duplicate the redaction report"
            );
            assert!(out.llm_usage.is_none(), "segment {index} must not duplicate llm usage");
            assert!(out.revisions.is_none(), "segment {index} must not duplicate revisions");
            #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
            assert_eq!(
                out.extracted_keywords, None,
                "segment {index} must not duplicate keywords"
            );
        }
    }

    #[test]
    fn should_partition_page_attributed_enrichment_per_segment_when_split_into_three() {
        let source = enriched_doc();
        let segments = three_segments(&source);

        let first = &segments[0].document;
        assert_eq!(segments[0].page_range, 1..=2);
        assert_eq!(first.content, "page 1 text\n\npage 2 text");
        assert_eq!(
            first.counts,
            DocumentCounts {
                pages: 2,
                tables: 1,
                images: 1
            }
        );
        assert_eq!(first.metadata.pages.as_ref().expect("page structure").total_count, 2);
        assert_eq!(
            first.tables.iter().map(|t| t.page_number).collect::<Vec<_>>(),
            vec![1u32]
        );
        assert_eq!(
            first
                .images
                .as_ref()
                .expect("images")
                .iter()
                .map(|i| i.image_index)
                .collect::<Vec<_>>(),
            vec![0u32]
        );
        assert_eq!(
            first
                .annotations
                .as_ref()
                .expect("annotations")
                .iter()
                .map(|a| a.page_number)
                .collect::<Vec<_>>(),
            vec![1u32]
        );
        assert_eq!(
            first.formulas.iter().map(|f| f.latex.as_str()).collect::<Vec<_>>(),
            vec!["e=mc^2"]
        );
        assert_eq!(first.ocr_elements.as_ref().expect("ocr elements").len(), 0);
        assert_eq!(
            first.page_classifications.as_ref().expect("classifications"),
            &vec![classification(2, "invoice")]
        );
        assert_eq!(element_texts(first), vec!["e1", "e3"]);

        let second = &segments[1].document;
        assert_eq!(segments[1].page_range, 3..=4);
        assert_eq!(second.content, "page 3 text\n\npage 4 text");
        assert_eq!(
            second.counts,
            DocumentCounts {
                pages: 2,
                tables: 0,
                images: 1
            }
        );
        assert_eq!(second.tables.len(), 0);
        assert_eq!(
            second
                .images
                .as_ref()
                .expect("images")
                .iter()
                .map(|i| i.image_index)
                .collect::<Vec<_>>(),
            vec![1u32]
        );
        assert_eq!(
            second
                .annotations
                .as_ref()
                .expect("annotations")
                .iter()
                .map(|a| a.page_number)
                .collect::<Vec<_>>(),
            vec![4u32]
        );
        assert_eq!(second.formulas.len(), 0);
        assert_eq!(
            second
                .ocr_elements
                .as_ref()
                .expect("ocr elements")
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["o3"]
        );
        assert_eq!(
            second.page_classifications.as_ref().expect("classifications"),
            &vec![classification(4, "letter")]
        );
        assert_eq!(element_texts(second), vec!["e2", "e3"]);

        let third = &segments[2].document;
        assert_eq!(segments[2].page_range, 5..=6);
        assert_eq!(third.content, "page 5 text\n\npage 6 text");
        assert_eq!(
            third.counts,
            DocumentCounts {
                pages: 2,
                tables: 1,
                images: 0
            }
        );
        assert_eq!(
            third.tables.iter().map(|t| t.page_number).collect::<Vec<_>>(),
            vec![6u32]
        );
        assert_eq!(third.images.as_ref().expect("images").len(), 0);
        assert_eq!(third.annotations.as_ref().expect("annotations").len(), 0);
        assert_eq!(
            third.formulas.iter().map(|f| f.latex.as_str()).collect::<Vec<_>>(),
            vec!["a+b"]
        );
        assert_eq!(
            third
                .ocr_elements
                .as_ref()
                .expect("ocr elements")
                .iter()
                .map(|e| e.text.as_str())
                .collect::<Vec<_>>(),
            vec!["o6"]
        );
        assert_eq!(third.page_classifications.as_ref().expect("classifications").len(), 0);
        assert_eq!(element_texts(third), vec!["e3"]);
    }

    #[test]
    fn should_keep_enrichment_without_page_attribution_in_every_segment() {
        let source = enriched_doc();
        let segments = three_segments(&source);

        assert_eq!(
            uri_urls(&segments[0].document),
            vec!["https://one.example", "https://pageless.example"]
        );
        assert_eq!(uri_urls(&segments[1].document), vec!["https://pageless.example"]);
        assert_eq!(
            uri_urls(&segments[2].document),
            vec!["https://six.example", "https://pageless.example"]
        );

        assert_eq!(form_field_names(&segments[0].document), vec!["fnone"]);
        assert_eq!(form_field_names(&segments[1].document), vec!["f3", "fnone"]);
        assert_eq!(form_field_names(&segments[2].document), vec!["fnone"]);

        for (index, split) in segments.iter().enumerate() {
            assert!(
                element_texts(&split.document).contains(&"e3"),
                "the page-less element must appear on segment {index}"
            );
        }
    }

    #[test]
    fn should_filter_and_renumber_chunks_per_segment_when_split_into_three() {
        let source = enriched_doc();
        let segments = three_segments(&source);

        assert_eq!(chunk_contents(&segments[0].document), vec!["alpha", "pageless"]);
        assert_eq!(chunk_contents(&segments[1].document), vec!["beta", "pageless"]);
        assert_eq!(chunk_contents(&segments[2].document), vec!["gamma", "pageless"]);

        for (index, split) in segments.iter().enumerate() {
            let chunks = split.document.chunks.as_ref().expect("chunks");
            assert_eq!(
                chunks.iter().map(|c| c.metadata.chunk_index).collect::<Vec<_>>(),
                vec![0usize, 1],
                "segment {index} must renumber chunk_index"
            );
            assert_eq!(
                chunks.iter().map(|c| c.metadata.total_chunks).collect::<Vec<_>>(),
                vec![2usize, 2],
                "segment {index} must renumber total_chunks"
            );
            assert_eq!(
                chunks[0].embedding,
                Some(vec![0.5f32, 0.25]),
                "segment {index} must keep chunk embeddings"
            );
            assert_eq!(chunks[0].metadata.token_count, Some(7), "segment {index}");
            assert_eq!(chunks[1].metadata.page_spans, Vec::<PageSpan>::new(), "segment {index}");
        }

        assert_eq!(
            segments[0].document.chunks.as_ref().expect("chunks")[0]
                .metadata
                .page_spans,
            vec![PageSpan { page: 1, bbox: None }, PageSpan { page: 2, bbox: None }]
        );
        assert_eq!(
            segments[1].document.chunks.as_ref().expect("chunks")[0]
                .metadata
                .page_spans,
            vec![PageSpan { page: 3, bbox: None }, PageSpan { page: 4, bbox: None }]
        );

        // `image_indices` are indices into the segment's own `images` vector.
        assert_eq!(
            segments[0].document.chunks.as_ref().expect("chunks")[0]
                .metadata
                .image_indices,
            vec![0u32]
        );
        assert_eq!(
            segments[1].document.chunks.as_ref().expect("chunks")[0]
                .metadata
                .image_indices,
            vec![0u32],
            "the page-4 image is at split-local index 0 in segment 3..=4"
        );
        assert_eq!(
            segments[2].document.chunks.as_ref().expect("chunks")[0]
                .metadata
                .image_indices,
            Vec::<u32>::new()
        );
    }

    #[test]
    fn should_remap_chunk_image_indices_when_an_image_has_no_page_attribution() {
        let mut source = enriched_doc();
        // Whole-document images: [page-less, page 2, page 4]. The page-less image is kept
        // in every segment, so it still occupies split-local index 0 in segment 1..=2 and
        // the page-2 image lands at split-local index 1.
        source.images = Some(vec![image(7, None), image(0, Some(2)), image(1, Some(4))]);
        source.chunks = Some(vec![chunk("alpha", Some((1, 2)), vec![1], vec![1, 2])]);

        let split = segment(&source, 1..=2, true);
        let images = split.document.images.as_ref().expect("images");
        assert_eq!(
            images.iter().map(|i| i.image_index).collect::<Vec<_>>(),
            vec![7u32, 0],
            "the page-less image is kept alongside the page-2 image"
        );

        let chunks = split.document.chunks.as_ref().expect("chunks");
        assert_eq!(
            chunks[0].metadata.image_indices,
            vec![1u32],
            "the page-2 image sits at split-local index 1 once the page-less image is kept"
        );
        assert_eq!(
            images[chunks[0].metadata.image_indices[0] as usize].image_index, 0,
            "the remapped index must resolve back to the same image"
        );
    }

    #[test]
    fn should_dedupe_and_tag_processing_warnings_on_every_segment() {
        let source = enriched_doc();
        let segments = three_segments(&source);

        for (index, split) in segments.iter().enumerate() {
            let warnings = &split.document.processing_warnings;
            assert_eq!(warnings.len(), 2, "segment {index} must carry the deduped warnings");
            assert_eq!(warnings[0].source.as_ref(), "split:chunking");
            assert_eq!(warnings[0].message.as_ref(), "tokenizer fallback");
            assert_eq!(warnings[1].source.as_ref(), "split:language_detection");
            assert_eq!(warnings[1].message.as_ref(), "low confidence");
        }
    }

    #[test]
    fn should_preserve_every_enrichment_field_when_document_splits_into_one_segment() {
        let source = enriched_doc();
        let split = segment(&source, 1..=6, true);
        let out = &split.document;

        assert_eq!(split.page_range, 1..=6);
        assert_eq!(
            out.counts,
            DocumentCounts {
                pages: 6,
                tables: 2,
                images: 2
            }
        );
        assert_eq!(out.tables.len(), 2);
        assert_eq!(out.images.as_ref().expect("images").len(), 2);
        assert_eq!(out.annotations.as_ref().expect("annotations").len(), 2);
        assert_eq!(out.formulas.len(), 2);
        assert_eq!(out.ocr_elements.as_ref().expect("ocr elements").len(), 2);
        assert_eq!(out.page_classifications.as_ref().expect("classifications").len(), 2);
        assert_eq!(
            uri_urls(out),
            vec!["https://one.example", "https://six.example", "https://pageless.example"]
        );
        assert_eq!(form_field_names(out), vec!["f3", "fnone"]);
        assert_eq!(element_texts(out), vec!["e1", "e2", "e3"]);

        let chunks = out.chunks.as_ref().expect("chunks");
        assert_eq!(chunk_contents(out), vec!["alpha", "beta", "gamma", "pageless"]);
        assert_eq!(
            chunks.iter().map(|c| c.metadata.chunk_index).collect::<Vec<_>>(),
            vec![0usize, 1, 2, 3]
        );
        assert_eq!(
            chunks.iter().map(|c| c.metadata.total_chunks).collect::<Vec<_>>(),
            vec![4usize; 4]
        );

        assert_eq!(out.detected_languages, Some(vec!["en".to_string(), "de".to_string()]));
        assert_eq!(out.quality_score, Some(0.75));
        assert_eq!(out.summary.as_ref().expect("summary").text, "a summary");
        assert_eq!(out.translation.as_ref().expect("translation").target_lang, "de");
        assert_eq!(out.entities.as_ref().expect("entities").len(), 1);
        assert_eq!(
            out.redaction_report.as_ref().expect("redaction report").total_redacted,
            1
        );
        assert_eq!(out.llm_usage.as_ref().expect("llm usage").len(), 1);
        assert_eq!(out.revisions.as_ref().expect("revisions").len(), 1);
        assert_eq!(out.structured_output, Some(serde_json::json!({ "invoice_total": 42 })));
        assert_eq!(out.children.as_ref().expect("children").len(), 1);
        assert_eq!(out.processing_warnings.len(), 2);
        #[cfg(any(feature = "keywords-yake", feature = "keywords-rake"))]
        assert_eq!(out.extracted_keywords.as_ref().expect("keywords").len(), 1);
    }

    #[test]
    fn should_yield_empty_page_attributed_collections_when_a_segment_has_no_enrichment() {
        let source = enriched_doc();
        // Page 5 carries no table, image, annotation, formula, OCR element,
        // classification, page-bound URI, page-bound form field or page-bound element.
        let split = segment(&source, 5..=5, false);
        let out = &split.document;

        assert_eq!(out.content, "page 5 text");
        assert_eq!(
            out.counts,
            DocumentCounts {
                pages: 1,
                tables: 0,
                images: 0
            }
        );
        assert_eq!(out.tables.len(), 0);
        assert_eq!(out.images.as_ref().expect("images").len(), 0);
        assert_eq!(out.annotations.as_ref().expect("annotations").len(), 0);
        assert_eq!(out.formulas.len(), 0);
        assert_eq!(out.ocr_elements.as_ref().expect("ocr elements").len(), 0);
        assert_eq!(out.page_classifications.as_ref().expect("classifications").len(), 0);

        // Page-less enrichment still rides along.
        assert_eq!(uri_urls(out), vec!["https://pageless.example"]);
        assert_eq!(form_field_names(out), vec!["fnone"]);
        assert_eq!(element_texts(out), vec!["e3"]);

        // The 5..=6 chunk overlaps this range; its page spans are trimmed to page 5.
        assert_eq!(chunk_contents(out), vec!["gamma", "pageless"]);
        let chunks = out.chunks.as_ref().expect("chunks");
        assert_eq!(chunks[0].metadata.page_spans, vec![PageSpan { page: 5, bbox: None }]);
        assert_eq!(chunks[0].metadata.chunk_index, 0);
        assert_eq!(chunks[0].metadata.total_chunks, 2);

        // Whole-document enrichment is still present; first-segment-only lists are not.
        assert_eq!(out.detected_languages, Some(vec!["en".to_string(), "de".to_string()]));
        assert_eq!(out.quality_score, Some(0.75));
        assert_eq!(out.summary.as_ref().expect("summary").text, "a summary");
        assert_eq!(out.processing_warnings.len(), 2);
        assert_eq!(out.entities, None);
        assert!(out.llm_usage.is_none());
    }
}
