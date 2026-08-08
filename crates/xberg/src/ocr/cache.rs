use super::error::OcrError;
use super::utils::compute_hash;
use crate::cache::version::cache_version_tag;
use crate::core::config::OutputFormat;
use crate::telemetry::conventions;
use crate::types::OcrExtractionResult;
use crate::types::internal::InternalDocument;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

/// On-disk envelope for a cached OCR result.
///
/// [`OcrExtractionResult::internal_document`] is `#[serde(skip)]`, so
/// serializing the result on its own silently drops the structured hOCR
/// document — the paragraph structure, bounding boxes and confidence scores that
/// the flattened `content` string does not carry. A cache hit would then return
/// strictly less than a cache miss for the same input, making extraction output
/// depend on cache state.
///
/// The envelope carries that field explicitly alongside the result so the
/// round-trip is lossless.
#[derive(Debug, Deserialize)]
struct CachedOcrResult {
    /// Everything `OcrExtractionResult`'s own `Serialize` impl preserves.
    result: OcrExtractionResult,
    /// The `#[serde(skip)]` structured document, carried out of band.
    #[serde(default)]
    internal_document: Option<InternalDocument>,
}

/// Borrowing counterpart of [`CachedOcrResult`] used on the write path.
///
/// Written with `rmp_serde::to_vec_named`, so both this and [`CachedOcrResult`]
/// encode as maps keyed by field name. Positional encoding cannot be used here:
/// `OcrExtractionResult` carries `skip_serializing_if`, so the write emits fewer
/// array elements than the read expects and every later field shifts by one.
#[derive(Debug, Serialize)]
struct CachedOcrResultRef<'a> {
    result: &'a OcrExtractionResult,
    internal_document: &'a Option<InternalDocument>,
}

impl CachedOcrResult {
    /// Reattach the out-of-band structured document to the decoded result.
    fn into_result(self) -> OcrExtractionResult {
        let mut result = self.result;
        result.internal_document = self.internal_document;
        result
    }
}

/// File-backed msgpack cache for OCR extraction results, keyed by image + config hash.
#[cfg_attr(alef, alef(skip))]
pub struct OcrCache {
    cache_dir: PathBuf,
}

impl OcrCache {
    pub(crate) fn new(cache_dir: Option<PathBuf>) -> Result<Self, OcrError> {
        let cache_dir = cache_dir.unwrap_or_else(|| crate::cache_dir::resolve_cache_dir("ocr"));

        Ok(Self { cache_dir })
    }

    fn get_cache_path(&self, cache_key: &str) -> PathBuf {
        self.cache_dir.join(format!("{}.msgpack", cache_key))
    }

