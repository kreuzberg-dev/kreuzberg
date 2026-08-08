//! Startup validation for plugin registries.
//!
//! This module provides diagnostics and health checks for plugins
//! at server startup, helping operators diagnose issues in containerized
//! environments like Kubernetes.

#[cfg(feature = "api")]
use crate::Result;
#[cfg(feature = "api")]
use crate::plugins::registry::{
    get_document_extractor_registry, get_ocr_backend_registry, get_post_processor_registry, get_validator_registry,
};

/// Plugin health status information.
///
/// Contains diagnostic information about registered plugins for each type.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub struct PluginHealthStatus {
    /// Number of registered OCR backends
    pub ocr_backends_count: usize,
    /// Names of registered OCR backends
    pub ocr_backends: Vec<String>,
    /// Number of registered document extractors
    pub extractors_count: usize,
    /// Names of registered document extractors
    pub extractors: Vec<String>,
    /// Number of registered post-processors
    pub post_processors_count: usize,
    /// Names of registered post-processors
    pub post_processors: Vec<String>,
    /// Number of registered validators
    pub validators_count: usize,
    /// Names of registered validators
    pub validators: Vec<String>,
}

impl PluginHealthStatus {
    /// Check plugin health and return status.
    ///
    /// This function reads all plugin registries and collects information
    /// about registered plugins. It logs warnings if critical plugins are missing.
    ///
    /// # Returns
    ///
    /// `PluginHealthStatus` with counts and names of all registered plugins.
    ///
    /// # Example
    ///
    /// `PluginHealthStatus::check` is `pub(crate)`, so this example cannot be compiled from
    /// outside the crate; it is kept as documentation of the intended shape.
    ///
    /// ```ignore
    /// use xberg::plugins::startup_validation::PluginHealthStatus;
    ///
    /// #[tokio::main]
    /// async fn main() {
    ///     let status = PluginHealthStatus::check();
    ///     println!("OCR backends: {:?}", status.ocr_backends);
    /// }
    /// ```
    #[cfg(feature = "api")]
    pub(crate) fn check() -> Self {
        let ocr_registry = get_ocr_backend_registry();
        let ocr_backends = ocr_registry.read().list();

        let extractor_registry = get_document_extractor_registry();
        let extractors = extractor_registry.read().list();

        let processor_registry = get_post_processor_registry();
        let post_processors = processor_registry.read().list();

        let validator_registry = get_validator_registry();
        let validators = validator_registry.read().list();

        let ocr_backends_count = ocr_backends.len();
        let extractors_count = extractors.len();
        let post_processors_count = post_processors.len();
        let validators_count = validators.len();

        PluginHealthStatus {
            ocr_backends_count,
            ocr_backends,
            extractors_count,
            extractors,
            post_processors_count,
            post_processors,
            validators_count,
            validators,
        }
    }
}

