//! Native Tesseract OCR backend.
//!
//! This module provides the native Tesseract backend that implements the OcrBackend
//! trait, bridging the plugin system with the low-level OcrProcessor.

use crate::Result;
use crate::core::config::OcrConfig;
use crate::ocr::processor::OcrProcessor;
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::types::ExtractedDocument;
use ahash::AHashMap;
use async_trait::async_trait;
use once_cell::sync::OnceCell;
use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use crate::ocr::types::TesseractConfig as InternalTesseractConfig;

/// Native Tesseract OCR backend.
///
/// This backend wraps the OcrProcessor and implements the OcrBackend trait,
/// allowing it to be used through the plugin system.
///
/// # Thread Safety
///
/// Uses Arc for shared ownership and is thread-safe (Send + Sync).
///
/// # Lazy Initialization
///
/// The native Tesseract/Leptonica FFI handle is allocated on first use,
/// not at backend construction. This allows the registry to be built without
/// triggering expensive native initialization.
#[cfg_attr(alef, alef(skip))]
pub struct TesseractBackend {
    processor: OnceCell<Arc<OcrProcessor>>,
    available_languages: OnceCell<Vec<String>>,
    #[cfg(not(target_arch = "wasm32"))]
    concurrency: Arc<tokio::sync::Semaphore>,
}

impl TesseractBackend {
    /// Create a new Tesseract backend wrapper (infallible).
    ///
    /// The actual FFI handle is allocated lazily on first use via
    /// `processor()`.
    pub(crate) fn new() -> Self {
        Self {
            processor: OnceCell::new(),
            available_languages: OnceCell::new(),
            #[cfg(not(target_arch = "wasm32"))]
            concurrency: Arc::new(tokio::sync::Semaphore::new(crate::ocr::processor::MAX_TESSERACT_APIS)),
        }
    }

    /// Get or initialize the Tesseract processor.
    ///
    /// Allocates the native FFI handle on first call; subsequent calls reuse
    /// the cached processor.
    fn processor(&self) -> Result<&Arc<OcrProcessor>> {
        self.processor.get_or_try_init(|| {
            OcrProcessor::new(None)
                .map(Arc::new)
                .map_err(|e| crate::XbergError::Ocr {
                    message: format!("Failed to create Tesseract processor: {}", e),
                    source: Some(Box::new(e)),
                })
        })
    }

    #[cfg(test)]
    pub(crate) fn processor_is_initialized(&self) -> bool {
        self.processor.get().is_some()
    }

    /// Convert OcrConfig to internal TesseractConfig.
    ///
    /// Uses tesseract_config from OcrConfig if provided, otherwise uses defaults
    /// with the language from OcrConfig. Multi-language configs are joined with "+".
    fn config_to_tesseract(&self, config: &OcrConfig) -> InternalTesseractConfig {
        let mut internal = match &config.tesseract_config {
            Some(tess_config) => InternalTesseractConfig::from(tess_config),
            None => InternalTesseractConfig {
                language: config.effective_languages().join("+"),
                ..Default::default()
            },
        };
        if internal.language.trim().is_empty() {
            internal.language = crate::core::config::ocr::DEFAULT_OCR_LANGUAGE.to_string();
        }
        if config.auto_rotate {
            internal.auto_rotate = true;
        }
        internal.tessdata_path = config.tessdata_path.clone();
        internal
    }

    /// Get cached available languages, lazily querying Tesseract if needed.
    ///
    /// Uses `OnceCell` to ensure the Tesseract API is only queried once.
    /// Falls back to hardcoded language list if dynamic querying fails.
    fn get_cached_languages(&self) -> &[String] {
        self.available_languages
            .get_or_init(|| match self.query_available_languages() {
                Ok(langs) => langs,
                Err(_) => Self::fallback_languages(),
            })
    }

