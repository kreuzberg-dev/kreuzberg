//! Envelope types and rendering for CLI output.
//!
//! When `--format json` or `--format toon` is used, extraction results are wrapped in these
//! envelopes so tooling (such as the benchmark harness) can read timing information without
//! parsing stderr or running a separate profiling tool.
//!
//! Text mode has no envelope to serialize into, so [`write_text_envelope`] renders the same
//! information as a short human-readable summary instead.

use serde::Serialize;
use std::io::Write;
use xberg::{ExtractedDocument, ProcessingWarning};

/// Prefix for every processing-warning line.
///
/// Kept as a constant so the CLI, its tests, and downstream log scrapers agree on one
/// literal instead of three copies of the same string.
pub const WARNING_PREFIX: &str = "warning";

/// Header line that opens the text-mode envelope summary.
pub const ENVELOPE_HEADER: &str = "--- extraction envelope ---";

/// Write the document's non-fatal [`ProcessingWarning`]s, one per line.
///
/// Warnings are written to the caller-supplied sink rather than `stdout` because in text
/// mode `stdout` carries the extracted content and must stay pipeable: `xberg extract doc.pdf
/// > out.txt` has to produce the document, not the document plus diagnostics. The CLI passes
/// `stderr`; tests pass a buffer.
///
/// Output is deliberately unstyled. `style::*` emits ANSI escapes whenever `NO_COLOR` is
/// unset, which would make the rendered text depend on the caller's environment and defeat
/// exact-value assertions.
///
/// Writes nothing at all when `warnings` is empty.
pub fn write_processing_warnings<W: Write>(warnings: &[ProcessingWarning], out: &mut W) -> std::io::Result<()> {
    for warning in warnings {
        writeln!(out, "{WARNING_PREFIX} [{}]: {}", warning.source, warning.message)?;
    }
    Ok(())
}

/// Write the text-mode envelope summary for an extracted document.
///
/// `--format json` and `--format toon` serialize the whole [`ExtractedDocument`], but text mode
/// prints only `content`, so everything else the extraction produced (page/table/image counts,
/// detected languages, quality score, chunk and entity counts, and any processing warnings) was
/// silently discarded. This renders those fields alongside the content.
///
/// Only populated sections are emitted: a field that carries no data produces no line, so a
/// plain-text extraction stays close to its previous single-line summary. Like
/// [`write_processing_warnings`] this targets `stderr` in the CLI so `stdout` remains exactly
/// the extracted content.
pub fn write_text_envelope<W: Write>(
    document: &ExtractedDocument,
    extraction_time_ms: f64,
    out: &mut W,
) -> std::io::Result<()> {
    write_processing_warnings(&document.processing_warnings, out)?;

    writeln!(out, "{ENVELOPE_HEADER}")?;
    writeln!(out, "mime type: {}", document.mime_type)?;
    if document.counts.pages > 0 {
        writeln!(out, "pages: {}", document.counts.pages)?;
    }
    if document.counts.tables > 0 {
        writeln!(out, "tables: {}", document.counts.tables)?;
    }
    if document.counts.images > 0 {
        writeln!(out, "images: {}", document.counts.images)?;
    }
    if let Some(languages) = document.detected_languages.as_ref().filter(|list| !list.is_empty()) {
        writeln!(out, "languages: {}", languages.join(", "))?;
    }
    if let Some(score) = document.quality_score {
        writeln!(out, "quality score: {score:.2}")?;
    }
    if let Some(chunks) = &document.chunks {
        writeln!(out, "chunks: {}", chunks.len())?;
    }
    if let Some(entities) = &document.entities {
        writeln!(out, "entities: {}", entities.len())?;
    }
    writeln!(out, "extraction time: {extraction_time_ms:.2} ms")
}

/// Per-stage cold-start timing breakdown for a single `xberg extract` invocation.
///
/// Only populated when stage timing is requested (see
/// [`crate::commands::extract::stage_timing_requested`]). Every duration is measured with
/// [`std::time::Instant`] (never wall-clock/system time) and reported in milliseconds.
///
/// # Stage coverage
///
/// - `process_init_ms` and `first_parse_ms` are measured directly at the CLI boundary and are
///   always accurate when this struct is present.
/// - `ort_session_and_inference_ms` is a coarse approximation: the core library does not expose
///   a public hook for ONNX Runtime session creation or first-inference timing (the closest
///   internal signal, `xberg::layout::inference_timings`, is `pub(crate)` and not reachable from
///   the CLI). When layout/OCR features that use ORT are active, this field reports the *total*
///   extraction wall time minus `first_parse_ms` as an upper bound that includes ORT session
///   creation, inference, and any other post-parse processing — it is not a clean sub-stage
///   measurement. See the doc comment on the field itself for the precise caveat.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct StageTimings {
    /// Time from process start (`main()` entry) to the point the extraction call begins,
    /// covering CLI argument parsing, logging setup, and config loading/merging.
    pub process_init_ms: f64,
    /// Wall-clock time for the core library's extraction call to return.
    ///
    /// Named "first parse" because this is the first (and only, for `extract`) document parse
    /// performed by the process. Includes any OCR/layout/ORT work performed during extraction.
    pub first_parse_ms: f64,
    /// Approximate ONNX Runtime session-creation-plus-first-inference cost, present only when a
    /// layout/OCR configuration that uses ORT is active for this extraction.
    ///
    /// This is **not** independently measured — the core extraction API has no public timing
    /// hook for ORT session creation or inference. It is reported as `first_parse_ms` again
    /// (the coarsest bound available at the CLI boundary): the whole extraction call, most of
    /// which is expected to be ORT session creation and inference on a cold-start layout
    /// extraction. Treat it as "extraction time when ORT-backed features are active", not as an
    /// isolated ORT sub-stage duration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ort_session_and_inference_ms: Option<f64>,
}