    pub(crate) fn get_cached_result(
        &self,
        image_hash: &str,
        backend: &str,
        config: &str,
        output_format: Option<&OutputFormat>,
    ) -> Result<Option<OcrExtractionResult>, OcrError> {
        let cache_key = self.generate_cache_key(image_hash, backend, config, output_format);
        let cache_path = self.get_cache_path(&cache_key);

        if !cache_path.exists() {
            Self::record_lookup(&cache_key, false);
            return Ok(None);
        }

        let cached_bytes = fs::read(&cache_path).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_LOOKUP,
                { conventions::CACHE_KEY } = cache_key.as_str(),
                { conventions::OCR_BACKEND } = backend,
                error = %e,
                "Failed to read an existing OCR cache entry"
            );
            OcrError::CacheError(format!("Failed to read cache file: {}", e))
        })?;

        match rmp_serde::from_slice::<CachedOcrResult>(&cached_bytes) {
            Ok(cached) => {
                Self::record_lookup(&cache_key, true);
                Ok(Some(cached.into_result()))
            }
            Err(e) => {
                tracing::warn!(
                    { conventions::OPERATION } = conventions::operations::CACHE_LOOKUP,
                    { conventions::CACHE_KEY } = cache_key.as_str(),
                    { conventions::OCR_BACKEND } = backend,
                    error = %e,
                    "Discarding an undecodable OCR cache entry and re-running OCR"
                );
                if let Err(e) = fs::remove_file(&cache_path) {
                    tracing::debug!("Failed to remove the undecodable OCR cache entry: {}", e);
                }
                Self::record_lookup(&cache_key, false);
                Ok(None)
            }
        }
    }

    pub(crate) fn set_cached_result(
        &self,
        image_hash: &str,
        backend: &str,
        config: &str,
        output_format: Option<&OutputFormat>,
        result: &OcrExtractionResult,
    ) -> Result<(), OcrError> {
        let cache_key = self.generate_cache_key(image_hash, backend, config, output_format);
        let cache_path = self.get_cache_path(&cache_key);

        fs::create_dir_all(&self.cache_dir).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = cache_key.as_str(),
                error = %e,
                "Failed to create the OCR cache directory; the result will not be cached"
            );
            OcrError::CacheError(format!("Failed to create cache directory: {}", e))
        })?;

        // Serialize the envelope, not the bare result: the result's own
        // `Serialize` impl drops `internal_document`.
        let envelope = CachedOcrResultRef {
            result,
            internal_document: &result.internal_document,
        };
        let serialized = rmp_serde::to_vec_named(&envelope).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = cache_key.as_str(),
                error = %e,
                "Failed to serialize the OCR result; it will not be cached"
            );
            OcrError::CacheError(format!("Failed to serialize result: {}", e))
        })?;

        let pid = std::process::id();
        let thread_id = std::thread::current().id();
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let temp_name = format!("{}.tmp.{}.{:?}.{}", cache_key, pid, thread_id, timestamp);
        let temp_path = self.cache_dir.join(temp_name);

        fs::write(&temp_path, &serialized).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = cache_key.as_str(),
                error = %e,
                "Failed to write the temporary OCR cache file; the result will not be cached"
            );
            OcrError::CacheError(format!("Failed to write temp cache file: {}", e))
        })?;

        fs::rename(&temp_path, &cache_path).map_err(|e| {
            tracing::warn!(
                { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
                { conventions::CACHE_KEY } = cache_key.as_str(),
                error = %e,
                "Failed to publish the OCR cache entry; the result will not be cached"
            );
            if let Err(e) = fs::remove_file(&temp_path) {
                tracing::debug!("Failed to clean up the temporary OCR cache file: {}", e);
            }
            OcrError::CacheError(format!("Failed to rename cache file: {}", e))
        })?;

        tracing::debug!(
            { conventions::OPERATION } = conventions::operations::CACHE_WRITE,
            { conventions::CACHE_KEY } = cache_key.as_str(),
            { conventions::OCR_BACKEND } = backend,
            size_bytes = serialized.len(),
            "OCR cache write"
        );

        Ok(())
    }

    /// Emit OCR cache-lookup telemetry.
    fn record_lookup(cache_key: &str, hit: bool) {
        #[cfg(feature = "otel")]
        {
            let metrics = crate::telemetry::metrics::get_metrics();
            if hit {
                metrics.cache_hits.add(1, &[]);
            } else {
                metrics.cache_misses.add(1, &[]);
            }
        }

        tracing::debug!(
            { conventions::OPERATION } = conventions::operations::CACHE_LOOKUP,
            { conventions::CACHE_KEY } = cache_key,
            { conventions::CACHE_HIT } = hit,
            "OCR cache lookup"
        );
    }

    /// Stable tag for the requested output format, as it appears in the cache key.
    ///
    /// The format is not part of `TesseractConfig`, but it changes the result:
    /// it selects the renderer and therefore the `content` string and the
    /// `mime_type` of the OCR result. Leaving it out of the key serves a
    /// document cached as one format when another was requested.
    fn output_format_tag(output_format: Option<&OutputFormat>) -> String {
        match output_format {
            Some(format) => format!("{:?}", format),
            None => "none".to_string(),
        }
    }

    fn generate_cache_key(
        &self,
        image_hash: &str,
        backend: &str,
        config: &str,
        output_format: Option<&OutputFormat>,
    ) -> String {
        let cache_string = format!(
            "cache_version={}&image_hash={}&ocr_backend={}&ocr_config={}&output_format={}",
            cache_version_tag(),
            image_hash,
            backend,
            config,
            Self::output_format_tag(output_format),
        );

        compute_hash(&cache_string)
    }

    pub(crate) fn clear(&self) -> Result<(), OcrError> {
        if !self.cache_dir.exists() {
            return Ok(());
        }

        let entries = fs::read_dir(&self.cache_dir)
            .map_err(|e| OcrError::CacheError(format!("Failed to read cache directory: {}", e)))?;

        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension()
                && ext == "msgpack"
            {
                let _ = fs::remove_file(entry.path());
            }
        }

        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn get_stats(&self) -> Result<OcrCacheStats, OcrError> {
        if !self.cache_dir.exists() {
            return Ok(OcrCacheStats::default());
        }

        let entries = fs::read_dir(&self.cache_dir)
            .map_err(|e| OcrError::CacheError(format!("Failed to read cache directory: {}", e)))?;

        let mut total_files = 0;
        let mut total_size_bytes = 0u64;

        for entry in entries.flatten() {
            if let Some(ext) = entry.path().extension()
                && ext == "msgpack"
            {
                total_files += 1;
                if let Ok(metadata) = entry.metadata() {
                    total_size_bytes += metadata.len();
                }
            }
        }

        Ok(OcrCacheStats {
            total_files,
            total_size_mb: total_size_bytes as f64 / 1024.0 / 1024.0,
        })
    }
}