/// Validate plugin registries at startup and emit diagnostic logs.
///
/// This function is designed to be called when the API server starts
/// to help diagnose configuration issues early. It checks:
///
/// - Whether OCR backends are registered (warns if none)
/// - Whether document extractors are registered (warns if none)
/// - Environment variables that might affect plugin initialization
/// - File permission issues in containerized environments
///
/// For Kubernetes deployments, this logs information that helps with
/// troubleshooting in the container logs.
///
/// # Returns
///
/// - `Ok(PluginHealthStatus)` with diagnostic information
/// - `Err(XbergError)` if critical issues are detected (currently always succeeds)
///
/// # Example
///
/// `validate_plugins_at_startup` is `pub(crate)`, so this example cannot be compiled from
/// outside the crate; it is kept as documentation of the intended shape.
///
/// ```ignore
/// use xberg::plugins::startup_validation::validate_plugins_at_startup;
///
/// #[tokio::main]
/// async fn main() -> xberg::Result<()> {
///     let status = validate_plugins_at_startup()?;
///     println!("Plugins ready: {} backends registered", status.ocr_backends_count);
///     Ok(())
/// }
/// ```
#[cfg(feature = "api")]
pub(crate) fn validate_plugins_at_startup() -> Result<PluginHealthStatus> {
    let status = PluginHealthStatus::check();

    if status.ocr_backends_count == 0 {
        tracing::warn!(
            "No OCR backends registered. OCR functionality will be unavailable. \
             This is normal if OCR is not required. \
             If OCR is needed, check that: \
             1. The 'ocr' feature is enabled in Cargo.toml \
             2. TESSDATA_PREFIX environment variable is set (e.g., /usr/share/tesseract-ocr/tessdata) \
             3. Tessdata files exist and are readable (tessdata/*.traineddata) \
             4. In containers, mount tessdata volume or install tesseract-ocr package. \
             See https://docs.xberg.io/guides/docker/ for Kubernetes setup."
        );
    } else {
        tracing::info!(
            "OCR backends registered: [{}]. Ready for OCR processing.",
            status.ocr_backends.join(", ")
        );
    }

    if status.extractors_count == 0 {
        tracing::warn!(
            "No document extractors registered. \
             Document extraction will fail. \
             This usually indicates a configuration issue. \
             Ensure extractors are properly registered during initialization."
        );
    } else {
        tracing::info!("Document extractors registered: [{}]", status.extractors.join(", "));
    }

    if status.post_processors_count > 0 {
        tracing::info!("Post-processors registered: [{}]", status.post_processors.join(", "));
    }

    if status.validators_count > 0 {
        tracing::info!("Validators registered: [{}]", status.validators.join(", "));
    }

    check_environment_variables();

    Ok(status)
}

/// Check and log relevant environment variables at startup.
///
/// Logs diagnostics about environment variables that affect plugin behavior,
/// particularly useful for Kubernetes deployments where configuration
/// is often done via environment variables.
#[cfg(feature = "api")]
fn check_environment_variables() {
    match std::env::var("TESSDATA_PREFIX") {
        Ok(path) => {
            tracing::debug!("TESSDATA_PREFIX={}", path);
            if let Ok(metadata) = std::fs::metadata(&path) {
                if metadata.is_dir() {
                    tracing::debug!(
                        "TESSDATA_PREFIX directory exists and is readable. \
                         Tesseract should find trained data files."
                    );
                } else {
                    tracing::warn!(
                        "TESSDATA_PREFIX={} exists but is not a directory. \
                         Tesseract may fail to initialize.",
                        path
                    );
                }
            } else {
                tracing::warn!(
                    "TESSDATA_PREFIX={} does not exist or is not readable. \
                     Tesseract may fail to initialize. \
                     Check directory permissions in containerized environments.",
                    path
                );
            }
        }
        Err(_) => {
            tracing::debug!("TESSDATA_PREFIX not set. Tesseract will use system default paths.");
        }
    }

    if std::path::Path::new("/usr/share/tesseract-ocr/tessdata").exists() {
        tracing::debug!("Found tessdata at system default: /usr/share/tesseract-ocr/tessdata");
    }

    if let Ok(log_level) = std::env::var("RUST_LOG") {
        tracing::debug!("RUST_LOG={}", log_level);
    }
}

#[cfg(all(test, feature = "api"))]
mod tests {
    use super::*;

    #[test]
    fn test_plugin_health_status_check() {
        let status = PluginHealthStatus::check();
        let _ = status.ocr_backends_count;
        let _ = status.extractors_count;
    }

    #[test]
    fn test_validate_plugins_at_startup() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();