    /// Query available languages from the Tesseract API.
    ///
    /// Creates a temporary Tesseract API instance and initializes it with
    /// the default English language to query available languages.
    ///
    /// # Returns
    ///
    /// Returns a vector of available language codes, or an error if querying fails.
    fn query_available_languages(&self) -> Result<Vec<String>> {
        let api = xberg_tesseract::TesseractAPI::new().map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to allocate Tesseract engine: {}", e),
            source: Some(Box::new(e)),
        })?;
        api.init("", "eng").map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to initialize Tesseract for language query: {}", e),
            source: Some(Box::new(e)),
        })?;

        api.get_available_languages().map_err(|e| crate::XbergError::Ocr {
            message: format!("Failed to query available Tesseract languages: {}", e),
            source: Some(Box::new(e)),
        })
    }

    /// Fallback list of supported languages (hardcoded list).
    ///
    /// Used when dynamic language querying fails, ensuring the backend
    /// always has a sensible default set of languages.
    fn fallback_languages() -> Vec<String> {
        vec![
            "eng", "deu", "fra", "spa", "ita", "por", "rus", "chi_sim", "chi_tra", "jpn", "jpn_vert", "kor", "ara",
            "hin", "ben", "tha", "vie", "heb", "tur", "pol", "nld", "swe", "dan", "fin", "nor", "ces", "hun", "ron",
            "ukr", "bul", "hrv", "srp", "slk", "slv", "lit", "lav", "est",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
}

impl Default for TesseractBackend {
    fn default() -> Self {
        Self::new()
    }
}

/// Convert a backend-internal [`crate::types::OcrTable`] into the public
/// [`crate::types::Table`], assigning a deterministic `table_id`.
///
/// `index` is the table's 0-based position among the tables returned for this
/// single OCR call (i.e. document push order for this call), so the id is
/// `"table-{index + 1}"` — never derived from randomness or wall-clock time,
/// so the same input always produces the same id. See
/// `crate::types::Table::table_id` for the shared scheme doc.
fn convert_ocr_table(index: usize, table: crate::types::OcrTable) -> crate::types::Table {
    let bounding_box = table.bounding_box.map(|bbox| crate::types::BoundingBox {
        x0: bbox.left as f64,
        y0: bbox.top as f64,
        x1: bbox.right as f64,
        y1: bbox.bottom as f64,
    });
    let columns = table.cells.first().cloned();
    crate::types::Table {
        cells: table.cells,
        markdown: table.markdown,
        page_number: table.page_number,
        bounding_box,
        table_id: Some(format!("table-{}", index + 1)),
        columns,
    }
}

impl Plugin for TesseractBackend {
    fn name(&self) -> &str {
        "tesseract"
    }

    fn version(&self) -> String {
        xberg_tesseract::TesseractAPI::version()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        if let Some(processor) = self.processor.get() {
            processor.clear_cache().map_err(|e| crate::XbergError::Plugin {
                message: format!("Failed to clear Tesseract cache: {}", e),
                plugin_name: "tesseract".to_string(),
            })
        } else {
            Ok(())
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl OcrBackend for TesseractBackend {
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
        self.process_image_owned(Arc::new(image_bytes.to_vec()), config).await
    }

    async fn process_image_owned(&self, image_bytes: Arc<Vec<u8>>, config: &OcrConfig) -> Result<ExtractedDocument> {
        let tess_config = self.config_to_tesseract(config);
        let tess_config_clone = tess_config.clone();
        let output_format = config.output_format.clone();

        let processor = Arc::clone(self.processor()?);

        #[cfg(not(target_arch = "wasm32"))]
        let permit = Arc::clone(&self.concurrency)
            .acquire_owned()
            .await
            .map_err(|error| crate::XbergError::Ocr {
                message: format!("Tesseract concurrency limiter closed unexpectedly: {error}"),
                source: None,
            })?;

        let operation = move || {
            #[cfg(not(target_arch = "wasm32"))]
            let _permit = permit;
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match output_format {
                Some(fmt) => processor.process_image_with_format(image_bytes.as_slice(), &tess_config_clone, fmt),
                None => processor.process_image(image_bytes.as_slice(), &tess_config_clone),
            }))
            .unwrap_or_else(|_| {
                Err(crate::ocr::error::OcrError::ProcessingFailed(
                    "Tesseract/Leptonica foreign exception caught".to_string(),
                ))
            })
        };
        #[cfg(not(target_arch = "wasm32"))]
        let ocr_result = tokio::task::spawn_blocking(operation)
            .await
            .map_err(|e| crate::XbergError::Plugin {
                message: format!("Tesseract task panicked or caught foreign exception: {}", e),
                plugin_name: "tesseract".to_string(),
            })?;
        #[cfg(target_arch = "wasm32")]
        let ocr_result = operation();
        let mut ocr_result = ocr_result.map_err(|e| crate::XbergError::Ocr {
            message: format!("Tesseract OCR failed: {}", e),
            source: Some(Box::new(e)),
        })?;
        normalize_vertical_cjk_result(&mut ocr_result, &tess_config.language, &tess_config.output_format);

        let resolved_language = ocr_result
            .metadata
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or(&tess_config.language)
            .to_string();

        let pre_formatted = extract_pre_formatted_metadata(&mut ocr_result.metadata);

        let processing_warnings = warnings_from_ocr_metadata(&ocr_result.metadata);
        strip_ocr_scratch_metadata_keys(&mut ocr_result.metadata);

        let mut additional = AHashMap::new();
        for (key, value) in ocr_result.metadata {
            additional.insert(Cow::Owned(key), value);
        }

        let metadata = crate::types::Metadata {
            format: Some(crate::types::FormatMetadata::Ocr(crate::types::OcrMetadata {
                language: resolved_language,
                psm: tess_config.psm as i32,
                output_format: tess_config.output_format.clone(),
                table_count: ocr_result.tables.len() as u32,
                table_rows: ocr_result.tables.first().map(|t| t.cells.len() as u32),
                table_cols: ocr_result
                    .tables
                    .first()
                    .and_then(|t| t.cells.first().map(|row| row.len() as u32)),
            })),
            output_format: pre_formatted,
            additional,
            ..Default::default()
        };

        Ok(ExtractedDocument {
            content: ocr_result.content,
            mime_type: ocr_result.mime_type.into(),
            metadata,
            tables: ocr_result
                .tables
                .into_iter()
                .enumerate()
                .map(|(index, t)| convert_ocr_table(index, t))
                .collect(),
            ocr_elements: ocr_result.ocr_elements,
            ocr_internal_document: ocr_result.internal_document,
            processing_warnings,
            ..Default::default()
        })
    }

    async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
        let tess_config = self.config_to_tesseract(config);
        let tess_config_clone = tess_config.clone();
        let output_format = config.output_format.clone();

        let processor = Arc::clone(self.processor()?);
        let path_str = path.to_string_lossy().to_string();

        #[cfg(not(target_arch = "wasm32"))]
        let permit = Arc::clone(&self.concurrency)
            .acquire_owned()
            .await
            .map_err(|error| crate::XbergError::Ocr {
                message: format!("Tesseract concurrency limiter closed unexpectedly: {error}"),
                source: None,
            })?;

        let operation = move || {
            #[cfg(not(target_arch = "wasm32"))]
            let _permit = permit;
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match output_format {
                Some(fmt) => processor.process_image_file_with_format(&path_str, &tess_config_clone, fmt),
                None => processor.process_image_file(&path_str, &tess_config_clone),
            }))
            .unwrap_or_else(|_| {
                Err(crate::ocr::error::OcrError::ProcessingFailed(
                    "Tesseract/Leptonica foreign exception caught".to_string(),
                ))
            })
        };
        #[cfg(not(target_arch = "wasm32"))]
        let ocr_result = tokio::task::spawn_blocking(operation)
            .await
            .map_err(|e| crate::XbergError::Plugin {
                message: format!("Tesseract task panicked or caught foreign exception: {}", e),
                plugin_name: "tesseract".to_string(),
            })?;
        #[cfg(target_arch = "wasm32")]
        let ocr_result = operation();
        let mut ocr_result = ocr_result.map_err(|e| crate::XbergError::Ocr {
            message: format!("Tesseract OCR failed: {}", e),
            source: Some(Box::new(e)),
        })?;
        normalize_vertical_cjk_result(&mut ocr_result, &tess_config.language, &tess_config.output_format);

        let resolved_language = ocr_result
            .metadata
            .get("language")
            .and_then(|v| v.as_str())
            .unwrap_or(&tess_config.language)
            .to_string();

        let pre_formatted = extract_pre_formatted_metadata(&mut ocr_result.metadata);

        let processing_warnings = warnings_from_ocr_metadata(&ocr_result.metadata);
        strip_ocr_scratch_metadata_keys(&mut ocr_result.metadata);

        let mut additional = AHashMap::new();
        for (key, value) in ocr_result.metadata {
            additional.insert(Cow::Owned(key), value);
        }

        let metadata = crate::types::Metadata {
            format: Some(crate::types::FormatMetadata::Ocr(crate::types::OcrMetadata {
                language: resolved_language,
                psm: tess_config.psm as i32,
                output_format: tess_config.output_format.clone(),
                table_count: ocr_result.tables.len() as u32,
                table_rows: ocr_result.tables.first().map(|t| t.cells.len() as u32),
                table_cols: ocr_result
                    .tables
                    .first()
                    .and_then(|t| t.cells.first().map(|row| row.len() as u32)),
            })),
            output_format: pre_formatted,
            additional,
            ..Default::default()
        };

        Ok(ExtractedDocument {
            content: ocr_result.content,
            mime_type: ocr_result.mime_type.into(),
            metadata,
            tables: ocr_result
                .tables
                .into_iter()
                .enumerate()
                .map(|(index, t)| convert_ocr_table(index, t))
                .collect(),
            ocr_elements: ocr_result.ocr_elements,
            ocr_internal_document: ocr_result.internal_document,
            processing_warnings,
            ..Default::default()
        })
    }

    fn supports_language(&self, lang: &str) -> bool {
        self.get_cached_languages().contains(&lang.to_string())
    }

    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::Tesseract
    }

    fn supported_languages(&self) -> Vec<String> {
        self.get_cached_languages().to_vec()
    }

    fn supports_table_detection(&self) -> bool {
        true
    }

    #[cfg_attr(alef, alef(skip))]
    fn probe(&self, config: &OcrConfig) -> crate::doctor::DoctorCheck {
        #[cfg(target_arch = "wasm32")]
        {
            let _ = config;
            crate::doctor::DoctorCheck::skip("ocr.tesseract", "tessdata probe is not available on wasm32")
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            probe_tessdata(config)
        }
    }
}