/// Statistics about the current state of the OCR result cache.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Default)]
pub struct OcrCacheStats {
    /// Number of msgpack cache files on disk.
    pub total_files: usize,
    /// Total size of all cache files in megabytes.
    pub total_size_mb: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    #[test]
    fn test_cache_get_set() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result = OcrExtractionResult {
            content: "Test OCR result".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("abc123", "tesseract", "eng", None, &result)
            .unwrap();

        let cached = cache.get_cached_result("abc123", "tesseract", "eng", None).unwrap();

        assert!(cached.is_some());
        assert_eq!(cached.unwrap().content, "Test OCR result");
    }

    #[test]
    fn test_cache_miss() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let cached = cache
            .get_cached_result("nonexistent", "tesseract", "eng", None)
            .unwrap();

        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_clear() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result = OcrExtractionResult {
            content: "Test".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("test", "tesseract", "eng", None, &result)
            .unwrap();

        cache.clear().unwrap();

        let cached = cache.get_cached_result("test", "tesseract", "eng", None).unwrap();
        assert!(cached.is_none());
    }

    #[test]
    fn test_cache_stats() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let stats = cache.get_stats().unwrap();
        assert_eq!(stats.total_files, 0);

        let result = OcrExtractionResult {
            content: "Test".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("test", "tesseract", "eng", None, &result)
            .unwrap();

        let stats = cache.get_stats().unwrap();
        assert_eq!(stats.total_files, 1);
        assert!(stats.total_size_mb > 0.0);
    }

    #[test]
    fn test_cache_key_generation_deterministic() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let key1 = cache.generate_cache_key("abc123", "tesseract", "eng", None);
        let key2 = cache.generate_cache_key("abc123", "tesseract", "eng", None);

