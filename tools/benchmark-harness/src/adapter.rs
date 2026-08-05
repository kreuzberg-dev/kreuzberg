//! Framework adapter system
//!
//! Adapters provide a unified interface for extracting content across different
//! extraction frameworks (both Xberg language bindings and open source alternatives).
//! This allows benchmarking any extraction framework against the same test fixtures.

use crate::{
    Error, Result,
    config::BenchmarkMode,
    provenance::ExecutableProvenance,
    types::{BatchCapability, BenchmarkResult, OutputFormat},
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;

/// Canonicalize a Tesseract OCR language request into individual codes.
///
/// A request may join languages with `+` (e.g. `"deu+eng"`). Xberg's own
/// `OcrConfig` deserializer splits on `+` (see `core/config/ocr.rs`), so the
/// benchmark adapters must build `OcrConfig.language` the same way — otherwise
/// a single `"deu+eng"` entry is treated as a literal pack name and never
/// resolves. Whitespace is trimmed and empty segments dropped.
pub(crate) fn canonicalize_ocr_languages(language: &str) -> Vec<String> {
    language
        .split('+')
        .map(str::trim)
        .filter(|code| !code.is_empty())
        .map(str::to_string)
        .collect()
}

pub(crate) fn is_valid_ocr_language_code(code: &str) -> bool {
    !code.is_empty()
        && code
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '_' | '-'))
}

pub(crate) fn canonical_ocr_language_arg(language: &str) -> Option<String> {
    let languages = canonicalize_ocr_languages(language);
    (!languages.is_empty()).then(|| languages.join("+"))
}

/// How an adapter accepts fixture-specific OCR language requests.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OcrLanguagePolicy {
    /// The adapter exposes no language selector; only the backend default is eligible.
    #[default]
    DefaultOnly,
    /// Each document carries any valid explicit language configuration.
    AnyPerDocument,
    /// One language configuration applies to an entire native batch invocation.
    AnyBatchGlobal,
    /// Sceptre's supported script groups, configured independently per document.
    SceptrePerDocument,
}

impl OcrLanguagePolicy {
    pub fn supports(self, language: Option<&str>) -> bool {
        let Some(language) = language else {
            return true;
        };
        if self == Self::DefaultOnly {
            return false;
        }
        if self != Self::SceptrePerDocument {
            return canonical_ocr_language_arg(language).is_some();
        }

        let groups: Option<Vec<usize>> = canonicalize_ocr_languages(language)
            .iter()
            .map(|code| sceptre_language_group(code))
            .collect();
        let Some(groups) = groups.filter(|groups| !groups.is_empty()) else {
            return false;
        };
        let mut non_english = groups.into_iter().filter(|group| *group != 0);
        let Some(first) = non_english.next() else {
            return true;
        };
        non_english.all(|group| group == first)
    }

    pub fn requires_homogeneous_batch_language(self) -> bool {
        self == Self::AnyBatchGlobal
    }

    pub fn partition_key(self, language: Option<&str>) -> Option<String> {
        language.and_then(canonical_ocr_language_arg)
    }

    pub fn batch_partition_count(self, languages: &[Option<String>], batch_size: usize) -> usize {
        if batch_size == 0 {
            return 0;
        }
        if !self.requires_homogeneous_batch_language() {
            return languages.len().div_ceil(batch_size);
        }

        let mut group_counts: Vec<(Option<String>, usize)> = Vec::new();
        for language in languages {
            let key = self.partition_key(language.as_deref());
            if let Some((_, count)) = group_counts.iter_mut().find(|(candidate, _)| *candidate == key) {
                *count += 1;
            } else {
                group_counts.push((key, 1));
            }
        }
        group_counts
            .into_iter()
            .map(|(_, count)| count.div_ceil(batch_size))
            .sum()
    }
}

/// Declares the OCR-language contract for a benchmark framework identity.
///
/// Keep artifact validation and runtime adapter construction on this shared policy so release
/// eligibility cannot drift from the rows the runner actually emits. ~keep
pub fn declared_ocr_language_policy(framework: &str) -> OcrLanguagePolicy {
    if framework.contains("-sceptre-") {
        return OcrLanguagePolicy::SceptrePerDocument;
    }
    if framework.starts_with("xberg-") {
        return OcrLanguagePolicy::AnyPerDocument;
    }

    match framework.strip_suffix("-batch").unwrap_or(framework) {
        "docling" | "mineru" => OcrLanguagePolicy::AnyBatchGlobal,
        "tika" | "unstructured" => OcrLanguagePolicy::AnyPerDocument,
        _ => OcrLanguagePolicy::DefaultOnly,
    }
}