/// Check-only tessdata probe: mirrors the runtime resolution chain without
/// materializing or downloading language packs.
///
/// The runtime requires ONE directory containing every requested language
/// (`resolve_tessdata_path`); the probe applies the same rule, then
/// distinguishes "known language, would download on first use" (skip) from
/// "unknown language code, download would also fail" (fail).
#[cfg(not(target_arch = "wasm32"))]
fn probe_tessdata(config: &OcrConfig) -> crate::doctor::DoctorCheck {
    let dirs = crate::ocr::processor::validation::tessdata_search_dirs(config.tessdata_path.as_deref());
    probe_tessdata_in_dirs(config, &dirs)
}

#[cfg(not(target_arch = "wasm32"))]
fn probe_tessdata_in_dirs(config: &OcrConfig, dirs: &[String]) -> crate::doctor::DoctorCheck {
    use crate::doctor::DoctorCheck;
    use crate::ocr::validation::TESSERACT_SUPPORTED_LANGUAGE_CODES;

    let version = xberg_tesseract::TesseractAPI::version();
    let languages = config.effective_languages();

    if let Some(dir) = dirs.iter().find(|dir| {
        languages
            .iter()
            .all(|lang| std::path::Path::new(dir).join(format!("{lang}.traineddata")).exists())
    }) {
        return DoctorCheck::pass(
            "ocr.tesseract",
            format!(
                "tesseract {version}; tessdata for {} language(s) at {dir}",
                languages.len()
            ),
        );
    }

    let missing: Vec<&str> = languages
        .iter()
        .map(String::as_str)
        .filter(|lang| {
            !dirs
                .iter()
                .any(|dir| std::path::Path::new(dir).join(format!("{lang}.traineddata")).exists())
        })
        .collect();

    let (unknown, downloadable): (Vec<&str>, Vec<&str>) = missing
        .iter()
        .copied()
        .partition(|lang| !TESSERACT_SUPPORTED_LANGUAGE_CODES.contains(lang));

    if !unknown.is_empty() {
        return DoctorCheck::fail(
            "ocr.tesseract",
            format!(
                "unknown language code(s): {} (no traineddata exists)",
                unknown.join(", ")
            ),
        );
    }

    DoctorCheck::skip(
        "ocr.tesseract",
        format!(
            "tessdata for [{}] not found locally (will download on first use)",
            downloadable.join(", ")
        ),
    )
}

