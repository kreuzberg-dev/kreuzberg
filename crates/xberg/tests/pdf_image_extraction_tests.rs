#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test/bench binaries print by design; org logging policy exempts tests
//! Regression tests for PDF image extraction in markdown output.
//!
//! Verifies that embedded images in PDFs produce proper `![](image_N.fmt)`
//! references instead of empty `![]()` placeholders.

#![cfg(feature = "pdf")]

use std::path::PathBuf;
use xberg::core::config::{ExtractionConfig, OutputFormat};

mod helpers;
use helpers::extract_uri_document;
// Gated to match its callers, every one of which is `#[cfg(feature = "ocr")]`.
// Without this, a leg that enables `pdf` but not `ocr` sees an unused import
// and fails on `-D warnings`. ~keep
#[cfg(feature = "ocr")]
use helpers::extract_uri_document_blocking;

fn test_documents_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test_documents")
}

fn extract_markdown(relative_path: &str) -> xberg::types::ExtractedDocument {
    use xberg::core::config::ImageExtractionConfig;
    let path = test_documents_dir().join(relative_path);
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        images: Some(ImageExtractionConfig {
            extract_images: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(extract_uri_document(&path, None, &config)).unwrap()
}

#[test]
fn test_multipage_marketing_no_empty_image_refs() {
    let result = extract_markdown("pdf/multipage_marketing.pdf");
    let content = &result.content;

    assert!(
        !content.contains("![]()"),
        "Markdown output must not contain empty image references ![](), got:\n{}",
        content
    );
}

#[test]
fn test_multipage_marketing_has_image_refs() {
    let result = extract_markdown("pdf/multipage_marketing.pdf");
    let content = &result.content;

    assert!(
        content.contains("![](image_"),
        "Markdown output must contain image references like ![](image_N.png), got:\n{}",
        content
    );
}

#[test]
fn test_multipage_marketing_images_populated() {
    let result = extract_markdown("pdf/multipage_marketing.pdf");

    let images = result.images.as_ref().expect("images field must be Some");
    assert!(!images.is_empty(), "Extraction result must contain extracted images");

    let images_with_data = images.iter().filter(|img| !img.data.is_empty()).count();
    assert!(
        images_with_data > 0,
        "At least some images should have actual pixel data, got {} images total but none with data",
        images.len()
    );
}

#[test]
fn test_docling_no_empty_image_refs() {
    let result = extract_markdown("pdf/docling.pdf");
    let content = &result.content;

    assert!(
        !content.contains("![]()"),
        "Docling markdown must not contain empty image references ![](), got:\n{}",
        content
    );
}

#[test]
fn test_docling_has_image_refs() {
    let result = extract_markdown("pdf/docling.pdf");
    let content = &result.content;

    assert!(
        content.contains("![](image_"),
        "Docling markdown must contain image references, got:\n{}",
        content
    );
}

#[test]
fn test_docling_content_quality() {
    let result = extract_markdown("pdf/docling.pdf");
    let content = &result.content;

    assert!(content.contains("Docling"), "Must contain 'Docling'");
    assert!(content.contains("PDF"), "Must contain 'PDF'");
    assert!(
        content.contains("table structure recognition") || content.contains("TableFormer"),
        "Must mention table structure recognition or TableFormer"
    );
}

/// Regression test for issue #752: structured output was ~1000x slower than text
/// on Ghostscript-produced PDFs with many inline images (~1,924 per page).
///
/// Root cause: `populate_images_from_oxide` used `Vec::contains` (O(N)) inside
/// the per-page object loop — O(N²) total. Fixed by converting to `AHashSet` for
/// O(1) lookup before the loop.
///
/// This test skips when the repro file is absent (it is not committed to the
/// repository due to size). To reproduce locally, generate a Ghostscript vector
/// decomposition PDF and place it at:
///   test_documents/pdf/ghostscript_inline_images_repro.pdf
#[test]
fn test_ghostscript_inline_images_completes_in_reasonable_time() {
    let path = test_documents_dir().join("pdf/ghostscript_inline_images_repro.pdf");
    if !path.exists() {
        eprintln!("SKIP: test_documents/pdf/ghostscript_inline_images_repro.pdf not present");
        return;
    }

    let config = xberg::core::config::ExtractionConfig {
        output_format: xberg::core::config::OutputFormat::Markdown,
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();

    let start = std::time::Instant::now();
    let result = rt
        .block_on(extract_uri_document(&path, None, &config))
        .expect("extraction must succeed for Ghostscript inline-image PDF");
    let elapsed = start.elapsed();

    assert!(
        elapsed.as_secs() < 10,
        "Ghostscript inline-image PDF must extract in under 10 seconds, took {elapsed:?}"
    );

    let _ = result;
}

/// Helper: extract with a specific output format and images explicitly disabled
/// via `ImageExtractionConfig.extract_images = false`.
fn extract_no_images(relative_path: &str, fmt: OutputFormat) -> xberg::types::ExtractedDocument {
    use xberg::core::config::ImageExtractionConfig;
    let path = test_documents_dir().join(relative_path);
    let config = ExtractionConfig {
        output_format: fmt,
        images: Some(ImageExtractionConfig {
            extract_images: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(extract_uri_document(&path, None, &config)).unwrap()
}

/// Helper: extract with a specific output format and images disabled via
/// `PdfConfig.extract_images = false`.
fn extract_no_images_via_pdf_options(relative_path: &str, fmt: OutputFormat) -> xberg::types::ExtractedDocument {
    use xberg::core::config::pdf::PdfConfig;
    let path = test_documents_dir().join(relative_path);
    let config = ExtractionConfig {
        output_format: fmt,
        pdf_options: Some(PdfConfig {
            extract_images: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(extract_uri_document(&path, None, &config)).unwrap()
}

/// Regression #796: images must be absent when extract_images=false, output_format=Markdown.
///
/// Uses `embedded_images_tables.pdf` — a known-image PDF. Before the fix, this
/// returned `ExtractedDocument.images` with full base64 data despite the flag.
#[test]
fn test_regression_796_markdown_no_images_when_disabled_via_images_config() {
    let result = extract_no_images("pdf/embedded_images_tables.pdf", OutputFormat::Markdown);
    assert!(
        result.images.as_ref().map(|v| v.is_empty()).unwrap_or(true),
        "images.extract_images=false must produce an empty images list even for \
         output_format=Markdown. Got {} image(s).",
        result.images.as_ref().map(|v| v.len()).unwrap_or(0)
    );
    assert!(
        !result.content.is_empty(),
        "Content must still be extracted when images are disabled"
    );
}

/// Regression #796: same assertion for Djot output format.
#[test]
fn test_regression_796_djot_no_images_when_disabled_via_images_config() {
    let result = extract_no_images("pdf/embedded_images_tables.pdf", OutputFormat::Djot);
    assert!(
        result.images.as_ref().map(|v| v.is_empty()).unwrap_or(true),
        "images.extract_images=false must produce an empty images list even for \
         output_format=Djot. Got {} image(s).",
        result.images.as_ref().map(|v| v.len()).unwrap_or(0)
    );
}

/// Regression #796: the pdf_options.extract_images path must also be respected
/// when output_format=Markdown.
#[test]
fn test_regression_796_markdown_no_images_when_disabled_via_pdf_options() {
    let result = extract_no_images_via_pdf_options("pdf/embedded_images_tables.pdf", OutputFormat::Markdown);
    assert!(
        result.images.as_ref().map(|v| v.is_empty()).unwrap_or(true),
        "pdf_options.extract_images=false must produce an empty images list even for \
         output_format=Markdown. Got {} image(s).",
        result.images.as_ref().map(|v| v.len()).unwrap_or(0)
    );
}

/// Sanity check: images must still appear when extract_images=true (no regression).
#[test]
fn test_regression_796_markdown_images_present_when_enabled() {
    use xberg::core::config::ImageExtractionConfig;
    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        images: Some(ImageExtractionConfig {
            extract_images: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(extract_uri_document(&path, None, &config)).unwrap();
    let images = result
        .images
        .as_ref()
        .expect("images must be Some when extract_images=true");
    assert!(
        !images.is_empty(),
        "images list must be non-empty when extract_images=true and the PDF contains images"
    );
}

/// Plain-text baseline: images must never appear for plain output (already passing
/// before the fix; kept as a safety net).
#[test]
fn test_regression_796_plain_no_images_when_disabled() {
    let result = extract_no_images("pdf/embedded_images_tables.pdf", OutputFormat::Plain);
    assert!(
        result.images.as_ref().map(|v| v.is_empty()).unwrap_or(true),
        "Plain output with extract_images=false must have no images. Got {} image(s).",
        result.images.as_ref().map(|v| v.len()).unwrap_or(0)
    );
}

/// Djot content must not contain image markup when `extract_images=false`.
///
/// End-to-end contract test: requires both the `inject_placeholders` guard in
/// `extraction.rs` AND the Djot renderer's `None` guard to be absent before it
/// fails. The renderer-level unit test `test_djot_renderer_skips_orphaned_image_element`
/// in `djot.rs` is the minimal proof that the renderer fix works independently.
#[test]
fn test_djot_content_has_no_image_refs_when_extraction_disabled() {
    let result = extract_no_images("pdf/embedded_images_tables.pdf", OutputFormat::Djot);
    assert!(
        !result.content.contains("![]()"),
        "Djot output must not contain empty ![]() refs when extract_images=false.\n\
         Got content:\n{}",
        &result.content[..result.content.len().min(400)]
    );
    assert!(
        !result.content.contains("![](image_"),
        "Djot output must not contain image placeholder refs when extract_images=false.\n\
         Got content:\n{}",
        &result.content[..result.content.len().min(400)]
    );
    assert!(
        !result.content.is_empty(),
        "Djot content must not be empty when images are disabled"
    );
}

/// Markdown content must not contain image markup when `extract_images=false`.
///
/// comrak_bridge already has a None guard so this would pass even without the
/// extraction-level guard, but it pins the end-to-end contract explicitly.
#[test]
fn test_markdown_content_has_no_image_refs_when_extraction_disabled() {
    let result = extract_no_images("pdf/embedded_images_tables.pdf", OutputFormat::Markdown);
    assert!(
        !result.content.contains("![]()"),
        "Markdown output must not contain empty ![]() refs when extract_images=false.\n\
         Got content:\n{}",
        &result.content[..result.content.len().min(400)]
    );
    assert!(
        !result.content.contains("![](image_"),
        "Markdown output must not contain image placeholder refs when extract_images=false.\n\
         Got content:\n{}",
        &result.content[..result.content.len().min(400)]
    );
}

/// Djot content must not contain image markup when disabled via `pdf_options.extract_images`.
///
/// Verifies both config paths are covered — mirrors the existing `result.images`
/// test for the pdf_options path.
#[test]
fn test_djot_content_has_no_image_refs_when_disabled_via_pdf_options() {
    let result = extract_no_images_via_pdf_options("pdf/embedded_images_tables.pdf", OutputFormat::Djot);
    assert!(
        !result.content.contains("![]()"),
        "Djot output (pdf_options path) must not contain ![]() when extract_images=false"
    );
    assert!(
        !result.content.contains("![](image_"),
        "Djot output (pdf_options path) must not contain image refs when extract_images=false"
    );
}

fn extract_with_pages_and_images(relative_path: &str) -> xberg::types::ExtractedDocument {
    use xberg::core::config::{ImageExtractionConfig, PageConfig};
    let path = test_documents_dir().join(relative_path);
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        pages: Some(PageConfig {
            extract_pages: true,
            ..Default::default()
        }),
        images: Some(ImageExtractionConfig {
            extract_images: true,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(extract_uri_document(&path, None, &config)).unwrap()
}

/// Pages that contain images must have non-empty `image_indices` pointing into
/// `ExtractedDocument.images`. Every index must be in-bounds.
#[test]
fn test_page_image_indices_are_valid_when_images_extracted() {
    let result = extract_with_pages_and_images("pdf/embedded_images_tables.pdf");

    let images = result.images.as_ref().expect("images must be Some");
    assert!(!images.is_empty(), "fixture must have extracted images");

    let pages = result
        .pages
        .as_ref()
        .expect("pages must be Some when extract_pages=true");
    assert!(!pages.is_empty(), "fixture must have pages");

    let pages_with_images: Vec<_> = pages.iter().filter(|p| !p.image_indices.is_empty()).collect();
    assert!(
        !pages_with_images.is_empty(),
        "at least one page must have image_indices populated when the PDF contains images"
    );

    for page in pages {
        for &idx in &page.image_indices {
            assert!(
                (idx as usize) < images.len(),
                "page {} image_indices[{}] = {} is out of bounds (images.len() = {})",
                page.page_number,
                idx,
                idx,
                images.len()
            );
            let img_page = images[idx as usize].page_number;
            assert_eq!(
                img_page,
                Some(page.page_number),
                "image at index {} has page_number {:?} but is referenced by page {}",
                idx,
                img_page,
                page.page_number
            );
        }
    }
}

/// `image_indices` on pages must be empty when image extraction is disabled.
#[test]
fn test_page_image_indices_empty_when_images_disabled() {
    use xberg::core::config::{ImageExtractionConfig, PageConfig};
    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        pages: Some(PageConfig {
            extract_pages: true,
            ..Default::default()
        }),
        images: Some(ImageExtractionConfig {
            extract_images: false,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(extract_uri_document(&path, None, &config)).unwrap();

    if let Some(pages) = result.pages.as_ref() {
        for page in pages {
            assert!(
                page.image_indices.is_empty(),
                "page {} must have no image_indices when extract_images=false",
                page.page_number
            );
        }
    }
}

#[cfg(feature = "chunking")]
fn extract_with_pages_images_and_chunks(relative_path: &str) -> xberg::types::ExtractedDocument {
    use xberg::core::config::{ChunkingConfig, ImageExtractionConfig, PageConfig};
    let path = test_documents_dir().join(relative_path);
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        pages: Some(PageConfig {
            extract_pages: true,
            ..Default::default()
        }),
        images: Some(ImageExtractionConfig {
            extract_images: true,
            ..Default::default()
        }),
        chunking: Some(ChunkingConfig {
            max_characters: 500,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    rt.block_on(extract_uri_document(&path, None, &config)).unwrap()
}

/// Chunks that span pages containing images must have non-empty `image_indices`.
/// Every index must be in-bounds, and the referenced image's `page_number` must
/// fall within the chunk's `[first_page, last_page]` range.
#[cfg(feature = "chunking")]
#[test]
fn test_chunk_image_indices_are_valid_when_images_extracted() {
    let result = extract_with_pages_images_and_chunks("pdf/embedded_images_tables.pdf");

    let images = result.images.as_ref().expect("images must be Some");
    assert!(!images.is_empty(), "fixture must have extracted images");

    let chunks = result
        .chunks
        .as_ref()
        .expect("chunks must be Some when chunking is configured");
    assert!(!chunks.is_empty(), "fixture must produce chunks");

    let chunks_with_images: Vec<_> = chunks.iter().filter(|c| !c.metadata.image_indices.is_empty()).collect();
    assert!(
        !chunks_with_images.is_empty(),
        "at least one chunk must have image_indices when the PDF contains images"
    );

    for chunk in chunks {
        for &idx in &chunk.metadata.image_indices {
            assert!(
                (idx as usize) < images.len(),
                "chunk image_indices[{}] = {} is out of bounds (images.len() = {})",
                idx,
                idx,
                images.len()
            );

            if let (Some(first), Some(last)) = (chunk.metadata.first_page, chunk.metadata.last_page) {
                let img_page = images[idx as usize]
                    .page_number
                    .expect("image referenced by a chunk must have a page_number set");
                assert!(
                    img_page >= first && img_page <= last,
                    "image at index {} is on page {} but chunk covers pages [{}, {}]",
                    idx,
                    img_page,
                    first,
                    last
                );
            }
        }
    }
}

/// Regression for #985: max_images_per_page must cap the output count per page.
///
/// Before the fix, `extract_image_positions` ran a complete decompression pass
/// over every page unconditionally (even when extract_images=false), then
/// `extract_images_with_data` ran a second pass.  The `.take(N)` limit only
/// clipped the returned slice — it did not stop the decompression work.
///
/// After the fix:
/// - When extract_images=false, NO decompression occurs at all (the main hang fix).
/// - When extract_images=true, a single pass runs and the cap is respected in output.
///   The per-page decompression cost for images beyond the cap is a pdf_oxide
///   upstream limitation: `extract_images()` is eager.  Eliminating that
///   remaining cost requires a count-limited API upstream.
#[test]
fn test_max_images_per_page_cap_respected_in_output() {
    use std::collections::HashMap;
    use xberg::core::config::ImageExtractionConfig;

    let path = test_documents_dir().join("pdf/installatiehandleiding_kombi_kompakt_hr.pdf");
    if !path.exists() {
        eprintln!("skipping: test PDF not present at {}", path.display());
        return;
    }

    let cap: u32 = 5;
    let config = ExtractionConfig {
        images: Some(ImageExtractionConfig {
            extract_images: true,
            max_images_per_page: Some(cap),
            ..Default::default()
        }),
        ..Default::default()
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(extract_uri_document(&path, None, &config))
        .expect("extraction must succeed");

    let images = result
        .images
        .as_ref()
        .expect("images must be Some when extract_images=true");

    let mut per_page: HashMap<u32, usize> = HashMap::new();
    for img in images {
        *per_page.entry(img.page_number.unwrap_or(1)).or_default() += 1;
    }
    for (page, count) in &per_page {
        assert!(
            *count <= cap as usize,
            "page {page} has {count} images; cap={cap} must be respected"
        );
    }
}

/// Regression for #985 (no-images case): when extract_images=false, no images
/// are returned and the result is consistent with the fix.
///
/// Before the fix, `extract_image_positions` ran unconditionally and triggered
/// a full decompression pass over every image on every page — even when the
/// caller never asked for image data.  After the fix the decompression path is
/// skipped entirely when images are not requested.
#[test]
fn test_no_images_returned_when_extraction_disabled_on_dense_pdf() {
    let path = test_documents_dir().join("pdf/installatiehandleiding_kombi_kompakt_hr.pdf");
    if !path.exists() {
        eprintln!("skipping: test PDF not present at {}", path.display());
        return;
    }

    let config = ExtractionConfig::default();

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(extract_uri_document(&path, None, &config))
        .expect("extraction must succeed");

    assert!(
        result.images.is_none() || result.images.as_ref().is_some_and(|v| v.is_empty()),
        "images must be absent when extract_images=false"
    );
}

/// Positions derived from extracted data must be consistent with the Markdown placeholders.
///
/// When inject_placeholders=true, the renderer emits `![](image_N.ext)` links where N
/// is the image_index.  Every such N must have a corresponding entry in result.images.
/// Also verifies that image_index values are unique — the derivation loop must not emit
/// duplicate global indices.
#[test]
fn test_image_positions_consistent_with_image_data() {
    use xberg::core::config::{ImageExtractionConfig, OutputFormat};

    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        images: Some(ImageExtractionConfig {
            extract_images: true,
            inject_placeholders: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(extract_uri_document(&path, None, &config)).unwrap();

    let images = match result.images.as_ref() {
        Some(imgs) if !imgs.is_empty() => imgs,
        _ => return,
    };

    let mut seen = std::collections::HashSet::new();
    for img in images {
        assert!(
            seen.insert(img.image_index),
            "image_index {} appears more than once — position derivation emitted duplicates",
            img.image_index
        );
    }

    let known: std::collections::HashSet<u32> = images.iter().map(|i| i.image_index).collect();
    let re = regex::Regex::new(r"!\[\]\(image_(\d+)\.[a-z]+\)").unwrap();
    for cap in re.captures_iter(&result.content) {
        let idx: u32 = cap[1].parse().unwrap();
        assert!(
            known.contains(&idx),
            "Markdown contains `![](image_{idx}.ext)` but result.images has no entry \
             with image_index={idx} — orphaned placeholder"
        );
    }
}

/// Regression for #985 (double-decompression fix): the text-only extraction path must
/// skip `extract_images_with_data` entirely.
///
/// When `extract_images` is `false` (the default), `extraction.rs` must not enter the
/// images branch at all — verified here by confirming that `result.images` is `None`
/// (or empty) and that the call completes without decompressing any image data.
/// This is the minimal structural proof that the guard in `extraction.rs` works:
/// if `extract_images_with_data` were called unconditionally, the result would be
/// `Some(non_empty_vec)` for a PDF that actually contains images.
#[test]
fn test_no_decompression_when_images_disabled() {
    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    assert!(path.exists(), "missing fixture: {}", path.display());

    let config = ExtractionConfig::default();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(extract_uri_document(&path, None, &config))
        .expect("extraction must succeed");

    assert!(
        result.images.as_ref().is_none_or(|v| v.is_empty()),
        "images must be absent on text-only extraction (extract_images=false). \
         Got {} image(s) — extract_images_with_data was called when it should not have been.",
        result.images.as_ref().map_or(0, |v| v.len())
    );

    assert!(
        !result.content.is_empty(),
        "text content must still be extracted when images are disabled"
    );
}

/// Trace-span assertion for #985: `extract_images_with_data` must NOT be entered
/// when `extract_images` is false (the default).
///
/// This directly proves the decompression code path was skipped — complementing
/// `test_no_decompression_when_images_disabled` which only observes the output.
/// An event with target `xberg::pdf::oxide::images` and field
/// `event = "decompression_started"` is emitted at the top of
/// `extract_images_with_data`; absence of that event is structural proof the
/// function was not called.
#[test]
fn test_no_decompression_trace_when_images_disabled() {
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _};

    #[allow(clippy::type_complexity)]
    #[derive(Clone, Default)]
    struct EventCapture {
        events: Arc<Mutex<Vec<(String, Option<String>)>>>,
    }

    impl<S> tracing_subscriber::Layer<S> for EventCapture
    where
        S: tracing::Subscriber,
    {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            let target = event.metadata().target().to_owned();

            if target != "xberg::pdf::oxide::images" {
                return;
            }

            struct FieldVisitor(Option<String>);
            impl tracing::field::Visit for FieldVisitor {
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    if field.name() == "event" {
                        self.0 = Some(value.to_owned());
                    }
                }
                fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                    if field.name() == "event" {
                        self.0 = Some(format!("{value:?}"));
                    }
                }
            }

            let mut visitor = FieldVisitor(None);
            event.record(&mut visitor);

            self.events.lock().unwrap().push((target, visitor.0));
        }
    }

    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    assert!(path.exists(), "missing fixture: {}", path.display());

    let capture = EventCapture::default();
    let capture_clone = capture.clone();

    // Enable DEBUG so the tracing event would be visible if the function ran.
    let filter = EnvFilter::new("debug");
    let subscriber = tracing_subscriber::registry().with(filter).with(capture_clone);

    let result = tracing::subscriber::with_default(subscriber, || {
        let config = ExtractionConfig::default();
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(extract_uri_document(&path, None, &config))
            .expect("extraction must succeed")
    });

    assert!(
        result.images.as_ref().is_none_or(|v| v.is_empty()),
        "images must be absent when extract_images=false"
    );

    let events = capture.events.lock().unwrap();
    let decompression_events: Vec<_> = events
        .iter()
        .filter(|(target, event_field)| {
            target == "xberg::pdf::oxide::images" && event_field.as_deref() == Some("decompression_started")
        })
        .collect();

    assert!(
        decompression_events.is_empty(),
        "extract_images_with_data must not be entered when extract_images=false; \
         got {} decompression_started event(s)",
        decompression_events.len()
    );
}

/// When `ocr_inline_images=true` and `extract_images=false`, images must still
/// be decompressed — `ocr_inline_images` forces entry into the extraction branch.
///
/// Before the fix for #985 this path was doubly dangerous: the unconditional
/// `extract_image_positions` call ran even when `extract_images=false`, and on
/// the oxide path the decompression was unbounded.  The OCR path was never
/// covered by a test, so a regression disabling decompression for
/// `ocr_inline_images=true` would be invisible.
#[test]
fn test_ocr_inline_images_enters_decompression_path() {
    use xberg::PdfConfig;
    use xberg::core::config::ImageExtractionConfig;

    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    assert!(path.exists(), "missing fixture: {}", path.display());

    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        images: Some(ImageExtractionConfig {
            extract_images: false,
            ..Default::default()
        }),
        pdf_options: Some(PdfConfig {
            ocr_inline_images: true,
            ..Default::default()
        }),
        ..Default::default()
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(extract_uri_document(&path, None, &config)).unwrap();

    let images = result.images.as_ref().expect(
        "result.images must be Some when ocr_inline_images=true, \
         even if extract_images=false — the extraction branch must be entered",
    );
    assert!(
        !images.is_empty(),
        "embedded_images_tables.pdf has embedded images; result.images must be non-empty \
         when ocr_inline_images=true forces entry into the decompression branch"
    );
}

/// Enabling `include_page_rasters` on a PDF with `force_ocr=true` must produce
/// `ImageKind::PageRaster` entries in `ExtractedDocument.images`.
///
/// Verifies:
/// - At least one `PageRaster` entry is present (per-page rendering ran).
/// - Every raster has `page_number = Some(N)` where N >= 1 (1-based assignment).
/// - Every raster has non-empty `data` (actual PNG bytes were captured).
/// - `image_index` values are unique across the full result set (reindex in
///   mod.rs:501-507 did not produce collisions).
/// - No `page_rasters` processing warning (Tesseract uses per-page path, not bypass).
#[cfg(feature = "ocr")]
#[test]
fn test_include_page_rasters_produces_rasters_on_force_ocr_pdf() {
    use xberg::core::config::{ImageExtractionConfig, OcrConfig};
    use xberg::types::ImageKind;

    let path = test_documents_dir().join("pdf/fake_memo.pdf");
    if !path.exists() {
        eprintln!("SKIP: test_documents/pdf/fake_memo.pdf not present");
        return;
    }

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: true,
        images: Some(ImageExtractionConfig {
            include_page_rasters: true,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&path, None, &config).expect("force_ocr extraction must succeed");

    let images = result
        .images
        .as_ref()
        .expect("images must be Some when include_page_rasters=true");

    let rasters: Vec<_> = images
        .iter()
        .filter(|img| img.image_kind == Some(ImageKind::PageRaster))
        .collect();

    assert!(
        !rasters.is_empty(),
        "include_page_rasters=true must produce at least one PageRaster entry; \
         got {} total images but none with PageRaster kind",
        images.len()
    );

    for raster in &rasters {
        assert!(
            raster.page_number.is_some(),
            "PageRaster at image_index={} must have page_number set",
            raster.image_index
        );
        assert!(
            raster.page_number.unwrap() >= 1,
            "PageRaster page_number must be >= 1 (1-based), got {}",
            raster.page_number.unwrap()
        );
        assert!(
            !raster.data.is_empty(),
            "PageRaster at image_index={} must have non-empty PNG data",
            raster.image_index
        );
    }

    let mut seen = std::collections::HashSet::new();
    for img in images {
        assert!(
            seen.insert(img.image_index),
            "image_index {} appears more than once — reindex produced duplicates",
            img.image_index
        );
    }

    let raster_warnings: Vec<_> = result
        .processing_warnings
        .iter()
        .filter(|w| w.source.as_ref() == "page_rasters")
        .collect();
    assert!(
        raster_warnings.is_empty(),
        "no page_rasters warning expected for Tesseract per-page OCR; got: {:?}",
        raster_warnings.iter().map(|w| w.message.as_ref()).collect::<Vec<_>>()
    );
}

/// `include_page_rasters=false` (the default) must not produce any `PageRaster`
/// entries even when `force_ocr=true` triggers per-page rendering.
///
/// Regression guard: the raster capture is conditional on the config flag;
/// removing that condition would cause this to fail.
#[cfg(feature = "ocr")]
#[test]
fn test_include_page_rasters_false_does_not_capture_rasters() {
    use xberg::core::config::{ImageExtractionConfig, OcrConfig};
    use xberg::types::ImageKind;

    let path = test_documents_dir().join("pdf/fake_memo.pdf");
    if !path.exists() {
        eprintln!("SKIP: test_documents/pdf/fake_memo.pdf not present");
        return;
    }

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr: true,
        images: Some(ImageExtractionConfig {
            include_page_rasters: false,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&path, None, &config).expect("force_ocr extraction must succeed");

    let raster_count = result
        .images
        .as_ref()
        .map(|imgs| {
            imgs.iter()
                .filter(|i| i.image_kind == Some(ImageKind::PageRaster))
                .count()
        })
        .unwrap_or(0);

    assert_eq!(
        raster_count, 0,
        "include_page_rasters=false must not produce any PageRaster images; got {raster_count}"
    );
}

/// `force_ocr_pages` path through `extract_mixed_ocr_native` must also produce
/// `PageRaster` entries when `include_page_rasters=true`.
///
/// This exercises a different code path than `force_ocr=true`: the mixed-OCR
/// path in `extract_mixed_ocr_native` (ocr.rs) encodes only the selected pages,
/// not all pages. Verifies that the per-page raster capture in that path works
/// end-to-end and produces correctly numbered entries.
#[cfg(feature = "ocr")]
#[test]
fn test_include_page_rasters_on_force_ocr_pages_path() {
    use xberg::core::config::{ImageExtractionConfig, OcrConfig};
    use xberg::types::ImageKind;

    let path = test_documents_dir().join("pdf/fake_memo.pdf");
    if !path.exists() {
        eprintln!("SKIP: test_documents/pdf/fake_memo.pdf not present");
        return;
    }

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr_pages: Some(vec![1]),
        images: Some(ImageExtractionConfig {
            include_page_rasters: true,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };

    let result = extract_uri_document_blocking(&path, None, &config).expect("force_ocr_pages extraction must succeed");

    let images = result
        .images
        .as_ref()
        .expect("images must be Some when include_page_rasters=true");

    let rasters: Vec<_> = images
        .iter()
        .filter(|img| img.image_kind == Some(ImageKind::PageRaster))
        .collect();

    assert!(
        !rasters.is_empty(),
        "include_page_rasters=true on force_ocr_pages=[1] must produce at least one PageRaster; \
         got {} total images but none with PageRaster kind",
        images.len()
    );

    for raster in &rasters {
        assert_eq!(
            raster.page_number,
            Some(1),
            "force_ocr_pages=[1] rasters must all be page_number=1; got {:?}",
            raster.page_number
        );
        assert!(
            !raster.data.is_empty(),
            "PageRaster at image_index={} must have non-empty PNG data",
            raster.image_index
        );
    }

    let mut seen = std::collections::HashSet::new();
    for img in images {
        assert!(
            seen.insert(img.image_index),
            "image_index {} appears more than once — reindex produced duplicates",
            img.image_index
        );
    }
}

/// When `force_ocr_pages` contains only page numbers that are out of range (e.g.,
/// page 99 on a 1-page PDF), `extract_mixed_ocr_native` returns `None` for rasters
/// because `render_selected_pages_for_ocr` produces an empty list. This is NOT a
/// document-level bypass, so no `ProcessingWarning` with `source = "page_rasters"`
/// should be emitted even when `include_page_rasters=true`.
#[cfg(feature = "ocr")]
#[test]
fn test_include_page_rasters_no_warning_on_out_of_range_pages() {
    use xberg::core::config::{ImageExtractionConfig, OcrConfig};

    let path = test_documents_dir().join("pdf/fake_memo.pdf");
    if !path.exists() {
        eprintln!("SKIP: test_documents/pdf/fake_memo.pdf not present");
        return;
    }

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        }),
        force_ocr_pages: Some(vec![99]),
        images: Some(ImageExtractionConfig {
            include_page_rasters: true,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };

    let result =
        extract_uri_document_blocking(&path, None, &config).expect("out-of-range force_ocr_pages must not error");

    let raster_warning = result
        .processing_warnings
        .iter()
        .find(|w| w.source.as_ref() == "page_rasters");

    assert!(
        raster_warning.is_none(),
        "force_ocr_pages with all-out-of-range pages must not emit a page_rasters warning \
         (no document-level bypass occurred); got: {:?}",
        result
            .processing_warnings
            .iter()
            .map(|w| (w.source.as_ref(), w.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

/// When `include_page_rasters=true` but the active OCR backend uses document-level
/// processing (bypassing per-page rendering), a `ProcessingWarning` with
/// `source = "page_rasters"` must be emitted.
///
/// This is the only scenario where `None` rasters flow through the `force_ocr`
/// path while `used_ocr=true`. Exercises the warning guard in
/// `PdfExtractor::extract` (mod.rs).
///
/// A mock backend with `supports_document_processing() = true` is registered so
/// that `extract_with_ocr` takes the document-level bypass instead of per-page
/// rendering. The mock returns an empty result — enough to trigger the warning.
#[cfg(feature = "ocr")]
#[test]
fn test_include_page_rasters_emits_warning_on_document_level_ocr_bypass() {
    use std::path::Path;
    use std::sync::Arc;
    use xberg::core::config::{ImageExtractionConfig, OcrConfig};
    use xberg::plugins::{OcrBackend, OcrBackendType, Plugin};
    use xberg::types::ExtractedDocument;

    let pdf_path = test_documents_dir().join("pdf/fake_memo.pdf");
    if !pdf_path.exists() {
        eprintln!("SKIP: test_documents/pdf/fake_memo.pdf not present");
        return;
    }

    struct DocLevelMock;

    #[async_trait::async_trait]
    impl OcrBackend for DocLevelMock {
        fn backend_type(&self) -> OcrBackendType {
            OcrBackendType::Custom
        }
        fn supports_language(&self, _: &str) -> bool {
            true
        }
        async fn process_image(&self, _: &[u8], _: &OcrConfig) -> xberg::Result<ExtractedDocument> {
            panic!("process_image must not be called for a document-level backend")
        }
        fn supports_document_processing(&self) -> bool {
            true
        }
        async fn process_document(&self, _: &Path, _: &OcrConfig) -> xberg::Result<ExtractedDocument> {
            Ok(ExtractedDocument::default())
        }
    }

    impl Plugin for DocLevelMock {
        fn name(&self) -> &str {
            "doc-level-mock-raster-warn"
        }
        fn version(&self) -> String {
            "1.0.0".to_string()
        }
        fn initialize(&self) -> xberg::Result<()> {
            Ok(())
        }
        fn shutdown(&self) -> xberg::Result<()> {
            Ok(())
        }
    }

    xberg::plugins::register_ocr_backend(Arc::new(DocLevelMock)).unwrap();

    let config = ExtractionConfig {
        ocr: Some(OcrConfig {
            backend: "doc-level-mock-raster-warn".to_string(),
            ..Default::default()
        }),
        force_ocr: true,
        images: Some(ImageExtractionConfig {
            include_page_rasters: true,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(extract_uri_document(&pdf_path, None, &config))
        .expect("document-level OCR mock must succeed");

    xberg::plugins::unregister_ocr_backend("doc-level-mock-raster-warn").unwrap();

    let raster_warning = result
        .processing_warnings
        .iter()
        .find(|w| w.source.as_ref() == "page_rasters");

    assert!(
        raster_warning.is_some(),
        "expected a page_rasters ProcessingWarning when include_page_rasters=true \
         and OCR backend uses document-level processing; got warnings: {:?}",
        result
            .processing_warnings
            .iter()
            .map(|w| (w.source.as_ref(), w.message.as_ref()))
            .collect::<Vec<_>>()
    );
}

/// Regression test for #1077: PDFs containing embedded images (common in Acrobat Sign /
/// Word exports) must extract without "image dimension probe failed" errors. Every image
/// must be probeable by load_image_for_ocr, extract_image_metadata, and VLM pipelines —
/// raw pixel buffers are re-encoded to PNG, DCT images pass through as JPEG, and no image
/// may escape as a headerless buffer with an undeterminable format.
///
/// Fixture: user_reports/mp_axmp_rec_en.pdf — the original bug reproducer.
#[test]
fn test_regression_1077_raw_pdf_images_re_encoded_as_png() {
    use xberg::core::config::ImageExtractionConfig;

    let path = test_documents_dir().join("user_reports/mp_axmp_rec_en.pdf");
    if !path.exists() {
        eprintln!("SKIP: user_reports/mp_axmp_rec_en.pdf not present");
        return;
    }

    let config = ExtractionConfig {
        images: Some(ImageExtractionConfig {
            extract_images: true,
            ..Default::default()
        }),
        use_cache: false,
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt
        .block_on(extract_uri_document(&path, None, &config))
        .expect("extraction of mp_axmp_rec_en.pdf must succeed without probe errors");

    let images = result
        .images
        .as_ref()
        .expect("images must be Some when extract_images=true");

    assert!(
        !images.is_empty(),
        "mp_axmp_rec_en.pdf must yield at least one extracted image"
    );

    for img in images.iter() {
        let magic = &img.data[..8.min(img.data.len())];
        let magic_matches = match img.format.as_ref() {
            "png" => img.data.starts_with(b"\x89PNG\r\n\x1a\n"),
            "jpeg" => img.data.starts_with(b"\xff\xd8\xff"),
            "gif" => img.data.starts_with(b"GIF8"),
            "tiff" => img.data.starts_with(b"II") || img.data.starts_with(b"MM"),
            "bmp" => img.data.starts_with(b"BM"),
            "webp" => img.data.len() >= 12 && &img.data[0..4] == b"RIFF" && &img.data[8..12] == b"WEBP",
            other => panic!(
                "image at index {} has non-probeable format {other:?} (the #1077 regression: \
                 headerless/unrecognized image buffer); first 8 bytes: {magic:02x?}",
                img.image_index
            ),
        };
        assert!(
            magic_matches,
            "image at index {} declares format={:?} but data does not start with the matching \
             magic bytes; first 8 bytes: {magic:02x?}",
            img.image_index, img.format
        );
    }
}

/// `image_indices` on chunks must be empty when image extraction is disabled.
#[cfg(feature = "chunking")]
#[test]
fn test_chunk_image_indices_empty_when_images_disabled() {
    use xberg::core::config::{ChunkingConfig, ImageExtractionConfig};
    let path = test_documents_dir().join("pdf/embedded_images_tables.pdf");
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        images: Some(ImageExtractionConfig {
            extract_images: false,
            ..Default::default()
        }),
        chunking: Some(ChunkingConfig {
            max_characters: 500,
            ..Default::default()
        }),
        ..Default::default()
    };
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(extract_uri_document(&path, None, &config)).unwrap();

    if let Some(chunks) = result.chunks.as_ref() {
        for chunk in chunks {
            assert!(
                chunk.metadata.image_indices.is_empty(),
                "chunk must have no image_indices when extract_images=false"
            );
        }
    }
}