fn sceptre_language_group(language: &str) -> Option<usize> {
    let normalized = language.trim().to_ascii_lowercase().replace('_', "-");
    SCEPTRE_LANGUAGE_GROUPS
        .iter()
        .position(|group| group.contains(&normalized.as_str()))
}

// Mirrors Xberg's private Sceptre language routing so benchmark eligibility matches execution.
const SCEPTRE_ENGLISH_LANGUAGES: &[&str] = &["english", "en", "eng"];
const SCEPTRE_LATIN_LANGUAGES: &[&str] = &[
    "latin", "af", "afr", "az", "aze", "bs", "bos", "cs", "ces", "cze", "cy", "cym", "wel", "da", "dan", "de", "deu",
    "ger", "es", "spa", "et", "est", "fr", "fra", "fre", "ga", "gle", "hr", "hrv", "hu", "hun", "id", "ind", "is",
    "isl", "ice", "it", "ita", "ku", "kur", "la", "lat", "lt", "lit", "lv", "lav", "mi", "mri", "mao", "ms", "msa",
    "may", "mt", "mlt", "nl", "nld", "dut", "no", "nor", "oc", "oci", "pi", "pli", "pl", "pol", "pt", "por", "ro",
    "ron", "rum", "rs-latin", "sr-latn", "srp-latn", "sk", "slk", "slo", "sl", "slv", "sq", "sqi", "alb", "sv", "swe",
    "sw", "swa", "tl", "fil", "tr", "tur", "uz", "uzb", "vi", "vie",
];
const SCEPTRE_CHINESE_LANGUAGES: &[&str] = &[
    "chinese-simplified",
    "simplified-chinese",
    "ch-sim",
    "zh",
    "zh-cn",
    "zh-hans",
    "zho",
    "chi",
    "chs",
];
const SCEPTRE_JAPANESE_LANGUAGES: &[&str] = &["japanese", "ja", "jpn", "jpn-vert"];
const SCEPTRE_KOREAN_LANGUAGES: &[&str] = &["korean", "ko", "kor"];
const SCEPTRE_TELUGU_LANGUAGES: &[&str] = &["telugu", "te", "tel"];
const SCEPTRE_KANNADA_LANGUAGES: &[&str] = &["kannada", "kn", "kan"];
const SCEPTRE_CYRILLIC_LANGUAGES: &[&str] = &[
    "cyrillic",
    "ru",
    "rus",
    "rs-cyrillic",
    "sr-cyrl",
    "srp-cyrl",
    "be",
    "bel",
    "bg",
    "bul",
    "uk",
    "ukr",
    "mn",
    "mon",
    "abq",
    "ady",
    "kbd",
    "ava",
    "dar",
    "inh",
    "che",
    "lbe",
    "lez",
    "tab",
    "tjk",
    "tg",
    "tgk",
];
const SCEPTRE_LANGUAGE_GROUPS: &[&[&str]] = &[
    SCEPTRE_ENGLISH_LANGUAGES,
    SCEPTRE_LATIN_LANGUAGES,
    SCEPTRE_CHINESE_LANGUAGES,
    SCEPTRE_JAPANESE_LANGUAGES,
    SCEPTRE_KOREAN_LANGUAGES,
    SCEPTRE_TELUGU_LANGUAGES,
    SCEPTRE_KANNADA_LANGUAGES,
    SCEPTRE_CYRILLIC_LANGUAGES,
];