fn normalize_vertical_cjk_result(result: &mut crate::types::OcrExtractionResult, language: &str, output_format: &str) {
    if matches!(output_format, "hocr" | "tsv")
        || !language
            .split('+')
            .any(|code| code.to_ascii_lowercase().ends_with("_vert"))
    {
        return;
    }
    result.content = compact_cjk_horizontal_spacing(&result.content);
    for table in &mut result.tables {
        for row in &mut table.cells {
            for cell in row {
                *cell = compact_cjk_horizontal_spacing(cell);
            }
        }
        table.markdown = compact_cjk_horizontal_spacing(&table.markdown);
    }
    if let Some(document) = result.internal_document.as_mut() {
        for (index, element) in document.elements.iter_mut().enumerate() {
            element.text = compact_cjk_horizontal_spacing(&element.text);
            element.id = crate::types::internal::InternalElementId::generate(
                element.kind.discriminant(),
                &element.text,
                element.page,
                index as u32,
            );
        }
    }
}

/// Metadata key `perform_ocr` sets when the Tesseract result iterator dropped
/// one or more words (null pointer, invalid parameter, or invalid UTF-8)
/// rather than including them in `ocr_elements`. Mirrors the literal written
/// in `ocr::processor::execution::insert_word_iterator_skipped_count_metadata`.
const WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY: &str = "word_iterator_skipped_count";

/// Metadata key `perform_ocr` sets when `auto_rotate` was requested but the
/// `auto-rotate` build feature is not compiled in, so orientation detection
/// never ran. Mirrors the literal written in `ocr::processor::execution::perform_ocr`.
const AUTO_ROTATE_UNAVAILABLE_METADATA_KEY: &str = "auto_rotate_unavailable";

/// Metadata key `perform_ocr` sets when it rebuilds `content` with inline
/// table markdown at each table's original vertical position (see
/// `perform_ocr`'s `build_content_with_inline_tables` step). Promoted to
/// `Metadata::output_format` by [`extract_pre_formatted_metadata`] rather
/// than left as a raw `Metadata::additional` entry (#354).
const PRE_FORMATTED_METADATA_KEY: &str = "pre_formatted";

/// Pull the `pre_formatted` marker out of an `OcrExtractionResult`'s metadata
/// map, returning its string value if present.
///
/// Uses `.remove()`, not `.get()`, so the key never also survives into the
/// user-visible `Metadata::additional` map once the remaining metadata is
/// copied wholesale (#354). Extracted as its own pure function, mirroring
/// `warnings_from_ocr_metadata`, so the removal is unit-testable without a
/// live Tesseract API instance.
fn extract_pre_formatted_metadata(
    metadata: &mut std::collections::HashMap<String, serde_json::Value>,
) -> Option<String> {
    metadata
        .remove(PRE_FORMATTED_METADATA_KEY)
        .and_then(|v| v.as_str().map(str::to_string))
}

/// Turn the OCR backend's metadata side-channel into the `ProcessingWarning`s a
/// caller of `ExtractedDocument` actually sees (#309).
///
/// `OcrExtractionResult` has no dedicated warnings field of its own -- adding
/// one would be binding-visible drift across every language binding, since the
/// struct carries no `alef(skip)`. `perform_ocr` instead records data-loss
/// signals into its existing free-form `metadata` map, the same precedent set
/// by the `pre_formatted` and `word_iterator_skipped_count` keys. This function
/// is the other half: it reads those same keys back out here, where the map is
/// still available (before it is drained into `Metadata::additional`), and
/// turns them into warnings on the returned `ExtractedDocument`.
///
/// Extracted as its own pure function so the metadata-to-warning mapping is
/// unit-testable without a live Tesseract API instance.
fn warnings_from_ocr_metadata(
    metadata: &std::collections::HashMap<String, serde_json::Value>,
) -> Vec<crate::types::ProcessingWarning> {
    let mut warnings = Vec::new();

    if let Some(skipped) = metadata
        .get(WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY)
        .and_then(serde_json::Value::as_u64)
        && skipped > 0
    {
        crate::core::diagnostics::push_warning(
            &mut warnings,
            "tesseract",
            format!(
                "The Tesseract result iterator failed to extract {skipped} word(s) from this image \
                 (null pointer, invalid parameter, or invalid UTF-8); those words are missing from \
                 the OCR output"
            ),
        );
    }

    if metadata
        .get(AUTO_ROTATE_UNAVAILABLE_METADATA_KEY)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
    {
        crate::core::diagnostics::push_warning(
            &mut warnings,
            "tesseract",
            "auto_rotate was requested but this build does not include the `auto-rotate` feature; \
             the image was OCR'd without orientation detection or correction",
        );
    }

    warnings
}