        assert_eq!(key1, key2);
        assert_eq!(key1.len(), 32);
    }

    #[test]
    fn test_cache_key_generation_different_inputs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let key1 = cache.generate_cache_key("abc123", "tesseract", "eng", None);
        let key2 = cache.generate_cache_key("def456", "tesseract", "eng", None);
        let key3 = cache.generate_cache_key("abc123", "paddleocr", "eng", None);
        let key4 = cache.generate_cache_key("abc123", "tesseract", "fra", None);

        assert_ne!(key1, key2);
        assert_ne!(key1, key3);
        assert_ne!(key1, key4);
    }

    #[test]
    fn test_cache_multiple_entries() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result1 = OcrExtractionResult {
            content: "First".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        let result2 = OcrExtractionResult {
            content: "Second".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("hash1", "tesseract", "eng", None, &result1)
            .unwrap();
        cache
            .set_cached_result("hash2", "tesseract", "eng", None, &result2)
            .unwrap();

        let stats = cache.get_stats().unwrap();
        assert_eq!(stats.total_files, 2);

        let retrieved1 = cache.get_cached_result("hash1", "tesseract", "eng", None).unwrap();
        let retrieved2 = cache.get_cached_result("hash2", "tesseract", "eng", None).unwrap();

        assert_eq!(retrieved1.unwrap().content, "First");
        assert_eq!(retrieved2.unwrap().content, "Second");
    }

    #[test]
    fn test_cache_overwrite() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result1 = OcrExtractionResult {
            content: "Original".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        let result2 = OcrExtractionResult {
            content: "Updated".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("test", "tesseract", "eng", None, &result1)
            .unwrap();
        cache
            .set_cached_result("test", "tesseract", "eng", None, &result2)
            .unwrap();

        let retrieved = cache.get_cached_result("test", "tesseract", "eng", None).unwrap();
        assert_eq!(retrieved.unwrap().content, "Updated");

        let stats = cache.get_stats().unwrap();
        assert_eq!(stats.total_files, 1);
    }

    #[test]
    fn test_cache_with_tables() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        use crate::types::OcrTable;

        let table = OcrTable {
            cells: vec![vec!["A".to_string(), "B".to_string()]],
            markdown: "| A | B |".to_string(),
            page_number: 0,
            bounding_box: None,
        };

        let result = OcrExtractionResult {
            content: "Content with table".to_string(),
            mime_type: "text/markdown".to_string(),
            metadata: HashMap::new(),
            tables: vec![table],
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("test", "tesseract", "eng", None, &result)
            .unwrap();

        let retrieved = cache
            .get_cached_result("test", "tesseract", "eng", None)
            .unwrap()
            .unwrap();
        assert_eq!(retrieved.tables.len(), 1);
        assert_eq!(retrieved.tables[0].cells[0][0], "A");
    }

    #[test]
    fn test_cache_with_metadata() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let mut metadata = HashMap::new();
        metadata.insert("language".to_string(), serde_json::Value::String("eng".to_string()));
        metadata.insert("confidence".to_string(), serde_json::Value::String("95.5".to_string()));

        let result = OcrExtractionResult {
            content: "Content".to_string(),
            mime_type: "text/plain".to_string(),
            metadata,
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("test", "tesseract", "eng", None, &result)
            .unwrap();

        let retrieved = cache
            .get_cached_result("test", "tesseract", "eng", None)
            .unwrap()
            .unwrap();
        assert_eq!(
            retrieved.metadata.get("language").unwrap(),
            &serde_json::Value::String("eng".to_string())
        );
        assert_eq!(
            retrieved.metadata.get("confidence").unwrap(),
            &serde_json::Value::String("95.5".to_string())
        );
    }

    #[test]
    fn test_cache_clear_selective() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result = OcrExtractionResult {
            content: "Test".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("test1", "tesseract", "eng", None, &result)
            .unwrap();
        cache
            .set_cached_result("test2", "tesseract", "eng", None, &result)
            .unwrap();

        fs::write(temp_dir.path().join("other.txt"), "not a msgpack file").unwrap();

        cache.clear().unwrap();

        assert!(
            cache
                .get_cached_result("test1", "tesseract", "eng", None)
                .unwrap()
                .is_none()
        );
        assert!(
            cache
                .get_cached_result("test2", "tesseract", "eng", None)
                .unwrap()
                .is_none()
        );

        assert!(temp_dir.path().join("other.txt").exists());
    }

    #[test]
    fn test_new_does_not_create_cache_dir_when_caching_unused() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("ocr-cache-disabled");

        let _cache = OcrCache::new(Some(cache_dir.clone())).unwrap();

        assert!(
            !cache_dir.exists(),
            "OcrCache::new must not create the cache directory eagerly"
        );
    }

    #[test]
    fn test_set_cached_result_creates_cache_dir_lazily() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_dir = temp_dir.path().join("ocr-cache-enabled");
        let cache = OcrCache::new(Some(cache_dir.clone())).unwrap();

        assert!(!cache_dir.exists(), "cache dir must not exist before the first write");

        let result = OcrExtractionResult {
            content: "Lazy dir creation".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("lazy", "tesseract", "eng", None, &result)
            .unwrap();

        assert!(cache_dir.exists(), "cache dir must be created on first cached write");

        let retrieved = cache.get_cached_result("lazy", "tesseract", "eng", None).unwrap();
        assert_eq!(retrieved.unwrap().content, "Lazy dir creation");
    }

    #[test]
    fn test_cache_stats_nonexistent_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_path = temp_dir.path().join("nonexistent");
        let cache = OcrCache { cache_dir: cache_path };

        let stats = cache.get_stats().unwrap();
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_size_mb, 0.0);
    }

    #[test]
    fn test_cache_clear_nonexistent_dir() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache_path = temp_dir.path().join("nonexistent");
        let cache = OcrCache { cache_dir: cache_path };

        assert!(cache.clear().is_ok());
    }

    #[test]
    fn test_cache_get_path() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let path = cache.get_cache_path("abc123");

        assert!(path.to_string_lossy().contains("abc123.msgpack"));
        assert_eq!(path.parent().unwrap(), temp_dir.path());
    }

    #[test]
    fn test_cache_stats_default() {
        let stats = OcrCacheStats::default();
        assert_eq!(stats.total_files, 0);
        assert_eq!(stats.total_size_mb, 0.0);
    }

    #[test]
    fn test_cache_empty_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result = OcrExtractionResult {
            content: String::new(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("empty", "tesseract", "eng", None, &result)
            .unwrap();

        let retrieved = cache.get_cached_result("empty", "tesseract", "eng", None).unwrap();
        assert_eq!(retrieved.unwrap().content, "");
    }

    /// An OCR result carrying a structured hOCR document.
    fn result_with_internal_document() -> OcrExtractionResult {
        OcrExtractionResult {
            content: "Flattened text".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: Some(InternalDocument {
                source_format: "hocr".to_string(),
                mime_type: "text/plain".to_string(),
                ..Default::default()
            }),
        }
    }

    // ---------------------------------------------------------------------
    // #205 — `output_format` is missing from the OCR cache key.
    // ---------------------------------------------------------------------

    #[test]
    fn cache_key_should_differ_when_only_the_output_format_differs() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let plain = cache.generate_cache_key("abc123", "tesseract", "eng", Some(&OutputFormat::Plain));
        let markdown = cache.generate_cache_key("abc123", "tesseract", "eng", Some(&OutputFormat::Markdown));
        let unspecified = cache.generate_cache_key("abc123", "tesseract", "eng", None);

        assert_ne!(plain, markdown, "Plain and Markdown must not share a cache key");
        assert_ne!(plain, unspecified, "an unspecified format must not alias Plain");
        assert_ne!(markdown, unspecified, "an unspecified format must not alias Markdown");
        assert_eq!(plain.len(), 32);
        assert_eq!(markdown.len(), 32);
    }

    #[test]
    fn cache_key_should_be_identical_for_identical_inputs_including_the_output_format() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        assert_eq!(
            cache.generate_cache_key("abc123", "tesseract", "eng", Some(&OutputFormat::Markdown)),
            cache.generate_cache_key("abc123", "tesseract", "eng", Some(&OutputFormat::Markdown)),
        );
        assert_eq!(
            cache.generate_cache_key("abc123", "tesseract", "eng", None),
            cache.generate_cache_key("abc123", "tesseract", "eng", None),
        );
    }

    #[test]
    fn a_markdown_request_should_not_be_served_the_plain_cached_result() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let plain_result = OcrExtractionResult {
            content: "Heading\nbody".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("img", "tesseract", "eng", Some(&OutputFormat::Plain), &plain_result)
            .unwrap();

        assert_eq!(
            cache
                .get_cached_result("img", "tesseract", "eng", Some(&OutputFormat::Markdown))
                .unwrap()
                .map(|r| r.content),
            None,
            "the Plain entry must not satisfy a Markdown request"
        );
        assert_eq!(
            cache
                .get_cached_result("img", "tesseract", "eng", Some(&OutputFormat::Plain))
                .unwrap()
                .map(|r| r.content),
            Some("Heading\nbody".to_string()),
            "the Plain entry must still satisfy a Plain request"
        );
    }

    // ---------------------------------------------------------------------
    // #174 / #207 — the OCR cache discards the structured document, so a
    // cache hit returns less than a cache miss for the same input.
    // ---------------------------------------------------------------------

    #[test]
    fn cached_result_should_preserve_the_internal_document() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let original = result_with_internal_document();
        cache
            .set_cached_result("img", "tesseract", "eng", None, &original)
            .unwrap();

        let cached = cache
            .get_cached_result("img", "tesseract", "eng", None)
            .unwrap()
            .expect("the entry must be a hit");

        let document = cached
            .internal_document
            .expect("the structured document must survive the cache round-trip");
        assert_eq!(document.source_format, "hocr");
        assert_eq!(document.mime_type, "text/plain");
    }

    #[test]
    fn cache_hit_should_equal_cache_miss_for_the_same_input() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        // The value a cache miss would produce.
        let fresh = result_with_internal_document();
        cache
            .set_cached_result("img", "tesseract", "eng", None, &fresh)
            .unwrap();

        let hit = cache
            .get_cached_result("img", "tesseract", "eng", None)
            .unwrap()
            .expect("the entry must be a hit");

        assert_eq!(hit.content, fresh.content);
        assert_eq!(hit.mime_type, fresh.mime_type);
        assert_eq!(hit.tables.len(), fresh.tables.len());
        assert_eq!(
            hit.internal_document.is_some(),
            fresh.internal_document.is_some(),
            "a cache hit must not be structurally poorer than a cache miss"
        );
    }

    #[test]
    fn a_result_without_an_internal_document_should_round_trip_as_none() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let result = OcrExtractionResult {
            content: "no structure".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };
        cache
            .set_cached_result("img", "tesseract", "eng", None, &result)
            .unwrap();

        let cached = cache
            .get_cached_result("img", "tesseract", "eng", None)
            .unwrap()
            .expect("the entry must be a hit");
        assert_eq!(cached.content, "no structure");
        assert!(cached.internal_document.is_none());
    }

    #[test]
    fn a_legacy_bare_result_entry_should_be_discarded_instead_of_returned() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let legacy = OcrExtractionResult {
            content: "written by an older build".to_string(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        // Old on-disk shape: the bare result, without the envelope.
        let cache_key = cache.generate_cache_key("img", "tesseract", "eng", None);
        fs::create_dir_all(temp_dir.path()).unwrap();
        fs::write(cache.get_cache_path(&cache_key), rmp_serde::to_vec(&legacy).unwrap()).unwrap();

        assert!(
            cache
                .get_cached_result("img", "tesseract", "eng", None)
                .unwrap()
                .is_none(),
            "an entry in the pre-envelope format must be discarded, not misread"
        );
        assert!(
            !cache.get_cache_path(&cache_key).exists(),
            "the undecodable entry must be removed"
        );
    }

    #[test]
    fn cache_key_should_be_scoped_to_the_build_version() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let key = cache.generate_cache_key("abc123", "tesseract", "eng", None);
        let unversioned = compute_hash("image_hash=abc123&ocr_backend=tesseract&ocr_config=eng");

        assert_ne!(
            key, unversioned,
            "the OCR cache key must be scoped to the build version tag"
        );
    }

    #[test]
    fn test_cache_large_content() {
        let temp_dir = tempfile::tempdir().unwrap();
        let cache = OcrCache::new(Some(temp_dir.path().to_path_buf())).unwrap();

        let large_content = "x".repeat(10_000);

        let result = OcrExtractionResult {
            content: large_content.clone(),
            mime_type: "text/plain".to_string(),
            metadata: HashMap::new(),
            tables: Vec::new(),
            ocr_elements: None,
            internal_document: None,
        };

        cache
            .set_cached_result("large", "tesseract", "eng", None, &result)
            .unwrap();

        let retrieved = cache.get_cached_result("large", "tesseract", "eng", None).unwrap();
        assert_eq!(retrieved.unwrap().content.len(), 10_000);
    }
}