/// PSM xberg auto-selects for standalone (whole-image) Tesseract OCR when no explicit `psm` is
/// configured — the documented production value of the private `WHOLE_IMAGE_TESSERACT_PSM` in
/// `crates/xberg/src/extractors/image.rs`.
///
/// ~keep: `crates/xberg`'s constant is a private `const`, not `pub`, so it cannot be imported
/// here — this is a hand-maintained mirror, NOT verified against xberg's source at build or test
/// time. It MUST be kept in sync by hand whenever `crates/xberg/src/extractors/image.rs` changes
/// `WHOLE_IMAGE_TESSERACT_PSM`. A benchmark that materializes `tesseract_config` to disable the
/// OCR result cache (see `comparison.rs::finalize_timed_ocr_result_cache`,
/// `batch_diagnostic.rs::disable_ocr_result_caches`, and
/// `adapters/subprocess.rs::materialize_tesseract_ocr`) must pin PSM to this same value, or it
/// silently regresses to `TesseractConfig::default()`'s PSM 3 and stops measuring xberg's real
/// production default.
pub(crate) const XBERG_WHOLE_IMAGE_TESSERACT_PSM: i32 = 11;

/// PSM xberg auto-selects for a vertical-script Tesseract language (any `*_vert` code, e.g.
/// `jpn_vert`) — the documented production value of the private `VERTICAL_BLOCK_TESSERACT_PSM` in
/// `crates/xberg/src/extractors/image.rs`. ~keep, same hand-maintained-mirror rationale as
/// [`XBERG_WHOLE_IMAGE_TESSERACT_PSM`].
pub(crate) const XBERG_VERTICAL_BLOCK_TESSERACT_PSM: i32 = 5;

/// Mirrors `apply_default_whole_image_tesseract_psm`'s vertical-language detection in
/// `crates/xberg/src/extractors/image.rs`: PSM 5 if any `+`-joined language code ends in
/// `_vert` (case-insensitive), else PSM 11. ~keep: MUST track that function by hand — it is not
/// `pub`, so this is not shared-source-backed either.
pub(crate) fn xberg_default_tesseract_psm(languages: &[String]) -> i32 {
    let has_vertical_language = languages
        .iter()
        .flat_map(|language| language.split('+'))
        .any(|language| language.trim().to_ascii_lowercase().ends_with("_vert"));
    if has_vertical_language {
        XBERG_VERTICAL_BLOCK_TESSERACT_PSM
    } else {
        XBERG_WHOLE_IMAGE_TESSERACT_PSM
    }
}

/// Unified interface for document extraction frameworks
///
/// Implementations of this trait can extract content from documents using
/// different extraction frameworks (Xberg language bindings and open source alternatives).
#[async_trait]
pub trait FrameworkAdapter: Send + Sync {
    /// Get the framework name (e.g., "xberg-rust", "xberg-python")
    fn name(&self) -> &str;

    /// Check if this adapter supports the given file type
    ///
    /// # Arguments
    /// * `file_type` - File extension without dot (e.g., "pdf", "docx")
    fn supports_format(&self, file_type: &str) -> bool;

    /// Check if this adapter should skip a specific file
    ///
    /// Some adapters need to skip specific files that are known to cause
    /// issues (e.g., timeouts in WASM for very large OCR-heavy documents).
    ///
    /// # Arguments
    /// * `file_name` - The file name (not full path) to check
    fn should_skip_file(&self, _file_name: &str) -> bool {
        false
    }

    /// Get the output formats supported by this adapter
    ///
    /// # Returns
    /// * `Vec<OutputFormat>` - List of supported output formats
    fn supported_output_formats(&self) -> Vec<OutputFormat> {
        vec![OutputFormat::Plaintext]
    }

    fn ocr_language_policy(&self) -> OcrLanguagePolicy {
        OcrLanguagePolicy::DefaultOnly
    }

    fn supports_fixture(&self, file_type: &str, file_name: Option<&str>, ocr_language: Option<&str>) -> bool {
        self.supports_format(file_type)
            && file_name.is_none_or(|name| !self.should_skip_file(name))
            && self.ocr_language_policy().supports(ocr_language)
    }

    /// Extract content from a document
    ///
    /// # Arguments
    /// * `file_path` - Path to the document to extract
    /// * `timeout` - Maximum time to wait for extraction
    /// * `force_ocr` - When true, force OCR even if the document has a text layer
    /// * `ocr_language` - Optional fixture-specific OCR language code
    /// * `output_format` - Output format for extraction (markdown or plaintext)
    ///
    /// # Returns
    /// * `Ok(BenchmarkResult)` - Successful extraction with metrics
    /// * `Err(Error)` - Extraction failed
    async fn extract(
        &self,
        file_path: &Path,
        timeout: Duration,
        force_ocr: bool,
        ocr_language: Option<&str>,
        output_format: OutputFormat,
    ) -> Result<BenchmarkResult>;