/// Remove the OCR pipeline's internal scratch metadata keys before the
/// remaining map is copied wholesale into the user-visible
/// `Metadata::additional` (#354).
///
/// `WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY` and
/// `AUTO_ROTATE_UNAVAILABLE_METADATA_KEY` are plumbing for
/// `warnings_from_ocr_metadata` (call that first -- it reads these keys back
/// out before this function removes them) and have no meaning as document
/// metadata. `pre_formatted` is handled separately by
/// [`extract_pre_formatted_metadata`] because it is promoted to
/// `Metadata::output_format`, not dropped.
///
/// Extracted as its own pure function, mirroring `warnings_from_ocr_metadata`,
/// so the filtering is unit-testable without a live Tesseract API instance.
fn strip_ocr_scratch_metadata_keys(metadata: &mut std::collections::HashMap<String, serde_json::Value>) {
    metadata.remove(WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY);
    metadata.remove(AUTO_ROTATE_UNAVAILABLE_METADATA_KEY);
}

fn compact_cjk_horizontal_spacing(text: &str) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < chars.len() {
        if !matches!(chars[index], ' ' | '\t') {
            output.push(chars[index]);
            index += 1;
            continue;
        }

        let whitespace_start = index;
        while index < chars.len() && matches!(chars[index], ' ' | '\t') {
            index += 1;
        }
        let joins_cjk = output.chars().next_back().is_some_and(is_compact_cjk_char)
            && chars.get(index).copied().is_some_and(is_compact_cjk_char);
        if !joins_cjk {
            output.extend(chars[whitespace_start..index].iter());
        }
    }
    output
}

