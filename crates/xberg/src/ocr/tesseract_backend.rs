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

        let pre_formatted = ocr_result
            .metadata
            .get("pre_formatted")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

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
                .map(|t| {
                    let bounding_box = t.bounding_box.map(|bbox| crate::types::BoundingBox {
                        x0: bbox.left as f64,
                        y0: bbox.top as f64,
                        x1: bbox.right as f64,
                        y1: bbox.bottom as f64,
                    });
                    crate::types::Table {
                        cells: t.cells,
                        markdown: t.markdown,
                        page_number: t.page_number,
                        bounding_box,
                        ..Default::default()
                    }
                })
                .collect(),
            ocr_elements: ocr_result.ocr_elements,
            ocr_internal_document: ocr_result.internal_document,
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

        let pre_formatted = ocr_result
            .metadata
            .get("pre_formatted")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());

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
                .map(|t| {
                    let bounding_box = t.bounding_box.map(|bbox| crate::types::BoundingBox {
                        x0: bbox.left as f64,
                        y0: bbox.top as f64,
                        x1: bbox.right as f64,
                        y1: bbox.bottom as f64,
                    });
                    crate::types::Table {
                        cells: t.cells,
                        markdown: t.markdown,
                        page_number: t.page_number,
                        bounding_box,
                        ..Default::default()
                    }
                })
                .collect(),
            ocr_elements: ocr_result.ocr_elements,
            ocr_internal_document: ocr_result.internal_document,
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
