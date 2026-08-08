#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Issue #210 — `split_and_extract` must preserve enrichment, not just pages/tables/images.
//!
//! These are the end-to-end assertions for the enrichment fields that a fixture PDF can
//! populate deterministically and offline: chunking, keyword extraction, language
//! detection and processing warnings. The fields that need an ONNX model, a network
//! call or an LLM provider (entities, summary, translation, redaction report, LLM usage,
//! page classifications, code intelligence) are asserted field-by-field in the unit tests
//! in `src/core/split.rs::enrichment_preservation_tests`, because they cannot be produced
//! from a fixture without those backends.
//!
//! Fixture required (relative to `test_documents/`):
//! - `pdf/multi_page.pdf` — the 5-page native-text PDF also used by `split_and_extract.rs`.

#![cfg(all(
    feature = "pdf",
    feature = "chunking",
    feature = "keywords-yake",
    feature = "language-detection"
))]

mod helpers;
use helpers::{extract_bytes_document, get_test_file_path, skip_if_missing};

use xberg::{
    ChunkingConfig, ExtractedDocument, ExtractionConfig, KeywordConfig, LanguageDetectionConfig, PageConfig,
    SplitConfig, SplitStrategy, split_and_extract,
};

const FIXTURE: &str = "pdf/multi_page.pdf";

/// The three segments the fixture is split into. `multi_page.pdf` has 5 pages.
fn ranges() -> Vec<std::ops::RangeInclusive<u32>> {
    vec![1..=2, 3..=3, 4..=5]
}