        let result = validate_plugins_at_startup();
        assert!(result.is_ok());
        let status = result.unwrap();
        let _ = status.ocr_backends_count;
    }

    #[test]
    fn test_plugin_health_status_ocr_backends_empty() {
        let status = PluginHealthStatus::check();
        assert_eq!(status.ocr_backends.len(), status.ocr_backends_count);
    }

    #[test]
    fn test_plugin_health_status_extractors_empty() {
        let status = PluginHealthStatus::check();
        assert_eq!(status.extractors.len(), status.extractors_count);
    }

    #[test]
    fn test_plugin_health_status_post_processors_empty() {
        let status = PluginHealthStatus::check();
        assert_eq!(status.post_processors.len(), status.post_processors_count);
    }

    #[test]
    fn test_plugin_health_status_validators_empty() {
        let status = PluginHealthStatus::check();
        assert_eq!(status.validators.len(), status.validators_count);
    }

    #[test]
    fn test_validate_plugins_at_startup_returns_status() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();

        let result = validate_plugins_at_startup();
        assert!(result.is_ok());

        let status = result.unwrap();
        assert_eq!(status.ocr_backends.len(), status.ocr_backends_count);
        assert_eq!(status.extractors.len(), status.extractors_count);
        assert_eq!(status.post_processors.len(), status.post_processors_count);
        assert_eq!(status.validators.len(), status.validators_count);
    }

    #[test]
    fn test_plugin_health_status_check_consistency() {
        let status1 = PluginHealthStatus::check();
        let status2 = PluginHealthStatus::check();

        assert_eq!(status1.ocr_backends_count, status2.ocr_backends_count);
        assert_eq!(status1.extractors_count, status2.extractors_count);
        assert_eq!(status1.post_processors_count, status2.post_processors_count);
        assert_eq!(status1.validators_count, status2.validators_count);
    }

    #[test]
    fn test_validate_plugins_at_startup_with_logging() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::INFO)
            .with_test_writer()
            .try_init();

        let result = validate_plugins_at_startup();
        assert!(result.is_ok());

        let status = result.unwrap();
        assert_eq!(status.ocr_backends.len(), status.ocr_backends_count);
        assert_eq!(status.extractors.len(), status.extractors_count);
        assert_eq!(status.post_processors.len(), status.post_processors_count);
        assert_eq!(status.validators.len(), status.validators_count);
    }

    #[test]
    fn test_plugin_health_status_all_counts_valid() {
        let status = PluginHealthStatus::check();

        assert_eq!(status.ocr_backends.len(), status.ocr_backends_count);
        assert_eq!(status.extractors.len(), status.extractors_count);
        assert_eq!(status.post_processors.len(), status.post_processors_count);
        assert_eq!(status.validators.len(), status.validators_count);
    }

    #[test]
    fn test_plugin_health_status_vec_sizes_match_counts() {
        let status = PluginHealthStatus::check();

        assert_eq!(status.ocr_backends.len(), status.ocr_backends_count);
        assert_eq!(status.extractors.len(), status.extractors_count);
        assert_eq!(status.post_processors.len(), status.post_processors_count);
        assert_eq!(status.validators.len(), status.validators_count);
    }

    #[test]
    fn test_validate_plugins_at_startup_logs_warnings_and_info() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();

        let result = validate_plugins_at_startup();
        assert!(result.is_ok());

        let status = result.unwrap();
        assert_eq!(status.ocr_backends.len(), status.ocr_backends_count);
    }

    #[test]
    fn test_check_environment_variables_with_rust_log() {
        let _ = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_test_writer()
            .try_init();

        let result = validate_plugins_at_startup();
        assert!(result.is_ok());
    }

    #[test]
    fn test_plugin_health_status_clone() {
        let status1 = PluginHealthStatus::check();
        let status2 = status1.clone();

        assert_eq!(status1.ocr_backends_count, status2.ocr_backends_count);
        assert_eq!(status1.extractors_count, status2.extractors_count);
        assert_eq!(status1.post_processors_count, status2.post_processors_count);
        assert_eq!(status1.validators_count, status2.validators_count);
    }

    #[test]
    fn test_plugin_health_status_debug_format() {
        let status = PluginHealthStatus::check();
        let debug_str = format!("{:?}", status);
        assert!(!debug_str.is_empty());
        assert!(debug_str.contains("ocr_backends_count"));
    }
}
