//! OCR backend registry.

use crate::Result;
#[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
use crate::XbergError;
use crate::plugins::OcrBackend;
use ahash::AHashMap;
use std::sync::Arc;

/// Registry for OCR backend plugins.
///
/// Manages OCR backends with backend type and language-based selection.
///
/// # Thread Safety
///
/// The registry is thread-safe and can be accessed concurrently from multiple threads.
///
/// # Example
///
/// ```rust,no_run
/// use xberg::plugins::registry::OcrBackendRegistry;
/// use std::sync::Arc;
///
/// let registry = OcrBackendRegistry::new();
/// // Register OCR backends
/// // registry.register(Arc::new(TesseractBackend::new()));
/// ```
#[cfg_attr(alef, alef(skip))]
pub struct OcrBackendRegistry {
    pub(super) backends: AHashMap<String, Arc<dyn OcrBackend>>,
}

#[cfg(all(test, sceptre_ocr, not(target_arch = "wasm32")))]
mod sceptre_tests {
    use std::sync::Arc;

    use super::OcrBackendRegistry;

    #[test]
    fn registry_registers_sceptre_without_loading_models() {
        let registry = OcrBackendRegistry::new();
        let backend = registry.get("sceptre").expect("Sceptre backend should be registered");
        let uppercase = registry
            .get("SCEPTRE")
            .expect("validated built-in backend names should be case-insensitive");

        assert_eq!(backend.backend_type(), crate::plugins::OcrBackendType::Custom);
        assert!(Arc::ptr_eq(&backend, &uppercase));
    }
}

impl OcrBackendRegistry {
    /// Create a new OCR backend registry with default backends.
    ///
    /// Registers the Tesseract backend by default if the "ocr" feature is enabled,
    /// PaddleOCR if the "paddle-ocr" feature is enabled, and Sceptre if the
    /// "sceptre-ocr" feature is enabled.
    ///
    /// If a backend fails to initialize or register it is skipped with a warning,
    /// allowing the process to continue with whichever backends are available.
    #[tracing::instrument(name = "ocr_backend_registry_init")]
    pub fn new() -> Self {
        let mut registry = Self {
            backends: AHashMap::new(),
        };
        registry.register_defaults();
        registry
    }