    /// Extract content from multiple documents using framework's batch API
    ///
    /// Frameworks with native batch support must override this method to use
    /// their optimized batch extraction API (e.g., Xberg's unified `extract_batch`).
    /// The default fails closed so batch benchmarks can never silently measure
    /// repeated single-file extraction.
    ///
    /// # Arguments
    /// * `file_paths` - Paths to documents to extract
    /// * `timeout` - Maximum time to wait for each extraction
    /// * `force_ocr` - Per-file force_ocr flags (must be same length as file_paths)
    /// * `ocr_languages` - Per-file optional OCR language codes
    /// * `output_format` - Output format for extraction
    ///
    /// # Returns
    /// * `Ok(Vec<BenchmarkResult>)` - Results for all files
    /// * `Err(Error)` - Batch extraction failed
    async fn extract_batch(
        &self,
        file_paths: &[&Path],
        timeout: Duration,
        force_ocr: &[bool],
        ocr_languages: &[Option<String>],
        output_format: OutputFormat,
    ) -> Result<Vec<BenchmarkResult>> {
        let _ = (file_paths, timeout, force_ocr, ocr_languages, output_format);
        Err(crate::Error::Config(format!(
            "framework '{}' does not expose a verified native batch API",
            self.name()
        )))
    }

    /// Return the verified batch API and timing semantics exposed by this adapter.
    fn batch_capability(&self) -> Option<BatchCapability> {
        None
    }

    /// Get version information for this framework
    fn version(&self) -> String {
        "unknown".to_string()
    }

    /// Return a path-free identity for the executable used by this adapter.
    fn executable_provenance(&self) -> Option<ExecutableProvenance> {
        None
    }

    /// Return the executable identity for the entry point used in the selected mode.
    fn executable_provenance_for_mode(&self, _mode: BenchmarkMode) -> Option<ExecutableProvenance> {
        self.executable_provenance()
    }

    /// Requested and effective worker counts, when the adapter exposes a worker control.
    fn worker_provenance(&self, requested: usize) -> (Option<usize>, Option<usize>) {
        (Some(requested), Some(requested))
    }

    /// Return the configured execution thread budget, when the adapter exposes one.
    ///
    /// This reports the value the adapter will pass to the framework, not merely
    /// the benchmark configuration that requested it.
    fn configured_thread_budget(&self) -> Option<usize> {
        None
    }

    /// Perform any necessary setup before benchmarking
    async fn setup(&self) -> Result<()> {
        Ok(())
    }

    /// Perform any necessary cleanup after benchmarking
    async fn teardown(&self) -> Result<()> {
        Ok(())
    }

    /// Warm up the framework by performing a test extraction
    ///
    /// This is called once before benchmarking to get the framework into a warm state.
    /// It measures the cold start time (framework load + first extraction).
    ///
    /// The default implementation performs a single extraction on the provided warmup file.
    ///
    /// # Arguments
    /// * `warmup_file` - Path to a small test file for warmup
    /// * `timeout` - Maximum time to wait for warmup
    /// * `output_format` - Output format for warmup extraction
    ///
    /// # Returns
    /// * `Ok(Duration)` - Cold start duration (framework load + first extraction)
    /// * `Err(Error)` - Warmup failed
    async fn warmup(&self, warmup_file: &Path, timeout: Duration, output_format: OutputFormat) -> Result<Duration> {
        let start = std::time::Instant::now();
        let result = self.extract(warmup_file, timeout, false, None, output_format).await?;
        if !result.success {
            return Err(Error::Benchmark(format!(
                "warmup extraction for '{}' failed: {}",
                self.name(),
                result
                    .error_message
                    .as_deref()
                    .unwrap_or("framework returned success=false")
            )));
        }
        Ok(start.elapsed())
    }
}

#[cfg(test)]
mod tests {
    use super::{
        OcrLanguagePolicy, XBERG_VERTICAL_BLOCK_TESSERACT_PSM, XBERG_WHOLE_IMAGE_TESSERACT_PSM,
        canonical_ocr_language_arg, canonicalize_ocr_languages, declared_ocr_language_policy,
        is_valid_ocr_language_code, xberg_default_tesseract_psm,
    };