fn is_compact_cjk_char(character: char) -> bool {
    matches!(
        character as u32,
        0x2E80..=0x30FF | 0x31F0..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF
            | 0x20000..=0x2FA1F
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertical_cjk_spacing_removes_only_inter_character_horizontal_space() {
        assert_eq!(
            compact_cjk_horizontal_spacing("元 来 日 本 語 は 漢文 に 倣い 、 API 文書 。\n次 行"),
            "元来日本語は漢文に倣い、 API 文書。\n次行"
        );
    }

    #[test]
    fn vertical_cjk_spacing_preserves_latin_and_paragraph_whitespace() {
        assert_eq!(
            compact_cjk_horizontal_spacing("API 仕様\tversion 2\n\n次段落"),
            "API 仕様\tversion 2\n\n次段落"
        );
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test]
    async fn test_tesseract_backend_limits_concurrent_calls() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let backend = Arc::new(TesseractBackend::new());
        let active = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let mut tasks = Vec::new();
        for _ in 0..12 {
            let backend = Arc::clone(&backend);
            let active = Arc::clone(&active);
            let peak = Arc::clone(&peak);
            tasks.push(tokio::spawn(async move {
                let _permit = backend.concurrency.acquire().await.unwrap();
                let current = active.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(current, Ordering::SeqCst);
                tokio::task::yield_now().await;
                active.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), crate::ocr::processor::MAX_TESSERACT_APIS);
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn test_tesseract_permit_outlives_cancelled_async_caller() {
        let backend = Arc::new(TesseractBackend::new());
        let reserved = Arc::clone(&backend.concurrency)
            .acquire_many_owned((crate::ocr::processor::MAX_TESSERACT_APIS - 1) as u32)
            .await
            .unwrap();
        let rendezvous = Arc::new(std::sync::Barrier::new(2));
        let (done_tx, done_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn({
            let semaphore = Arc::clone(&backend.concurrency);
            let rendezvous = Arc::clone(&rendezvous);
            async move {
                let permit = semaphore.acquire_owned().await.unwrap();
                tokio::task::spawn_blocking(move || {
                    {
                        let _permit = permit;
                        rendezvous.wait();
                        rendezvous.wait();
                    }
                    let _ = done_tx.send(());
                })
                .await
                .unwrap();
            }
        });

        rendezvous.wait();
        task.abort();
        assert!(Arc::clone(&backend.concurrency).try_acquire_owned().is_err());
        rendezvous.wait();
        done_rx.await.unwrap();
        drop(reserved);

        assert_eq!(
            backend.concurrency.available_permits(),
            crate::ocr::processor::MAX_TESSERACT_APIS
        );
    }

    #[test]
    fn test_tesseract_backend_creation() {
        let backend = TesseractBackend::new();
        assert!(!backend.processor_is_initialized());
    }

    #[test]
    fn test_tesseract_backend_plugin_interface() {
        let backend = TesseractBackend::new();
        assert_eq!(backend.name(), "tesseract");
        assert!(!backend.version().is_empty());
        assert!(backend.initialize().is_ok());
    }

    #[test]
    fn test_tesseract_backend_type() {
        let backend = TesseractBackend::new();
        assert_eq!(backend.backend_type(), OcrBackendType::Tesseract);
    }

    #[test]
    fn test_tesseract_backend_supports_language() {
        let backend = TesseractBackend::new();
        assert!(backend.supports_language("eng"));
        assert!(!backend.supports_language("xyz"));
        assert!(!backend.supports_language("invalid"));
    }

    #[test]
    fn test_tesseract_backend_supports_table_detection() {
        let backend = TesseractBackend::new();
        assert!(backend.supports_table_detection());
    }

    #[test]
    fn test_tesseract_backend_supported_languages() {
        let backend = TesseractBackend::new();
        let languages = backend.supported_languages();
        assert!(languages.contains(&"eng".to_string()));
        assert!(!languages.is_empty());
    }

    #[test]
    fn test_fallback_languages_include_vertical_japanese() {
        assert!(
            TesseractBackend::fallback_languages()
                .iter()
                .any(|language| language == "jpn_vert")
        );
    }

    /// Issue #181: OCR-produced tables must carry a deterministic `table_id`,
    /// `columns`, and `bounding_box` — not `..Default::default()` blanks.
    #[test]
    fn convert_ocr_table_assigns_sequential_ids_columns_and_bounding_box() {
        let first = crate::types::OcrTable {
            cells: vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ],
            markdown: "| Name | Age |\n|---|---|\n| Alice | 30 |".to_string(),
            page_number: 1,
            bounding_box: Some(crate::types::OcrTableBoundingBox {
                left: 10,
                top: 20,
                right: 110,
                bottom: 220,
            }),
        };
        let second = crate::types::OcrTable {
            cells: vec![vec!["X".to_string()]],
            markdown: "| X |".to_string(),
            page_number: 2,
            bounding_box: None,
        };

        let converted_first = convert_ocr_table(0, first);
        let converted_second = convert_ocr_table(1, second);

        assert_eq!(converted_first.table_id.as_deref(), Some("table-1"));
        assert_eq!(
            converted_first.columns,
            Some(vec!["Name".to_string(), "Age".to_string()])
        );
        let bbox = converted_first.bounding_box.expect("bounding box must be populated");
        assert_eq!(bbox.x0, 10.0);
        assert_eq!(bbox.y0, 20.0);
        assert_eq!(bbox.x1, 110.0);
        assert_eq!(bbox.y1, 220.0);

        assert_eq!(converted_second.table_id.as_deref(), Some("table-2"));
        assert_eq!(converted_second.columns, Some(vec!["X".to_string()]));
        assert!(converted_second.bounding_box.is_none());
    }

    #[test]
    fn test_config_to_tesseract_with_none() {
        let backend = TesseractBackend::new();
        let ocr_config = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["deu".to_string()],
            ..Default::default()
        };

        let tess_config = backend.config_to_tesseract(&ocr_config);
        assert_eq!(tess_config.language, "deu");
        assert_eq!(tess_config.psm, InternalTesseractConfig::default().psm);
    }

    #[test]
    fn test_config_to_tesseract_with_some() {
        let backend = TesseractBackend::new();
        let custom_tess_config = crate::types::TesseractConfig {
            language: vec!["fra".to_string()],
            psm: 6,
            enable_table_detection: true,
            ..Default::default()
        };

        let ocr_config = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            tesseract_config: Some(custom_tess_config),
            ..Default::default()
        };

        let tess_config = backend.config_to_tesseract(&ocr_config);
        assert_eq!(tess_config.language, "fra");
        assert_eq!(tess_config.psm, 6);
        assert!(tess_config.enable_table_detection);
    }

    #[test]
    fn test_config_to_tesseract_defaults_empty_language_to_eng() {
        let backend = TesseractBackend::new();

        let ocr_config = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec![],
            ..Default::default()
        };
        assert_eq!(backend.config_to_tesseract(&ocr_config).language, "eng");

        let ocr_config_with_tess = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec![],
            tesseract_config: Some(crate::types::TesseractConfig {
                language: vec![],
                ..Default::default()
            }),
            ..Default::default()
        };
        assert_eq!(backend.config_to_tesseract(&ocr_config_with_tess).language, "eng");
    }

    #[test]
    fn test_tesseract_backend_default() {
        let backend = TesseractBackend::default();
        assert_eq!(backend.name(), "tesseract");
    }

    #[test]
    fn test_config_conversion_with_new_fields() {
        let backend = TesseractBackend::new();

        let preprocessing = crate::types::ImagePreprocessingConfig {
            target_dpi: 600,
            auto_rotate: false,
            deskew: true,
            denoise: true,
            contrast_enhance: true,
            binarization_method: "adaptive".to_string(),
            invert_colors: false,
        };

        let custom_tess_config = crate::types::TesseractConfig {
            language: vec!["eng".to_string()],
            psm: 6,
            output_format: "markdown".to_string(),
            oem: 1,
            min_confidence: 80.0,
            preprocessing: Some(preprocessing.clone()),
            tessedit_char_blacklist: "!@#$".to_string(),
            ..Default::default()
        };

        let ocr_config = OcrConfig {
            backend: "tesseract".to_string(),
            language: vec!["eng".to_string()],
            tesseract_config: Some(custom_tess_config),
            ..Default::default()
        };

        let tess_config = backend.config_to_tesseract(&ocr_config);

        assert_eq!(tess_config.oem, 1);
        assert_eq!(tess_config.min_confidence, 80.0);
        assert_eq!(tess_config.tessedit_char_blacklist, "!@#$");

        assert!(tess_config.preprocessing.is_some());
        let preproc = tess_config.preprocessing.unwrap();
        assert_eq!(preproc.target_dpi, 600);
        assert!(!preproc.auto_rotate);
        assert!(preproc.deskew);
        assert!(preproc.denoise);
        assert!(preproc.contrast_enhance);
        assert_eq!(preproc.binarization_method, "adaptive");
        assert!(!preproc.invert_colors);
    }

    #[test]
    fn test_convert_config_type_conversions() {
        let public_config = crate::types::TesseractConfig {
            language: vec!["eng".to_string()],
            psm: 6,
            oem: 3,
            table_column_threshold: 100,
            ..Default::default()
        };

        let internal_config = InternalTesseractConfig::from(&public_config);

        assert_eq!(internal_config.psm, 6u8);
        assert_eq!(internal_config.oem, 3u8);
        assert_eq!(internal_config.table_column_threshold, 100u32);
    }

    /// #309: a positive `word_iterator_skipped_count` must surface as a
    /// `tesseract`-sourced warning naming the exact word count lost, not a
    /// generic "something was dropped" message.
    #[test]
    fn warnings_from_ocr_metadata_flags_dropped_words_with_exact_count() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY.to_string(),
            serde_json::Value::Number(2.into()),
        );

        let warnings = warnings_from_ocr_metadata(&metadata);

        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].source, "tesseract");
        assert!(
            warnings[0].message.contains("2 word(s)"),
            "message must name the exact skipped count: {}",
            warnings[0].message
        );
    }

    /// A `word_iterator_skipped_count` of exactly zero must not produce a
    /// warning -- the metadata-insertion side already omits the key in that
    /// case, but this guards the consumption side independently (#309).
    #[test]
    fn warnings_from_ocr_metadata_ignores_zero_skipped_count() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY.to_string(),
            serde_json::Value::Number(0.into()),
        );

        assert!(warnings_from_ocr_metadata(&metadata).is_empty());
    }

    /// #309: an `auto_rotate_unavailable` metadata flag must surface as a
    /// `tesseract`-sourced warning naming the `auto-rotate` feature, so a user
    /// who asked for rotation correction learns it never ran.
    #[test]
    fn warnings_from_ocr_metadata_flags_auto_rotate_unavailable() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            AUTO_ROTATE_UNAVAILABLE_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );

        let warnings = warnings_from_ocr_metadata(&metadata);

        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].source, "tesseract");
        assert!(
            warnings[0].message.contains("auto-rotate"),
            "message must name the missing feature: {}",
            warnings[0].message
        );
    }

    /// A clean extraction (neither metadata key present) must produce no
    /// warnings at all -- the whole point of the deduped, source-scoped
    /// warning convention is silence on the happy path (#309).
    #[test]
    fn warnings_from_ocr_metadata_is_silent_on_clean_extraction() {
        let metadata = std::collections::HashMap::new();
        assert!(warnings_from_ocr_metadata(&metadata).is_empty());
    }

    /// Both signals can fire together (a garbled page that also requested
    /// rotation on a build without the feature); both warnings must be kept,
    /// not just the first one found (#309).
    #[test]
    fn warnings_from_ocr_metadata_keeps_both_warnings_when_both_signals_fire() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY.to_string(),
            serde_json::Value::Number(1.into()),
        );
        metadata.insert(
            AUTO_ROTATE_UNAVAILABLE_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );

        assert_eq!(warnings_from_ocr_metadata(&metadata).len(), 2);
    }

    /// Round-trip test for the metadata -> warnings propagation path: the
    /// on-disk OCR cache (`ocr/cache.rs`) serializes `OcrExtractionResult`,
    /// including this `metadata` map, with `rmp_serde::to_vec_named` and
    /// decodes it back on a cache hit. This proves the two side-channel keys
    /// survive that exact round-trip and still produce the same warnings, so
    /// a cache hit surfaces the same `ProcessingWarning`s as a cache miss
    /// (#309).
    #[test]
    fn warnings_from_ocr_metadata_survives_msgpack_cache_round_trip() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY.to_string(),
            serde_json::Value::Number(5.into()),
        );
        metadata.insert(
            AUTO_ROTATE_UNAVAILABLE_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );

        let serialized = rmp_serde::to_vec_named(&metadata).expect("metadata must serialize for the OCR cache");
        let round_tripped: std::collections::HashMap<String, serde_json::Value> =
            rmp_serde::from_slice(&serialized).expect("metadata must deserialize from the OCR cache");

        let before = warnings_from_ocr_metadata(&metadata);
        let after = warnings_from_ocr_metadata(&round_tripped);
        assert_eq!(before.len(), 2);
        assert_eq!(after.len(), 2);
        assert_eq!(before[0].source, after[0].source);
        assert_eq!(before[0].message, after[0].message);
        assert_eq!(before[1].source, after[1].source);
        assert_eq!(before[1].message, after[1].message);
    }

    /// #354: `word_iterator_skipped_count` is pipeline plumbing consumed by
    /// `warnings_from_ocr_metadata` -- it must not survive into the
    /// user-visible metadata map that becomes `Metadata::additional`.
    #[test]
    fn strip_ocr_scratch_metadata_keys_removes_word_iterator_skipped_count() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY.to_string(),
            serde_json::Value::Number(2.into()),
        );

        strip_ocr_scratch_metadata_keys(&mut metadata);

        assert!(!metadata.contains_key(WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY));
    }

    /// #354: `auto_rotate_unavailable` is pipeline plumbing consumed by
    /// `warnings_from_ocr_metadata` -- it must not survive into the
    /// user-visible metadata map that becomes `Metadata::additional`.
    #[test]
    fn strip_ocr_scratch_metadata_keys_removes_auto_rotate_unavailable() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            AUTO_ROTATE_UNAVAILABLE_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );

        strip_ocr_scratch_metadata_keys(&mut metadata);

        assert!(!metadata.contains_key(AUTO_ROTATE_UNAVAILABLE_METADATA_KEY));
    }

    /// #354: `pre_formatted` is promoted to `Metadata::output_format`, so it
    /// must be removed from the metadata map (not merely read), or it would
    /// also leak into `Metadata::additional` once the remaining map is
    /// copied wholesale.
    #[test]
    fn extract_pre_formatted_metadata_removes_key_and_returns_value() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert(
            PRE_FORMATTED_METADATA_KEY.to_string(),
            serde_json::Value::String("markdown".to_string()),
        );

        let extracted = extract_pre_formatted_metadata(&mut metadata);

        assert_eq!(extracted.as_deref(), Some("markdown"));
        assert!(!metadata.contains_key(PRE_FORMATTED_METADATA_KEY));
    }

    /// #354 must not over-fire: genuine document metadata that happens to
    /// share the map with the scratch keys has to survive the filter intact,
    /// both in value and key set, so callers still see it in
    /// `Metadata::additional`.
    #[test]
    fn strip_ocr_scratch_metadata_keys_preserves_genuine_user_metadata() {
        let mut metadata = std::collections::HashMap::new();
        metadata.insert("language".to_string(), serde_json::Value::String("eng".to_string()));
        metadata.insert("mean_text_conf".to_string(), serde_json::Value::Number(87.into()));
        metadata.insert(
            WORD_ITERATOR_SKIPPED_COUNT_METADATA_KEY.to_string(),
            serde_json::Value::Number(1.into()),
        );
        metadata.insert(
            AUTO_ROTATE_UNAVAILABLE_METADATA_KEY.to_string(),
            serde_json::Value::Bool(true),
        );

        strip_ocr_scratch_metadata_keys(&mut metadata);

        assert_eq!(metadata.len(), 2);
        assert_eq!(
            metadata.get("language"),
            Some(&serde_json::Value::String("eng".to_string()))
        );
        assert_eq!(
            metadata.get("mean_text_conf"),
            Some(&serde_json::Value::Number(87.into()))
        );
    }

    #[test]
    fn tesseract_backend_does_not_eagerly_allocate_processor() {
        let backend = TesseractBackend::new();
        assert!(
            !backend.processor_is_initialized(),
            "TesseractBackend::new() should not eagerly allocate the processor"
        );
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod probe_tests {
    use super::*;
    use crate::doctor::ProbeStatus;

    fn config_with_tessdata(path: &Path, languages: &[&str]) -> OcrConfig {
        OcrConfig {
            tessdata_path: Some(path.to_path_buf()),
            language: languages.iter().map(|l| l.to_string()).collect(),
            ..OcrConfig::default()
        }
    }

    fn probe_with_dirs(config: &OcrConfig, dirs: &[&Path]) -> crate::doctor::DoctorCheck {
        let dirs: Vec<String> = dirs.iter().map(|d| d.to_string_lossy().into_owned()).collect();
        probe_tessdata_in_dirs(config, &dirs)
    }

    #[test]
    fn probe_passes_when_all_languages_present() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("eng.traineddata"), b"fake").unwrap();
        let check = probe_with_dirs(&config_with_tessdata(dir.path(), &["eng"]), &[dir.path()]);
        assert_eq!(check.status, ProbeStatus::Pass);
        assert!(check.message.contains(dir.path().to_str().unwrap()));
    }

    #[test]
    fn probe_skips_downloadable_language() {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::write(dir.path().join("eng.traineddata"), b"fake").unwrap();
        let check = probe_with_dirs(&config_with_tessdata(dir.path(), &["eng", "deu"]), &[dir.path()]);
        assert_eq!(check.status, ProbeStatus::Skip);
        assert!(check.message.contains("deu"));
    }

    #[test]
    fn probe_fails_on_unknown_language_code() {
        let dir = tempfile::TempDir::new().unwrap();
        let check = probe_with_dirs(&config_with_tessdata(dir.path(), &["xx9"]), &[dir.path()]);
        assert_eq!(check.status, ProbeStatus::Fail);
        assert!(check.message.contains("xx9"));
    }
}