    /// Register the built-in OCR backends into this registry.
    ///
    /// Registers whichever backends the active feature set enables — Tesseract
    /// (`ocr`/`ocr-wasm`), PaddleOCR (`paddle-ocr`), Sceptre (`sceptre-ocr`),
    /// and the VLM backend (`liter-llm`). Each backend is registered independently:
    /// if one fails to initialize it is skipped with a warning so the remaining
    /// backends still register.
    ///
    /// This is invoked by [`OcrBackendRegistry::new`] at construction and reused
    /// by the self-healing initialization path so a registry emptied via
    /// [`clear`](Self::clear) can be re-seeded with the defaults.
    pub fn register_defaults(&mut self) {
        #[cfg(feature = "ocr")]
        {
            use crate::ocr::tesseract_backend::TesseractBackend;
            tracing::info!("Initializing Tesseract OCR backend");
            let backend = TesseractBackend::new();
            self.register(Arc::new(backend)).unwrap_or_else(|e| {
                tracing::warn!("Failed to register Tesseract backend: {e}");
            });
            tracing::info!("Tesseract OCR backend registered successfully");
        }

        #[cfg(all(feature = "ocr-wasm", not(feature = "ocr")))]
        {
            use crate::ocr::tesseract_wasm_backend::TesseractWasmBackend;
            tracing::info!("Initializing Tesseract WASM OCR backend");
            match TesseractWasmBackend::new() {
                Ok(backend) => {
                    self.register(Arc::new(backend)).unwrap_or_else(|e| {
                        tracing::warn!("Failed to register Tesseract WASM backend: {e}");
                    });
                    tracing::info!("Tesseract WASM OCR backend registered successfully");
                }
                Err(e) => {
                    tracing::warn!("Tesseract WASM OCR backend unavailable: {e}");
                }
            }
        }

        #[cfg(feature = "paddle-ocr")]
        {
            use crate::paddle_ocr::PaddleOcrBackend;
            tracing::info!("Initializing PaddleOCR backend");
            match PaddleOcrBackend::new() {
                Ok(backend) => {
                    self.register(Arc::new(backend)).unwrap_or_else(|e| {
                        tracing::warn!("Failed to register PaddleOCR backend: {e}");
                    });
                    tracing::info!("PaddleOCR backend registered successfully");
                }
                Err(e) => {
                    tracing::warn!(
                        "PaddleOCR backend unavailable: {e}. \
                         Check ONNX Runtime availability and model files."
                    );
                }
            }
        }

        #[cfg(all(sceptre_ocr, not(target_arch = "wasm32")))]
        {
            use crate::sceptre_ocr::SceptreOcrBackend;
            tracing::info!("Registering Sceptre OCR backend");
            match SceptreOcrBackend::new() {
                Ok(backend) => {
                    self.register(Arc::new(backend)).unwrap_or_else(|error| {
                        tracing::warn!("Failed to register Sceptre OCR backend: {error}");
                    });
                }
                Err(error) => tracing::warn!("Failed to initialize Sceptre OCR backend: {error}"),
            }
        }

        #[cfg(all(feature = "liter-llm", not(target_arch = "wasm32")))]
        {
            use crate::llm::vlm_ocr::VlmOcrBackend;
            tracing::info!("Registering VLM OCR backend");
            self.register(Arc::new(VlmOcrBackend)).unwrap_or_else(|e| {
                tracing::warn!("Failed to register VLM OCR backend: {e}");
            });
        }

        #[cfg(feature = "candle-trocr")]
        {
            use crate::candle_ocr::TrocrBackend;
            use xberg_candle_ocr::models::TrocrVariant;
            tracing::info!("Initializing TrOCR backend");
            let backend = TrocrBackend::new(TrocrVariant::default());
            self.register(Arc::new(backend)).unwrap_or_else(|e| {
                tracing::warn!("Failed to register TrOCR backend: {e}");
            });
            tracing::info!("TrOCR backend registered successfully");
        }

        #[cfg(feature = "candle-paddleocr-vl")]
        {
            use crate::candle_ocr::PaddleOcrVlBackend;
            use xberg_candle_ocr::models::PaddleOcrVlTask;
            tracing::info!("Initializing PaddleOCR-VL backend");
            let backend = PaddleOcrVlBackend::new(PaddleOcrVlTask::default());
            self.register(Arc::new(backend)).unwrap_or_else(|e| {
                tracing::warn!("Failed to register PaddleOCR-VL backend: {e}");
            });
            tracing::info!("PaddleOCR-VL backend registered successfully");
        }

        #[cfg(feature = "candle-glm-ocr")]
        {
            use crate::candle_ocr::GlmOcrBackend;
            use crate::candle_ocr::glm_ocr_backend::LayoutMode;
            use xberg_candle_ocr::models::glm_ocr::GlmOcrTask;
            tracing::info!("Initializing GLM-OCR backend");
            let backend = GlmOcrBackend::new(GlmOcrTask::default(), LayoutMode::default());
            self.register(Arc::new(backend)).unwrap_or_else(|e| {
                tracing::warn!("Failed to register GLM-OCR backend: {e}");
            });
            tracing::info!("GLM-OCR backend registered successfully");
        }

        #[cfg(all(feature = "candle-deepseek-ocr", not(target_arch = "wasm32")))]
        {
            use crate::candle_ocr::DeepseekOcrBackend;
            tracing::info!("Initializing DeepSeek-OCR backend");
            let backend = DeepseekOcrBackend::new();
            self.register(Arc::new(backend)).unwrap_or_else(|e| {
                tracing::warn!("Failed to register DeepSeek-OCR backend: {e}");
            });
            tracing::info!("DeepSeek-OCR backend registered successfully");
        }
    }

    /// Create a new empty OCR backend registry without default backends.
    ///
    /// This is useful for testing or when you want full control over backend registration.
    pub fn new_empty() -> Self {
        Self {
            backends: AHashMap::new(),
        }
    }

    /// Register an OCR backend.
    ///
    /// # Arguments
    ///
    /// * `backend` - The OCR backend to register
    ///
    /// # Returns
    ///
    /// - `Ok(())` if registration succeeded
    /// - `Err(...)` if initialization failed
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// # use xberg::plugins::registry::OcrBackendRegistry;
    /// # use std::sync::Arc;
    /// let mut registry = OcrBackendRegistry::new();
    /// // let backend = Arc::new(MyOcrBackend::new());
    /// // registry.register(backend)?;
    /// # Ok::<(), xberg::XbergError>(())
    /// ```
    #[tracing::instrument(skip(self, backend), fields(backend_name))]
    pub fn register(&mut self, backend: Arc<dyn OcrBackend>) -> Result<()> {
        let name = backend.name().to_string();
        tracing::Span::current().record("backend_name", name.as_str());

        super::validate_plugin_name(&name)?;

        backend.initialize()?;

        tracing::info!(backend = %name, "OCR backend registered");
        self.backends.insert(name, backend);
        Ok(())
    }