/// Extraction config with every offline, deterministic enrichment stage switched on.
fn enriching_config() -> ExtractionConfig {
    ExtractionConfig {
        chunking: Some(ChunkingConfig {
            max_characters: 400,
            overlap: 0,
            ..Default::default()
        }),
        keywords: Some(KeywordConfig::default()),
        language_detection: Some(LanguageDetectionConfig::default()),
        pages: Some(PageConfig {
            extract_pages: true,
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// Extract the fixture as a single whole document — the ground truth every segment
/// assertion is derived from.
async fn whole_document(bytes: &[u8]) -> ExtractedDocument {
    extract_bytes_document(bytes, "application/pdf", &enriching_config())
        .await
        .expect("whole-document extraction of the fixture must succeed")
}

fn read_fixture() -> Option<Vec<u8>> {
    if skip_if_missing(FIXTURE) {
        eprintln!("skipping: fixture {FIXTURE} not found");
        return None;
    }
    Some(std::fs::read(get_test_file_path(FIXTURE)).expect("read multi_page.pdf"))
}

fn split_config() -> SplitConfig {
    SplitConfig {
        strategy: SplitStrategy::PageRanges(ranges()),
        extraction: enriching_config(),
    }
}

#[tokio::test]
async fn should_repeat_detected_languages_on_every_segment_when_split_into_three() {
    let Some(bytes) = read_fixture() else { return };
    let whole = whole_document(&bytes).await;
    let expected = whole
        .detected_languages
        .clone()
        .expect("precondition: language detection must populate the whole document");
    assert!(
        !expected.is_empty(),
        "precondition: the fixture must yield at least one detected language"
    );

    let segments = split_and_extract(&bytes, &split_config())
        .await
        .expect("split_and_extract must succeed");
    assert_eq!(segments.len(), 3);

    for (index, segment) in segments.iter().enumerate() {
        assert_eq!(
            segment.document.detected_languages.as_ref(),
            Some(&expected),
            "segment {index} must carry the whole-document detected languages"
        );
    }
}

#[tokio::test]
async fn should_attach_extracted_keywords_to_the_first_segment_only_when_split_into_three() {
    let Some(bytes) = read_fixture() else { return };
    let whole = whole_document(&bytes).await;
    let expected = whole
        .extracted_keywords
        .clone()
        .expect("precondition: keyword extraction must populate the whole document");
    assert!(
        !expected.is_empty(),
        "precondition: the fixture must yield at least one keyword"
    );

    let segments = split_and_extract(&bytes, &split_config())
        .await
        .expect("split_and_extract must succeed");

    assert_eq!(
        segments[0].document.extracted_keywords.as_ref(),
        Some(&expected),
        "the first segment carries the whole-document keywords verbatim"
    );
    assert_eq!(
        segments[1].document.extracted_keywords, None,
        "later segments must not duplicate whole-document keywords"
    );
    assert_eq!(segments[2].document.extracted_keywords, None);
}

#[tokio::test]
async fn should_partition_chunks_by_page_and_renumber_them_per_segment() {
    let Some(bytes) = read_fixture() else { return };
    let whole = whole_document(&bytes).await;
    let whole_chunks = whole
        .chunks
        .as_ref()
        .expect("precondition: chunking must populate the whole document");
    assert!(
        !whole_chunks.is_empty(),
        "precondition: the fixture must produce at least one chunk"
    );

    let segments = split_and_extract(&bytes, &split_config())
        .await
        .expect("split_and_extract must succeed");

    for (index, segment) in segments.iter().enumerate() {
        let (start, end) = (*segment.page_range.start(), *segment.page_range.end());

        // Specification: a chunk belongs to a segment when its page span overlaps the
        // segment's page range. A chunk with no page attribution cannot be excluded and
        // therefore belongs to every segment.
        let expected: Vec<&str> = whole_chunks
            .iter()
            .filter(|chunk| match (chunk.metadata.first_page, chunk.metadata.last_page) {
                (Some(first), Some(last)) => first <= end && last >= start,
                _ => true,
            })
            .map(|chunk| chunk.content.as_str())
            .collect();

        let actual = segment.document.chunks.as_ref().unwrap_or_else(|| {
            panic!("segment {index} must carry chunks");
        });
        assert_eq!(
            actual.iter().map(|chunk| chunk.content.as_str()).collect::<Vec<_>>(),
            expected,
            "segment {index} must carry exactly the chunks overlapping pages {start}..={end}"
        );
        assert_eq!(
            actual
                .iter()
                .map(|chunk| chunk.metadata.chunk_index)
                .collect::<Vec<_>>(),
            (0..actual.len()).collect::<Vec<_>>(),
            "segment {index} must renumber chunk_index from 0"
        );
        for chunk in actual {
            assert_eq!(
                chunk.metadata.total_chunks,
                actual.len(),
                "segment {index} must renumber total_chunks to its own chunk count"
            );
            for span in &chunk.metadata.page_spans {
                assert!(
                    (start..=end).contains(&span.page),
                    "segment {index} kept a page span for page {} outside {start}..={end}",
                    span.page
                );
            }
            if let Some(images) = segment.document.images.as_ref() {
                for &image_index in &chunk.metadata.image_indices {
                    assert!(
                        (image_index as usize) < images.len(),
                        "segment {index} chunk references image index {image_index} \
                         but the segment only has {} images",
                        images.len()
                    );
                }
            }
        }
    }
}

#[tokio::test]
async fn should_tag_and_dedupe_processing_warnings_on_every_segment() {
    let Some(bytes) = read_fixture() else { return };
    let whole = whole_document(&bytes).await;

    let mut seen = std::collections::HashSet::new();
    let expected: Vec<(String, String)> = whole
        .processing_warnings
        .iter()
        .filter(|warning| seen.insert((warning.source.to_string(), warning.message.to_string())))
        .map(|warning| (format!("split:{}", warning.source), warning.message.to_string()))
        .collect();

    let segments = split_and_extract(&bytes, &split_config())
        .await
        .expect("split_and_extract must succeed");

    for (index, segment) in segments.iter().enumerate() {
        let actual: Vec<(String, String)> = segment
            .document
            .processing_warnings
            .iter()
            .map(|warning| (warning.source.to_string(), warning.message.to_string()))
            .collect();
        assert_eq!(
            actual, expected,
            "segment {index} must carry the deduped, split-tagged whole-document warnings"
        );
    }
}

#[tokio::test]
async fn should_slice_pages_tables_and_images_by_page_range_when_split_into_three() {
    let Some(bytes) = read_fixture() else { return };
    let whole = whole_document(&bytes).await;
    let whole_pages = whole.pages.as_ref().expect("precondition: per-page content required");
    assert_eq!(whole_pages.len(), 5, "fixture multi_page.pdf must have 5 pages");

    let segments = split_and_extract(&bytes, &split_config())
        .await
        .expect("split_and_extract must succeed");
    assert_eq!(segments.len(), 3);

    let expected_ranges = ranges();
    for (index, segment) in segments.iter().enumerate() {
        let (start, end) = (*segment.page_range.start(), *segment.page_range.end());
        assert_eq!(segment.page_range, expected_ranges[index]);

        let expected_content = whole_pages
            .iter()
            .filter(|page| (start..=end).contains(&page.page_number))
            .map(|page| page.content.as_str())
            .filter(|text| !text.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");
        assert_eq!(
            segment.document.content, expected_content,
            "segment {index} content must be exactly its pages' content"
        );

        let expected_pages = (end - start + 1) as usize;
        assert_eq!(
            segment.document.counts.pages, expected_pages,
            "segment {index} page count"
        );
        assert_eq!(
            segment.document.metadata.pages.as_ref().map(|pages| pages.total_count),
            Some(expected_pages as u32),
            "segment {index} metadata page count must be rewritten to the slice"
        );

        for table in &segment.document.tables {
            assert!(
                (start..=end).contains(&table.page_number),
                "segment {index} kept a table from page {}",
                table.page_number
            );
        }
        for image in segment.document.images.iter().flatten() {
            assert!(
                image.page_number.is_none_or(|page| (start..=end).contains(&page)),
                "segment {index} kept an image from page {:?}",
                image.page_number
            );
        }
        assert_eq!(segment.document.mime_type.as_ref(), "application/pdf");
    }

    let total_pages: usize = segments.iter().map(|segment| segment.document.counts.pages).sum();
    assert_eq!(total_pages, 5, "the segments must cover every page exactly once");
}

#[tokio::test]
async fn should_preserve_enrichment_when_document_splits_into_one_segment() {
    let Some(bytes) = read_fixture() else { return };
    let whole = whole_document(&bytes).await;

    let config = SplitConfig {
        strategy: SplitStrategy::PageRanges(vec![1..=5]),
        extraction: enriching_config(),
    };
    let segments = split_and_extract(&bytes, &config)
        .await
        .expect("split_and_extract must succeed");
    assert_eq!(segments.len(), 1);

    let only = &segments[0].document;
    assert_eq!(segments[0].page_range, 1..=5);
    assert_eq!(only.counts.pages, 5);
    assert_eq!(only.detected_languages, whole.detected_languages);
    assert_eq!(only.extracted_keywords, whole.extracted_keywords);
    assert_eq!(
        only.chunks.as_ref().map(Vec::len),
        whole.chunks.as_ref().map(Vec::len),
        "a single segment covering the whole document must keep every chunk"
    );
    assert_eq!(only.tables.len(), whole.tables.len());
    assert_eq!(only.images.as_ref().map(Vec::len), whole.images.as_ref().map(Vec::len));
    assert_eq!(only.counts, whole.counts);
}
