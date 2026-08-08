//! OCR backend plugin trait.
//!
//! This module defines the trait for implementing custom OCR backends.

use crate::Result;
use crate::core::config::OcrConfig;
use crate::plugins::Plugin;
use crate::types::ExtractedDocument;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

#[cfg(not(feature = "tokio-runtime"))]
use crate::XbergError;

/// OCR backend types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize, serde::Serialize)]
pub enum OcrBackendType {
    /// Tesseract OCR (native Rust binding)
    #[default]
    Tesseract,
    /// PaddleOCR (Python-based, via FFI)
    PaddleOCR,
    /// Candle-based VLM OCR (TrOCR, PaddleOCR-VL).
    Candle,
    /// Name-selected built-in or third-party OCR backend.
    Custom,
}

/// Trait for OCR backend plugins.
///
/// Implement this trait to add custom OCR capabilities. OCR backends can be:
/// - Native Rust implementations (like Tesseract)
/// - FFI bridges to external libraries (like PaddleOCR)
/// - Cloud-based OCR services (Google Vision, AWS Textract, etc.)
///
/// # Thread Safety
///
/// OCR backends must be thread-safe (`Send + Sync`) to support concurrent processing.
///
/// # Example
///
/// ```rust
/// use xberg::plugins::{Plugin, OcrBackend, OcrBackendType};
/// use xberg::{Result, OcrConfig};
/// use async_trait::async_trait;
/// use std::borrow::Cow;
/// use std::path::Path;
/// use xberg::types::{ExtractedDocument, Metadata};
///
/// struct CustomOcrBackend;
///
/// impl Plugin for CustomOcrBackend {
///     fn name(&self) -> &str { "custom-ocr" }
///     fn version(&self) -> String { "1.0.0".to_string() }
///     fn initialize(&self) -> Result<()> { Ok(()) }
///     fn shutdown(&self) -> Result<()> { Ok(()) }
/// }
///
/// #[async_trait]
/// impl OcrBackend for CustomOcrBackend {
///     async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
///         // Implement OCR logic here
///         let mut document = ExtractedDocument::default();
///         document.content = "Extracted text".to_string();
///         document.mime_type = Cow::Borrowed("text/plain");
///         Ok(document)
///     }
///
///     async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
///         let bytes = std::fs::read(path)?;
///         self.process_image(&bytes, config).await
///     }
///
///     fn supports_language(&self, lang: &str) -> bool {
///         matches!(lang, "eng" | "deu" | "fra")
///     }
///
///     fn backend_type(&self) -> OcrBackendType {
///         OcrBackendType::Custom
///     }
/// }
/// ```
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait OcrBackend: Plugin {
    /// Process an image and extract text via OCR.
    ///
    /// # Arguments
    ///
    /// * `image_bytes` - Raw image data (JPEG, PNG, TIFF, etc.)
    /// * `config` - OCR configuration (language, PSM mode, etc.)
    ///
    /// # Returns
    ///
    /// An `ExtractedDocument` containing the extracted text and metadata.
    ///
    /// # Errors
    ///
    /// - `XbergError::Ocr` - OCR processing failed
    /// - `XbergError::Validation` - Invalid image format or configuration
    /// - `XbergError::Io` - I/O errors (these always bubble up)
    ///
    /// # Reading `backend_options`
    ///
    /// Backends that support runtime tuning can read `config.backend_options` and
    /// deserialize only the keys they care about. Unknown keys are silently ignored,
    /// so multiple backends can coexist in a pipeline without key conflicts.
    ///
    /// ```rust
    /// # use xberg::plugins::{Plugin, OcrBackend};
    /// # use xberg::{Result, OcrConfig};
    /// # use async_trait::async_trait;
    /// # use std::borrow::Cow;
    /// # use std::path::Path;
    /// # use xberg::types::{ExtractedDocument, Metadata};
    /// # struct MyOcr;
    /// # impl Plugin for MyOcr {
    /// #     fn name(&self) -> &str { "my-ocr" }
    /// #     fn version(&self) -> String { "1.0.0".to_string() }
    /// #     fn initialize(&self) -> Result<()> { Ok(()) }
    /// #     fn shutdown(&self) -> Result<()> { Ok(()) }
    /// # }
    /// # use xberg::plugins::OcrBackendType;
    /// # #[async_trait]
    /// # impl OcrBackend for MyOcr {
    /// #     fn supports_language(&self, _: &str) -> bool { true }
    /// #     fn backend_type(&self) -> OcrBackendType { OcrBackendType::Custom }
    /// #     async fn process_image_file(&self, _: &Path, _: &OcrConfig) -> Result<ExtractedDocument> {
    /// #         Ok(ExtractedDocument::default())
    /// #     }
    /// async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
    ///     // Read backend-specific options; unknown keys are silently ignored.
    ///     let fast_mode = config.backend_options
    ///         .as_ref()
    ///         .and_then(|v| v.get("mode"))
    ///         .and_then(|v| v.as_str())
    ///         .map(|s| s == "fast")
    ///         .unwrap_or(false);
    ///
    ///     if image_bytes.is_empty() {
    ///         return Err(xberg::XbergError::Validation {
    ///             message: "Empty image data".to_string(),
    ///             source: None,
    ///         });
    ///     }
    ///
    ///     let text = if fast_mode {
    ///         "Fast OCR result".to_string()
    ///     } else {
    ///         format!("Extracted text in language: {:?}", config.language)
    ///     };
    ///
    ///     let mut document = ExtractedDocument::default();
    ///     document.content = text;
    ///     document.mime_type = Cow::Borrowed("text/plain");
    ///     Ok(document)
    /// }
    /// # }
    /// ```
    async fn process_image(&self, image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument>;

    /// Process an owned image buffer and extract text via OCR.
    ///
    /// The default implementation delegates to [`Self::process_image`]. Backends
    /// that hand work to an owned blocking task can override this method to avoid
    /// copying the image buffer.
    ///
    /// Excluded from the polyglot binding surface: it is an owned-buffer perf
    /// override whose `Arc<Vec<u8>>` parameter has no binding representation, and
    /// foreign backends satisfy the trait through [`Self::process_image`] via this
    /// default delegation.
    #[cfg_attr(alef, alef(skip))]
    async fn process_image_owned(&self, image_bytes: Arc<Vec<u8>>, config: &OcrConfig) -> Result<ExtractedDocument> {
        self.process_image(image_bytes.as_slice(), config).await
    }

    /// Process a file and extract text via OCR.
    ///
    /// Default implementation reads the file and calls `process_image`.
    /// Override for custom file handling or optimizations.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the image file
    /// * `config` - OCR configuration
    ///
    /// # Errors
    ///
    /// Same as `process_image`, plus file I/O errors.
    async fn process_image_file(&self, path: &Path, config: &OcrConfig) -> Result<ExtractedDocument> {
        #[cfg(feature = "tokio-runtime")]
        {
            use crate::core::io;
            let bytes = io::read_file_async(path).await?;
            self.process_image(&bytes, config).await
        }
        #[cfg(not(feature = "tokio-runtime"))]
        {
            let _ = (path, config);
            Err(XbergError::Other(
                "File-based OCR processing requires the tokio-runtime feature".to_string(),
            ))
        }
    }

    /// Check if this backend supports a given language code.
    ///
    /// # Arguments
    ///
    /// * `lang` - ISO 639-2/3 language code (e.g., "eng", "deu", "fra")
    ///
    /// # Returns
    ///
    /// `true` if the language is supported, `false` otherwise.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use xberg::plugins::{Plugin, OcrBackend};
    /// # use xberg::Result;
    /// # use async_trait::async_trait;
    /// # use std::path::Path;
    /// # struct MyOcr { languages: Vec<String> }
    /// # impl Plugin for MyOcr {
    /// #     fn name(&self) -> &str { "my-ocr" }
    /// #     fn version(&self) -> String { "1.0.0".to_string() }
    /// #     fn initialize(&self) -> Result<()> { Ok(()) }
    /// #     fn shutdown(&self) -> Result<()> { Ok(()) }
    /// # }
    /// # use xberg::plugins::OcrBackendType;
    /// # use xberg::{ExtractedDocument, OcrConfig};
    /// # #[async_trait]
    /// # impl OcrBackend for MyOcr {
    /// #     fn backend_type(&self) -> OcrBackendType { OcrBackendType::Custom }
    /// #     async fn process_image(&self, _: &[u8], _: &OcrConfig) -> Result<ExtractedDocument> {
    /// #         Ok(ExtractedDocument::default())
    /// #     }
    /// #     async fn process_image_file(&self, _: &Path, _: &OcrConfig) -> Result<ExtractedDocument> {
    /// #         Ok(ExtractedDocument::default())
    /// #     }
    /// fn supports_language(&self, lang: &str) -> bool {
    ///     self.languages.contains(&lang.to_string())
    /// }
    /// # }
    /// ```
    fn supports_language(&self, lang: &str) -> bool;

    /// Get the backend type identifier.
    ///
    /// # Returns
    ///
    /// The backend type enum value.
    ///
    /// # Example
    ///
    /// ```rust
    /// # use xberg::plugins::{Plugin, OcrBackend, OcrBackendType};
    /// # use xberg::Result;
    /// # use async_trait::async_trait;
    /// # use std::path::Path;
    /// # struct TesseractBackend;
    /// # impl Plugin for TesseractBackend {
    /// #     fn name(&self) -> &str { "tesseract" }
    /// #     fn version(&self) -> String { "1.0.0".to_string() }
    /// #     fn initialize(&self) -> Result<()> { Ok(()) }
    /// #     fn shutdown(&self) -> Result<()> { Ok(()) }
    /// # }
    /// # use xberg::{ExtractedDocument, OcrConfig};
    /// # #[async_trait]
    /// # impl OcrBackend for TesseractBackend {
    /// #     fn supports_language(&self, _: &str) -> bool { true }
    /// #     async fn process_image(&self, _: &[u8], _: &OcrConfig) -> Result<ExtractedDocument> {
    /// #         Ok(ExtractedDocument::default())
    /// #     }
    /// #     async fn process_image_file(&self, _: &Path, _: &OcrConfig) -> Result<ExtractedDocument> {
    /// #         Ok(ExtractedDocument::default())
    /// #     }
    /// fn backend_type(&self) -> OcrBackendType {
    ///     OcrBackendType::Tesseract
    /// }
    /// # }
    /// ```
    fn backend_type(&self) -> OcrBackendType;

    /// Optional: Get a list of all supported languages.
    ///
    /// Defaults to empty list. Override to provide comprehensive language support info.
    fn supported_languages(&self) -> Vec<String> {
        vec![]
    }

    /// Optional: Check if the backend supports table detection.
    ///
    /// Defaults to `false`. Override if your backend can detect and extract tables.
    fn supports_table_detection(&self) -> bool {
        false
    }

    /// Check if the backend supports direct document-level processing (e.g. for PDFs).
    ///
    /// Defaults to `false`. Override if the backend has optimized document processing.
    fn supports_document_processing(&self) -> bool {
        false
    }

    /// Declare that this backend emits structured markdown directly (tables, headings, lists)
    /// and downstream layout reconstruction should be skipped.
    ///
    /// Defaults to `false` — classical OCR backends (Tesseract, PaddleOCR classical) return
    /// plain text per detected region. End-to-end VLM backends (PaddleOCR-VL, GOT-OCR 2.0)
    /// emit markdown in one forward pass and should override this to `true`.
    fn emits_structured_markdown(&self) -> bool {
        false
    }

    /// Process a document file directly via OCR.
    ///
    /// Only called if `supports_document_processing` returns `true`.
    ///
    /// # Arguments
    ///
    /// * `path` - Path to the document file (e.g. .pdf)
    /// * `config` - OCR configuration
    async fn process_document(&self, _path: &Path, _config: &OcrConfig) -> Result<ExtractedDocument> {
        Err(crate::XbergError::Other(
            "Document-level OCR processing not supported by this backend".to_string(),
        ))
    }

    /// Optional: Probe whether this backend will actually execute on this host
    /// with the given configuration.
    ///
    /// Used by `doctor` diagnostics. Implementations must not download models
    /// or make billable API calls; anything not yet local is reported as
    /// `ProbeStatus::Skip`. Defaults to `Skip` so custom backends need no
    /// changes; implement it to give users real environment diagnostics.
    ///
    /// Excluded from the polyglot binding surface: doctor results are produced
    /// by the generated `doctor()` function, not per-backend calls.
    #[cfg_attr(alef, alef(skip))]
    fn probe(&self, _config: &OcrConfig) -> crate::doctor::DoctorCheck {
        crate::doctor::DoctorCheck::skip(self.name(), "no probe implemented for this backend")
    }
}

/// Register an OCR backend with the global registry.
///
/// The OCR backend will be registered with its name from the `name()` method
/// and can be used for OCR processing via the extraction pipeline.
///
/// # Arguments
///
/// * `backend` - The OCR backend implementation wrapped in Arc
///
/// # Returns
///
/// - `Ok(())` if registration succeeded
/// - `Err(...)` if validation failed or initialization failed
///
/// # Errors
///
/// - `XbergError::Validation` - Invalid backend name (empty or contains whitespace)
/// - Any error from the backend's `initialize()` method
///
/// # Example
///
/// ```rust
/// use xberg::plugins::{Plugin, OcrBackend, register_ocr_backend, OcrBackendType};
/// use xberg::{Result, OcrConfig};
/// use xberg::types::{ExtractedDocument, Metadata};
/// use async_trait::async_trait;
/// use std::borrow::Cow;
/// use std::sync::Arc;
/// use std::path::Path;
///
/// struct CustomOcr;
///
/// impl Plugin for CustomOcr {
///     fn name(&self) -> &str { "custom-ocr" }
///     fn version(&self) -> String { "1.0.0".to_string() }
///     fn initialize(&self) -> Result<()> { Ok(()) }
///     fn shutdown(&self) -> Result<()> { Ok(()) }
/// }
///
/// #[async_trait]
/// impl OcrBackend for CustomOcr {
///     async fn process_image(&self, _: &[u8], _: &OcrConfig) -> Result<ExtractedDocument> {
///         let mut document = ExtractedDocument::default();
///         document.content = "text".to_string();
///         document.mime_type = Cow::Borrowed("text/plain");
///         Ok(document)
///     }
///     fn supports_language(&self, _: &str) -> bool { true }
///     fn backend_type(&self) -> OcrBackendType { OcrBackendType::Custom }
/// }
///
/// # tokio_test::block_on(async {
/// let backend = Arc::new(CustomOcr);
/// register_ocr_backend(backend)?;
/// # Ok::<(), xberg::XbergError>(())
/// # });
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn register_ocr_backend(backend: Arc<dyn OcrBackend>) -> crate::Result<()> {
    use crate::plugins::registry::get_ocr_backend_registry;

    let registry = get_ocr_backend_registry();
    let mut registry = registry.write();

    registry.register(backend)
}

/// Unregister an OCR backend by name.
///
/// Removes the OCR backend from the global registry and calls its `shutdown()` method.
///
/// # Arguments
///
/// * `name` - Name of the OCR backend to unregister
///
/// # Returns
///
/// - `Ok(())` if the backend was unregistered or didn't exist
/// - `Err(...)` if the shutdown method failed
///
/// # Example
///
/// ```rust
/// use xberg::plugins::unregister_ocr_backend;
///
/// # tokio_test::block_on(async {
/// unregister_ocr_backend("custom-ocr")?;
/// # Ok::<(), xberg::XbergError>(())
/// # });
/// ```
#[cfg_attr(alef, alef(skip))]
pub fn unregister_ocr_backend(name: &str) -> crate::Result<()> {
    use crate::plugins::registry::get_ocr_backend_registry;

    let registry = get_ocr_backend_registry();
    let mut registry = registry.write();

    registry.remove(name)
}

/// List all registered OCR backends.
///
/// Returns the names of all OCR backends currently registered in the global registry.
///
/// # Returns
///
/// A vector of OCR backend names.
///
/// # Example
///
/// ```rust
/// use xberg::plugins::list_ocr_backends;
///
/// # tokio_test::block_on(async {
/// let backends = list_ocr_backends()?;
/// for name in backends {
///     println!("Registered OCR backend: {}", name);
/// }
/// # Ok::<(), xberg::XbergError>(())
/// # });
/// ```
pub fn list_ocr_backends() -> crate::Result<Vec<String>> {
    use crate::plugins::registry::get_ocr_backend_registry;

    let registry = get_ocr_backend_registry();
    let registry = registry.read();

    Ok(registry.list())
}

/// Clear all OCR backends from the global registry.
///
/// Removes all OCR backends and calls their `shutdown()` methods.
///
/// # Returns
///
/// - `Ok(())` if all backends were cleared successfully
/// - `Err(...)` if any shutdown method failed
///
/// # Example
///
/// ```rust
/// use xberg::plugins::clear_ocr_backends;
///
/// # tokio_test::block_on(async {
/// clear_ocr_backends()?;
/// # Ok::<(), xberg::XbergError>(())
/// # });
/// ```
pub fn clear_ocr_backends() -> crate::Result<()> {
    use crate::plugins::registry::get_ocr_backend_registry;

    let registry = get_ocr_backend_registry();
    let mut registry = registry.write();

    registry.shutdown_all()
}

/// Ensure the global OCR backend registry has its built-in backends registered.
///
/// The global registry is seeded with the built-in backends (Tesseract,
/// PaddleOCR, VLM — gated by feature flags) when it is first constructed.
/// However, [`clear_ocr_backends`] empties the registry, leaving subsequent
/// OCR operations with no backend to dispatch to.
///
/// This function is the self-healing counterpart, mirroring
/// `crate::extractors::ensure_initialized` for the document extractor registry:
/// it re-registers the built-in backends whenever the built-in default is
/// missing so that callers always see a usable registry. It re-seeds not only
/// when the registry is empty but also when it is non-empty yet missing the
/// built-in default (e.g. after [`clear_ocr_backends`] followed by registering
/// a *different* backend) — the plain "empty" check would leave default-config
/// OCR without a backend. Re-seeding is non-destructive (user-registered
/// backends are kept) and cheap to invoke before every OCR dispatch.
#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
pub(crate) fn ensure_ocr_backends_initialized() {
    use crate::plugins::registry::get_ocr_backend_registry;

    let registry = get_ocr_backend_registry();

    {
        let registry = registry.read();
        if !registry.is_missing_default_backend() {
            return;
        }
    }

    registry.write().ensure_defaults();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::borrow::Cow;

    struct MockOcrBackend {
        languages: Vec<String>,
    }

    impl Plugin for MockOcrBackend {
        fn name(&self) -> &str {
            "mock-ocr"
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl OcrBackend for MockOcrBackend {
        async fn process_image(&self, _image_bytes: &[u8], _config: &OcrConfig) -> Result<ExtractedDocument> {
            Ok(ExtractedDocument {
                content: "Mocked OCR text".to_string(),
                mime_type: Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }

        fn supports_language(&self, lang: &str) -> bool {
            self.languages.iter().any(|l| l == lang)
        }

        fn backend_type(&self) -> OcrBackendType {
            OcrBackendType::Custom
        }

        fn supported_languages(&self) -> Vec<String> {
            self.languages.clone()
        }
    }

    #[tokio::test]
    async fn test_ocr_backend_process_image() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string(), "deu".to_string()],
        };

        let config = OcrConfig {
            backend: "mock".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        };

        let result = backend.process_image(b"fake image data", &config).await.unwrap();
        assert_eq!(result.content, "Mocked OCR text");
        assert_eq!(result.mime_type, "text/plain");
    }

    #[tokio::test]
    async fn test_ocr_backend_process_image_owned_default_impl_is_object_safe() {
        let backend: Arc<dyn OcrBackend> = Arc::new(MockOcrBackend {
            languages: vec!["eng".to_string()],
        });

        let result = backend
            .process_image_owned(Arc::new(b"fake image data".to_vec()), &OcrConfig::default())
            .await
            .unwrap();

        assert_eq!(result.content, "Mocked OCR text");
        assert_eq!(result.mime_type, "text/plain");
    }

    #[test]
    fn test_ocr_backend_supports_language() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string(), "deu".to_string()],
        };

        assert!(backend.supports_language("eng"));
        assert!(backend.supports_language("deu"));
        assert!(!backend.supports_language("fra"));
    }

    #[test]
    fn test_ocr_backend_type() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string()],
        };

        assert_eq!(backend.backend_type(), OcrBackendType::Custom);
    }

    #[test]
    fn test_ocr_backend_supported_languages() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string(), "deu".to_string(), "fra".to_string()],
        };

        let supported = backend.supported_languages();
        assert_eq!(supported.len(), 3);
        assert!(supported.contains(&"eng".to_string()));
        assert!(supported.contains(&"deu".to_string()));
        assert!(supported.contains(&"fra".to_string()));
    }

    #[test]
    fn test_ocr_backend_type_variants() {
        assert_eq!(OcrBackendType::Tesseract, OcrBackendType::Tesseract);
        assert_ne!(OcrBackendType::Tesseract, OcrBackendType::PaddleOCR);
        assert_ne!(OcrBackendType::PaddleOCR, OcrBackendType::Custom);
    }

    #[test]
    fn test_ocr_backend_type_debug() {
        let backend_type = OcrBackendType::Tesseract;
        let debug_str = format!("{:?}", backend_type);
        assert!(debug_str.contains("Tesseract"));
    }

    #[test]
    fn test_ocr_backend_type_clone() {
        let backend_type = OcrBackendType::PaddleOCR;
        let cloned = backend_type;
        assert_eq!(backend_type, cloned);
    }

    #[test]
    fn test_ocr_backend_default_table_detection() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string()],
        };
        assert!(!backend.supports_table_detection());
    }

    #[tokio::test]
    async fn test_ocr_backend_process_image_file_default_impl() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let backend = MockOcrBackend {
            languages: vec!["eng".to_string()],
        };

        let mut temp_file = NamedTempFile::new().unwrap();
        temp_file.write_all(b"fake image data").unwrap();
        let path = temp_file.path();

        let config = OcrConfig {
            backend: "mock".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        };

        let result = backend.process_image_file(path, &config).await.unwrap();
        assert_eq!(result.content, "Mocked OCR text");
    }

    #[test]
    fn test_ocr_backend_plugin_interface() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string()],
        };

        assert_eq!(backend.name(), "mock-ocr");
        assert_eq!(backend.version(), "1.0.0");
        assert!(backend.initialize().is_ok());
        assert!(backend.shutdown().is_ok());
    }

    #[test]
    fn test_ocr_backend_empty_languages() {
        let backend = MockOcrBackend { languages: vec![] };

        let supported = backend.supported_languages();
        assert_eq!(supported.len(), 0);
        assert!(!backend.supports_language("eng"));
    }

    #[tokio::test]
    async fn test_ocr_backend_with_empty_image() {
        let backend = MockOcrBackend {
            languages: vec!["eng".to_string()],
        };

        let config = OcrConfig {
            backend: "mock".to_string(),
            language: vec!["eng".to_string()],
            ..Default::default()
        };

        let result = backend.process_image(b"", &config).await;
        assert!(result.is_ok());
    }

    struct OptionAwareBackend;

    impl Plugin for OptionAwareBackend {
        fn name(&self) -> &str {
            "option-aware"
        }

        fn version(&self) -> String {
            "1.0.0".to_string()
        }

        fn initialize(&self) -> Result<()> {
            Ok(())
        }

        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl OcrBackend for OptionAwareBackend {
        async fn process_image(&self, _image_bytes: &[u8], config: &OcrConfig) -> Result<ExtractedDocument> {
            let mode = config
                .backend_options
                .as_ref()
                .and_then(|v| v.get("mode"))
                .and_then(|v| v.as_str())
                .unwrap_or("standard");

            Ok(ExtractedDocument {
                content: format!("mode={mode}"),
                mime_type: Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }

        fn supports_language(&self, _: &str) -> bool {
            true
        }

        fn backend_type(&self) -> OcrBackendType {
            OcrBackendType::Custom
        }
    }

    #[tokio::test]
    async fn test_backend_reads_backend_options() {
        let backend = OptionAwareBackend;

        let config_with_options = OcrConfig {
            backend_options: Some(serde_json::json!({"mode": "fast", "threshold": 0.8})),
            ..Default::default()
        };
        let result = backend.process_image(b"img", &config_with_options).await.unwrap();
        assert_eq!(result.content, "mode=fast");

        let config_without_options = OcrConfig::default();
        let result = backend.process_image(b"img", &config_without_options).await.unwrap();
        assert_eq!(result.content, "mode=standard");
    }

    #[tokio::test]
    async fn test_backend_options_unknown_keys_silently_ignored() {
        let backend = OptionAwareBackend;

        let config = OcrConfig {
            backend_options: Some(serde_json::json!({
                "unknown_key": "value",
                "another_unknown": 42
            })),
            ..Default::default()
        };
        let result = backend.process_image(b"img", &config).await;
        assert!(result.is_ok(), "unknown backend_options keys must not cause errors");
    }
}