    /// Get an OCR backend by name.
    ///
    /// # Arguments
    ///
    /// * `name` - Backend name
    ///
    /// # Returns
    ///
    /// The backend if found, or an error if not registered.
    #[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
    #[tracing::instrument(skip(self), fields(registered_backends = ?self.backends.keys().collect::<Vec<_>>()))]
    pub(crate) fn get(&self, name: &str) -> Result<Arc<dyn OcrBackend>> {
        if let Some(backend) = self.backends.get(name) {
            return Ok(Arc::clone(backend));
        }

        let normalized = name.to_ascii_lowercase();
        let canonical = match normalized.as_str() {
            "paddleocr" => "paddle-ocr",
            _ => normalized.as_str(),
        };
        self.backends.get(canonical).cloned().ok_or_else(|| {
            tracing::error!(
                backend = name,
                available = ?self.backends.keys().collect::<Vec<_>>(),
                "OCR backend not found in registry"
            );
            XbergError::Plugin {
                message: format!(
                    "OCR backend '{}' not registered. Available backends: {:?}",
                    name,
                    self.backends.keys().collect::<Vec<_>>()
                ),
                plugin_name: name.to_string(),
            }
        })
    }

    /// Get an OCR backend that supports a specific language.
    ///
    /// Returns the first backend that supports the language.
    ///
    /// # Arguments
    ///
    /// * `language` - Language code (e.g., "eng", "deu")
    ///
    /// # Returns
    ///
    /// The first backend supporting the language, or an error if none found.
    #[cfg(all(test, any(feature = "ocr", feature = "ocr-wasm")))]
    pub(crate) fn get_for_language(&self, language: &str) -> Result<Arc<dyn OcrBackend>> {
        self.backends
            .values()
            .find(|backend| backend.supports_language(language))
            .cloned()
            .ok_or_else(|| XbergError::Plugin {
                message: format!("No OCR backend supports language '{}'", language),
                plugin_name: language.to_string(),
            })
    }

    /// List all registered backend names.
    pub fn list(&self) -> Vec<String> {
        self.backends.keys().cloned().collect()
    }

    /// Remove a backend from the registry.
    ///
    /// Calls `shutdown()` on the backend before removing.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        if let Some(backend) = self.backends.remove(name) {
            backend.shutdown()?;
        }
        Ok(())
    }

    /// Shutdown all backends and clear the registry.
    pub fn shutdown_all(&mut self) -> Result<()> {
        let names: Vec<_> = self.backends.keys().cloned().collect();
        for name in names {
            self.remove(&name)?;
        }
        Ok(())
    }

    /// Drain the registry. Alias for `shutdown_all` used by alef trait-bridge codegen.
    pub fn clear(&mut self) -> Result<()> {
        self.shutdown_all()
    }

    /// True when the registry lacks a usable built-in default and should be
    /// re-seeded.
    ///
    /// This is the case when the registry is completely empty, or — when a
    /// Tesseract built-in is compiled in — when the built-in default backend
    /// (`tesseract`, the fallback dispatch target) is absent. The latter can
    /// happen after [`clear`](Self::clear) followed by registering a *different*
    /// backend: the registry is non-empty, yet default-config OCR has no usable
    /// backend.
    #[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
    pub(crate) fn is_missing_default_backend(&self) -> bool {
        #[cfg(any(feature = "ocr", feature = "ocr-wasm"))]
        const DEFAULT: Option<&str> = Some("tesseract");
        #[cfg(not(any(feature = "ocr", feature = "ocr-wasm")))]
        const DEFAULT: Option<&str> = None;

        self.backends.is_empty() || DEFAULT.is_some_and(|name| !self.backends.contains_key(name))
    }

    /// Non-destructively re-seed the built-in default backends when they are
    /// missing.
    ///
    /// Keeps any user-registered backends; a no-op when the built-in default is
    /// already present. Used by the self-healing dispatch path so a registry
    /// left without a default (via clear + register-other) heals itself.
    #[cfg(any(feature = "ocr", feature = "ocr-wasm", feature = "ocr-pipeline"))]
    pub(crate) fn ensure_defaults(&mut self) {
        if self.is_missing_default_backend() {
            self.register_defaults();
        }
    }
}