    #[test]
    fn framework_language_policies_match_runtime_capabilities() {
        assert_eq!(
            declared_ocr_language_policy("liteparse"),
            OcrLanguagePolicy::DefaultOnly
        );
        assert_eq!(
            declared_ocr_language_policy("docling"),
            OcrLanguagePolicy::AnyBatchGlobal
        );
        assert_eq!(
            declared_ocr_language_policy("mineru-batch"),
            OcrLanguagePolicy::AnyBatchGlobal
        );
        assert_eq!(declared_ocr_language_policy("tika"), OcrLanguagePolicy::AnyPerDocument);
        assert_eq!(
            declared_ocr_language_policy("xberg-markdown-sceptre-ort"),
            OcrLanguagePolicy::SceptrePerDocument
        );
        assert_eq!(
            declared_ocr_language_policy("xberg-markdown-baseline"),
            OcrLanguagePolicy::AnyPerDocument
        );
    }

    #[test]
    fn sceptre_policy_matches_core_language_groups() {
        let policy = OcrLanguagePolicy::SceptrePerDocument;
        for language in [
            None,
            Some("eng"),
            Some("deu+eng"),
            Some("jpn"),
            Some("jpn_vert"),
            Some("kor"),
            Some("tel"),
            Some("kan"),
            Some("rus"),
        ] {
            assert!(policy.supports(language), "expected {language:?} to be supported");
        }
        for language in [Some("ara"), Some("cat"), Some("kaz"), Some("deu+jpn")] {
            assert!(!policy.supports(language), "expected {language:?} to be rejected");
        }
    }

    #[test]
    fn canonicalizes_combined_ocr_languages() {
        assert_eq!(canonicalize_ocr_languages(" deu + eng "), ["deu", "eng"]);
        assert_eq!(canonical_ocr_language_arg(" deu + eng ").as_deref(), Some("deu+eng"));
    }

    #[test]
    fn rejects_empty_or_path_like_ocr_language_codes() {
        assert!(canonical_ocr_language_arg(" + ").is_none());
        assert!(is_valid_ocr_language_code("chi_sim"));
        assert!(!is_valid_ocr_language_code("../deu"));
        assert!(!is_valid_ocr_language_code(""));
    }

    #[test]
    fn xberg_psm_constants_are_the_documented_production_values() {
        // ~keep: this asserts the documented literals, NOT that they still match
        // `crates/xberg/src/extractors/image.rs`'s private `WHOLE_IMAGE_TESSERACT_PSM` /
        // `VERTICAL_BLOCK_TESSERACT_PSM` — those constants are not `pub`, so nothing in this repo
        // can import and compare against xberg's real source. This test only guards against an
        // accidental typo/edit of the values below; it CANNOT catch upstream drift. If xberg ever
        // changes those constants, `XBERG_WHOLE_IMAGE_TESSERACT_PSM` /
        // `XBERG_VERTICAL_BLOCK_TESSERACT_PSM` above (and this test) must be updated by hand.
        assert_eq!(XBERG_WHOLE_IMAGE_TESSERACT_PSM, 11);
        assert_eq!(XBERG_VERTICAL_BLOCK_TESSERACT_PSM, 5);
    }

    #[test]
    fn selects_vertical_psm_only_for_vert_suffixed_languages() {
        assert_eq!(
            xberg_default_tesseract_psm(&["eng".to_string()]),
            XBERG_WHOLE_IMAGE_TESSERACT_PSM
        );
        assert_eq!(
            xberg_default_tesseract_psm(&["deu".to_string(), "eng".to_string()]),
            XBERG_WHOLE_IMAGE_TESSERACT_PSM
        );
        assert_eq!(
            xberg_default_tesseract_psm(&["jpn_vert".to_string()]),
            XBERG_VERTICAL_BLOCK_TESSERACT_PSM
        );
        assert_eq!(
            xberg_default_tesseract_psm(&["JPN_VERT".to_string()]),
            XBERG_VERTICAL_BLOCK_TESSERACT_PSM
        );
        assert_eq!(
            xberg_default_tesseract_psm(&["deu+jpn_vert".to_string()]),
            XBERG_VERTICAL_BLOCK_TESSERACT_PSM
        );
    }
}