/// Single-file extraction result with wall-clock timing.
///
/// Emitted to stdout by `xberg extract --format json`.
#[derive(Debug, Serialize)]
pub struct ExtractEnvelope {
    /// The extracted document (content, metadata, tables, ...).
    pub result: ExtractedDocument,
    /// Wall-clock time for the extraction call in milliseconds.
    pub extraction_time_ms: f64,
    /// Self-reported peak resident-set size (RSS) in bytes for this process, measured via
    /// `getrusage(RUSAGE_SELF)` (see [`crate::peak_memory`]).
    ///
    /// The benchmark harness reads this as `_peak_memory_bytes` (see
    /// `tools/benchmark-harness/src/adapters/subprocess.rs::parse_output`), the same field every
    /// competitor wrapper self-reports via `resource.getrusage(...).ru_maxrss`, so both sides of a
    /// memory comparison use the same kernel-tracked high-water mark instead of two different
    /// estimators (one kernel-based, one sampling-based).
    pub peak_memory_bytes: u64,
    /// Per-stage cold-start timing breakdown, present only when stage timing was requested via
    /// the `XBERG_EMIT_STAGE_TIMING` environment variable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stage_timings: Option<StageTimings>,
}

/// Batch extraction results with per-file and total timing.
///
/// Emitted to stdout by `xberg batch --format json`.
#[derive(Debug, Serialize)]
pub struct BatchEnvelope {
    /// Extraction results in input order. A single input may yield multiple results.
    pub results: Vec<ExtractedDocument>,
    /// Total wall-clock time for the whole batch in milliseconds.
    pub total_ms: f64,
    /// Per-input wall-clock times in milliseconds, aligned with the input list.
    ///
    /// This has one entry per requested input even when an input yields multiple
    /// entries in `results`.
    pub per_file_ms: Vec<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn warning(source: &'static str, message: &'static str) -> ProcessingWarning {
        ProcessingWarning {
            source: source.into(),
            message: message.into(),
        }
    }

    fn rendered(document: &ExtractedDocument, extraction_time_ms: f64) -> String {
        let mut buffer = Vec::new();
        write_text_envelope(document, extraction_time_ms, &mut buffer).expect("writing to a Vec cannot fail");
        String::from_utf8(buffer).expect("renderer emits UTF-8")
    }

    /// Regression test for the defect where `xberg extract --format text` printed only
    /// `result.content`, so processing warnings were invisible at the CLI.
    #[test]
    fn write_processing_warnings_renders_source_and_message_for_each_warning() {
        let warnings = vec![
            warning("chunking", "chunk overlap exceeded chunk size"),
            warning("language_detection", "model unavailable"),
        ];

        let mut buffer = Vec::new();
        write_processing_warnings(&warnings, &mut buffer).unwrap();

        assert_eq!(
            String::from_utf8(buffer).unwrap(),
            "warning [chunking]: chunk overlap exceeded chunk size\n\
             warning [language_detection]: model unavailable\n"
        );
    }

    #[test]
    fn write_processing_warnings_writes_nothing_when_there_are_no_warnings() {
        let mut buffer = Vec::new();
        write_processing_warnings(&[], &mut buffer).unwrap();

        assert_eq!(String::from_utf8(buffer).unwrap(), "");
    }

    /// The warning text must reach the CLI's diagnostic stream as part of the text-mode
    /// envelope, not only via the `--format json` payload.
    #[test]
    fn write_text_envelope_includes_processing_warning_text() {
        let mut document = ExtractedDocument::default();
        document.mime_type = "text/plain".into();
        document.processing_warnings = vec![warning("embedding", "backend not configured")];

        assert_eq!(
            rendered(&document, 12.5),
            "warning [embedding]: backend not configured\n\
             --- extraction envelope ---\n\
             mime type: text/plain\n\
             extraction time: 12.50 ms\n"
        );
    }

    /// Regression test for the defect where every envelope field except `content` was
    /// discarded in text mode.
    #[test]
    fn write_text_envelope_reports_counts_languages_quality_chunks_and_entities() {
        let mut document = ExtractedDocument::default();
        document.mime_type = "application/pdf".into();
        document.counts = xberg::DocumentCounts {
            pages: 3,
            tables: 2,
            images: 1,
        };
        document.detected_languages = Some(vec!["en".to_string(), "de".to_string()]);
        // Exactly representable, so `{:.2}` cannot land on a rounding tie.
        document.quality_score = Some(0.5);
        document.chunks = Some(Vec::new());
        document.entities = Some(Vec::new());

        assert_eq!(
            rendered(&document, 250.0),
            "--- extraction envelope ---\n\
             mime type: application/pdf\n\
             pages: 3\n\
             tables: 2\n\
             images: 1\n\
             languages: en, de\n\
             quality score: 0.50\n\
             chunks: 0\n\
             entities: 0\n\
             extraction time: 250.00 ms\n"
        );
    }

    #[test]
    fn write_text_envelope_omits_sections_that_carry_no_data() {
        let mut document = ExtractedDocument::default();
        document.mime_type = "text/plain".into();
        document.detected_languages = Some(Vec::new());

        let output = rendered(&document, 1.0);

        assert_eq!(
            output,
            "--- extraction envelope ---\n\
             mime type: text/plain\n\
             extraction time: 1.00 ms\n"
        );
        assert!(!output.contains("pages:"), "zero page count must not be reported");
        assert!(
            !output.contains("languages:"),
            "an empty language list must not be reported"
        );
    }
}