impl Default for OcrBackendRegistry {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(all(test, any(feature = "ocr", feature = "ocr-wasm")))]
mod tests {
    use super::*;
    use crate::core::config::OcrConfig;
    use crate::plugins::{OcrBackend, Plugin};
    use crate::types::ExtractedDocument;
    use async_trait::async_trait;
    use std::borrow::Cow;

    struct MockOcrBackend {
        name: String,
        languages: Vec<String>,
    }

    impl Plugin for MockOcrBackend {
        fn name(&self) -> &str {
            &self.name
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
        async fn process_image(&self, _: &[u8], _: &OcrConfig) -> Result<ExtractedDocument> {
            Ok(ExtractedDocument {
                content: "test".to_string(),
                mime_type: Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }

        fn supports_language(&self, lang: &str) -> bool {
            self.languages.iter().any(|l| l == lang)
        }

        fn backend_type(&self) -> crate::plugins::ocr::OcrBackendType {
            crate::plugins::ocr::OcrBackendType::Custom
        }
    }

    #[test]
    fn test_ocr_backend_registry() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(MockOcrBackend {
            name: "test-ocr".to_string(),
            languages: vec!["eng".to_string(), "deu".to_string()],
        });

        registry.register(backend).unwrap();

        let retrieved = registry.get("test-ocr").unwrap();
        assert_eq!(retrieved.name(), "test-ocr");

        let eng_backend = registry.get_for_language("eng").unwrap();
        assert_eq!(eng_backend.name(), "test-ocr");

        let names = registry.list();
        assert_eq!(names.len(), 1);
        assert!(names.contains(&"test-ocr".to_string()));
    }

    #[test]
    fn test_ocr_backend_registry_new_empty() {
        let registry = OcrBackendRegistry::new_empty();
        assert_eq!(registry.list().len(), 0);
    }

    #[cfg(feature = "ocr")]
    #[test]
    fn ensure_defaults_reseeds_when_default_missing_but_registry_nonempty() {
        let mut registry = OcrBackendRegistry::new_empty();
        registry
            .register(Arc::new(MockOcrBackend {
                name: "custom-ocr".to_string(),
                languages: vec!["eng".to_string()],
            }))
            .unwrap();
        assert!(
            !registry.list().iter().any(|n| n == "tesseract"),
            "precondition: no built-in default present"
        );
        assert!(
            registry.is_missing_default_backend(),
            "a non-empty registry without the built-in default must report missing"
        );

        registry.ensure_defaults();

        assert!(
            registry.list().iter().any(|n| n == "tesseract"),
            "ensure_defaults should re-seed the built-in default even when the registry is non-empty"
        );
        assert!(
            registry.list().iter().any(|n| n == "custom-ocr"),
            "ensure_defaults must be non-destructive: the user backend is kept"
        );
    }

    #[test]
    fn should_re_register_default_backends_after_clear() {
        let mut registry = OcrBackendRegistry::new();
        let seeded = registry.list();
        assert!(
            !seeded.is_empty(),
            "expected built-in OCR backends to be seeded by `new` with the `ocr` feature enabled"
        );

        registry.clear().unwrap();
        assert_eq!(registry.list().len(), 0, "clear should empty the registry");

        registry.register_defaults();
        let mut restored = registry.list();
        let mut expected = seeded;
        restored.sort();
        expected.sort();
        assert_eq!(
            restored, expected,
            "register_defaults should restore the same built-in backends"
        );
    }

    #[test]
    fn test_registry_construction_does_not_eagerly_allocate_tesseract() {
        use crate::ocr::tesseract_backend::TesseractBackend;

        let backend = TesseractBackend::new();
        assert!(
            !backend.processor_is_initialized(),
            "TesseractBackend::new() should not eagerly allocate the processor"
        );
    }

    #[test]
    fn test_ocr_backend_get_missing() {
        let registry = OcrBackendRegistry::new_empty();
        let result = registry.get("nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_ocr_backend_get_for_language_missing() {
        let registry = OcrBackendRegistry::new_empty();
        let result = registry.get_for_language("fra");
        assert!(result.is_err());
    }

    #[test]
    fn test_ocr_backend_remove() {
        let mut registry = OcrBackendRegistry::new_empty();
        let backend = Arc::new(MockOcrBackend {
            name: "test-backend".to_string(),
            languages: vec!["eng".to_string()],
        });
        registry.register(backend).unwrap();

        registry.remove("test-backend").unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_ocr_backend_shutdown_all() {
        let mut registry = OcrBackendRegistry::new_empty();
        let backend1 = Arc::new(MockOcrBackend {
            name: "backend1".to_string(),
            languages: vec!["eng".to_string()],
        });
        let backend2 = Arc::new(MockOcrBackend {
            name: "backend2".to_string(),
            languages: vec!["deu".to_string()],
        });

        registry.register(backend1).unwrap();
        registry.register(backend2).unwrap();

        registry.shutdown_all().unwrap();
        assert_eq!(registry.list().len(), 0);
    }

    struct FailingOcrBackend {
        name: String,
        fail_on_init: bool,
    }

    impl Plugin for FailingOcrBackend {
        fn name(&self) -> &str {
            &self.name
        }
        fn version(&self) -> String {
            "1.0.0".to_string()
        }
        fn initialize(&self) -> Result<()> {
            if self.fail_on_init {
                Err(XbergError::Plugin {
                    message: "Backend initialization failed".to_string(),
                    plugin_name: self.name.clone(),
                })
            } else {
                Ok(())
            }
        }
        fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    #[async_trait]
    impl OcrBackend for FailingOcrBackend {
        async fn process_image(&self, _: &[u8], _: &OcrConfig) -> Result<ExtractedDocument> {
            Ok(ExtractedDocument {
                content: "test".to_string(),
                mime_type: Cow::Borrowed("text/plain"),
                ..Default::default()
            })
        }

        fn supports_language(&self, _lang: &str) -> bool {
            false
        }

        fn backend_type(&self) -> crate::plugins::ocr::OcrBackendType {
            crate::plugins::ocr::OcrBackendType::Custom
        }
    }

    #[test]
    fn test_ocr_backend_initialization_failure_logs_error() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(FailingOcrBackend {
            name: "failing-ocr".to_string(),
            fail_on_init: true,
        });

        let result = registry.register(backend);
        assert!(result.is_err());
        assert_eq!(registry.list().len(), 0);
    }

    #[test]
    fn test_ocr_backend_invalid_name_empty_logs_warning() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(MockOcrBackend {
            name: "".to_string(),
            languages: vec!["eng".to_string()],
        });

        let result = registry.register(backend);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_ocr_backend_invalid_name_with_spaces_logs_warning() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(MockOcrBackend {
            name: "invalid ocr backend".to_string(),
            languages: vec!["eng".to_string()],
        });

        let result = registry.register(backend);
        assert!(matches!(result, Err(XbergError::Validation { .. })));
    }

    #[test]
    fn test_ocr_backend_successful_registration_logs_debug() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(MockOcrBackend {
            name: "valid-ocr".to_string(),
            languages: vec!["eng".to_string()],
        });

        let result = registry.register(backend);
        assert!(result.is_ok());
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn test_ocr_backend_multiple_registrations() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend1 = Arc::new(MockOcrBackend {
            name: "ocr-backend-1".to_string(),
            languages: vec!["eng".to_string()],
        });

        let backend2 = Arc::new(MockOcrBackend {
            name: "ocr-backend-2".to_string(),
            languages: vec!["deu".to_string()],
        });

        registry.register(backend1).unwrap();
        registry.register(backend2).unwrap();

        assert_eq!(registry.list().len(), 2);
    }

    #[test]
    fn test_ocr_backend_paddleocr_alias_resolves() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(MockOcrBackend {
            name: "paddle-ocr".to_string(),
            languages: vec!["en".to_string()],
        });

        registry.register(backend).unwrap();

        let retrieved = registry.get("paddleocr").unwrap();
        assert_eq!(retrieved.name(), "paddle-ocr");

        let retrieved = registry.get("paddle-ocr").unwrap();
        assert_eq!(retrieved.name(), "paddle-ocr");
    }

    #[test]
    fn test_ocr_backend_paddleocr_alias_resolves_to_paddle_ocr() {
        let mut registry = OcrBackendRegistry::new_empty();

        let backend = Arc::new(MockOcrBackend {
            name: "paddle-ocr".to_string(),
            languages: vec!["en".to_string()],
        });

        registry.register(backend).unwrap();

        let retrieved = registry.get("paddle-ocr").unwrap();
        assert_eq!(retrieved.name(), "paddle-ocr");

        let aliased = registry.get("paddleocr").unwrap();
        assert_eq!(aliased.name(), "paddle-ocr");
    }
}
