//! Builder pattern API for constructing ExtractionConfig programmatically.
//!
//! This module provides a step-by-step builder interface for language bindings
//! that prefer to construct configurations programmatically rather than via JSON.
//!
//! Unlike the JSON-based API in config.rs, this builder allows incremental
//! configuration construction with immediate validation at each step.

use crate::ffi_panic_guard;
use crate::ffi_panic_guard_i32;
use crate::helpers::{clear_last_error, set_last_error};
use std::ffi::{c_char, CStr};
use std::ptr;
use xberg::core::config::LayoutDetectionConfig;
#[cfg(feature = "tree-sitter")]
use xberg::core::config::TreeSitterConfig;
use xberg::core::config::{
    AccelerationConfig, ChunkingConfig, ContentFilterConfig, ExtractionConfig, HtmlOutputConfig, ImageExtractionConfig,
    LanguageDetectionConfig, OcrConfig, PdfConfig, PostProcessorConfig,
};

/// Opaque builder struct for constructing ExtractionConfig.
///
/// Use xberg_config_builder_new() to create, set fields with setters,
/// then finalize with xberg_config_builder_build().
pub struct ConfigBuilder {
    config: ExtractionConfig,
}

impl ConfigBuilder {
    fn new() -> Self {
        Self {
            config: ExtractionConfig::default(),
        }
    }

    fn set_use_cache(&mut self, use_cache: bool) {
        self.config.use_cache = use_cache;
    }

    fn set_include_document_structure(&mut self, include: bool) {
        self.config.include_document_structure = include;
    }

    fn set_cache_namespace(&mut self, namespace: &str) {
        self.config.cache_namespace = if namespace.is_empty() {
            None
        } else {
            Some(namespace.to_string())
        };
    }

    fn set_cache_ttl_secs(&mut self, ttl_secs: u64) {
        self.config.cache_ttl_secs = Some(ttl_secs);
    }

    fn set_extraction_timeout_secs(&mut self, timeout_secs: u64) {
        self.config.extraction_timeout_secs = Some(timeout_secs);
    }

    fn set_ocr_from_json(&mut self, ocr_json: &str) -> Result<(), String> {
        let ocr_config: OcrConfig =
            serde_json::from_str(ocr_json).map_err(|e| format!("Failed to parse OCR config JSON: {}", e))?;
        self.config.ocr = Some(ocr_config);
        Ok(())
    }

    fn set_pdf_from_json(&mut self, pdf_json: &str) -> Result<(), String> {
        let pdf_config: PdfConfig =
            serde_json::from_str(pdf_json).map_err(|e| format!("Failed to parse PDF config JSON: {}", e))?;
        self.config.pdf_options = Some(pdf_config);
        Ok(())
    }

    fn set_chunking_from_json(&mut self, chunking_json: &str) -> Result<(), String> {
        let chunking_config: ChunkingConfig =
            serde_json::from_str(chunking_json).map_err(|e| format!("Failed to parse chunking config JSON: {}", e))?;
        self.config.chunking = Some(chunking_config);
        Ok(())
    }

    fn set_image_extraction_from_json(&mut self, image_json: &str) -> Result<(), String> {
        let image_config: ImageExtractionConfig = serde_json::from_str(image_json)
            .map_err(|e| format!("Failed to parse image extraction config JSON: {}", e))?;
        self.config.images = Some(image_config);
        Ok(())
    }

    fn set_post_processor_from_json(&mut self, pp_json: &str) -> Result<(), String> {
        let pp_config: PostProcessorConfig =
            serde_json::from_str(pp_json).map_err(|e| format!("Failed to parse post processor config JSON: {}", e))?;
        self.config.postprocessor = Some(pp_config);
        Ok(())
    }

    fn set_language_detection_from_json(&mut self, ld_json: &str) -> Result<(), String> {
        let ld_config: LanguageDetectionConfig = serde_json::from_str(ld_json)
            .map_err(|e| format!("Failed to parse language detection config JSON: {}", e))?;
        self.config.language_detection = Some(ld_config);
        Ok(())
    }

    fn set_layout_from_json(&mut self, layout_json: &str) -> Result<(), String> {
        let layout_config: LayoutDetectionConfig =
            serde_json::from_str(layout_json).map_err(|e| format!("Failed to parse layout config JSON: {}", e))?;
        self.config.layout = Some(layout_config);
        Ok(())
    }

    #[cfg(feature = "tree-sitter")]
    fn set_tree_sitter_from_json(&mut self, ts_json: &str) -> Result<(), String> {
        let ts_config: TreeSitterConfig =
            serde_json::from_str(ts_json).map_err(|e| format!("Failed to parse tree-sitter config JSON: {}", e))?;
        self.config.tree_sitter = Some(ts_config);
        Ok(())
    }

    fn set_acceleration_from_json(&mut self, accel_json: &str) -> Result<(), String> {
        let accel_config: AccelerationConfig =
            serde_json::from_str(accel_json).map_err(|e| format!("Failed to parse acceleration config JSON: {}", e))?;
        self.config.acceleration = Some(accel_config);
        Ok(())
    }

    fn set_content_filter_from_json(&mut self, cf_json: &str) -> Result<(), String> {
        let cf_config: ContentFilterConfig =
            serde_json::from_str(cf_json).map_err(|e| format!("Failed to parse content filter config JSON: {}", e))?;
        self.config.content_filter = Some(cf_config);
        Ok(())
    }

    fn set_html_output_from_json(&mut self, json: &str) -> Result<(), String> {
        let html_output_config: HtmlOutputConfig =
            serde_json::from_str(json).map_err(|e| format!("Failed to parse HTML output config JSON: {}", e))?;
        self.config.html_output = Some(html_output_config);
        Ok(())
    }

    fn build(self) -> ExtractionConfig {
        self.config
    }
}

/// Create a new config builder.
///
/// Returns an opaque pointer to ConfigBuilder. Must be freed with
/// xberg_config_builder_free() or consumed by xberg_config_builder_build().
///
/// # Safety
///
/// The returned pointer must be freed with xberg_config_builder_free()
/// or passed to xberg_config_builder_build().
///
/// # Example (C)
///
/// ```c
/// ConfigBuilder* builder = xberg_config_builder_new();
/// xberg_config_builder_set_use_cache(builder, 1);
/// ExtractionConfig* config = xberg_config_builder_build(builder);
/// // builder is now consumed, don't call xberg_config_builder_free
/// xberg_config_free(config);
/// ```
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_new() -> *mut ConfigBuilder {
    ffi_panic_guard!("xberg_config_builder_new", {
        clear_last_error();
        Box::into_raw(Box::new(ConfigBuilder::new()))
    })
}

/// Set the use_cache field.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `use_cache` - 1 for true, 0 for false
///
/// # Returns
///
/// 0 on success, -1 on error (NULL builder)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_use_cache(builder: *mut ConfigBuilder, use_cache: i32) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_use_cache", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();
        unsafe { (*builder).set_use_cache(use_cache != 0) };
        0
    })
}

/// Set the include_document_structure field.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `include` - 1 for true, 0 for false
///
/// # Returns
///
/// 0 on success, -1 on error (NULL builder)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_include_document_structure(
    builder: *mut ConfigBuilder,
    include: i32,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_include_document_structure", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();
        unsafe { (*builder).set_include_document_structure(include != 0) };
        0
    })
}

/// Set the cache_namespace field for tenant isolation.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `namespace` - Cache namespace string; empty string clears the field
///
/// # Returns
///
/// 0 on success, -1 on error (NULL builder or NULL namespace)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - `namespace` must be a valid, non-null, null-terminated C string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_set_cache_namespace(
    builder: *mut ConfigBuilder,
    namespace: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_set_cache_namespace", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if namespace.is_null() {
            set_last_error("namespace pointer cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();
        // SAFETY: caller guarantees namespace is a valid null-terminated C string
        let namespace_str = unsafe { CStr::from_ptr(namespace) }.to_string_lossy();
        unsafe { (*builder).set_cache_namespace(&namespace_str) };
        0
    })
}

/// Set the cache_ttl_secs field for per-request cache TTL.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `ttl_secs` - Cache TTL in seconds; 0 skips the cache for this request
///
/// # Returns
///
/// 0 on success, -1 on error (NULL builder)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_set_cache_ttl_secs(builder: *mut ConfigBuilder, ttl_secs: u64) -> i32 {
    ffi_panic_guard_i32!("xberg_config_set_cache_ttl_secs", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();
        unsafe { (*builder).set_cache_ttl_secs(ttl_secs) };
        0
    })
}

/// Set the extraction_timeout_secs field for per-file timeout in batch extraction.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `timeout_secs` - Default per-file timeout in seconds; 0 clears the timeout
///
/// # Returns
///
/// 0 on success, -1 on error (NULL builder)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_set_extraction_timeout_secs(
    builder: *mut ConfigBuilder,
    timeout_secs: u64,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_set_extraction_timeout_secs", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();
        unsafe { (*builder).set_extraction_timeout_secs(timeout_secs) };
        0
    })
}

/// Set OCR configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `ocr_json` - JSON string like `{"backend": "tesseract", "languages": ["en"]}`
///
/// # Returns
///
/// 0 on success, -1 on error (check xberg_last_error)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `ocr_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_ocr(builder: *mut ConfigBuilder, ocr_json: *const c_char) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_ocr", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if ocr_json.is_null() {
            set_last_error("OCR JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(ocr_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in OCR JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_ocr_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set PDF configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `pdf_json` - JSON string for PDF config
///
/// # Returns
///
/// 0 on success, -1 on error
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `pdf_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_pdf(builder: *mut ConfigBuilder, pdf_json: *const c_char) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_pdf", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if pdf_json.is_null() {
            set_last_error("PDF JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(pdf_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in PDF JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_pdf_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set chunking configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `chunking_json` - JSON string for chunking config
///
/// # Returns
///
/// 0 on success, -1 on error
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `chunking_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_chunking(
    builder: *mut ConfigBuilder,
    chunking_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_chunking", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if chunking_json.is_null() {
            set_last_error("Chunking JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(chunking_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in chunking JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_chunking_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set image extraction configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `image_json` - JSON string for image extraction config
///
/// # Returns
///
/// 0 on success, -1 on error
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `image_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_image_extraction(
    builder: *mut ConfigBuilder,
    image_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_image_extraction", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if image_json.is_null() {
            set_last_error("Image extraction JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(image_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in image extraction JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_image_extraction_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set post-processor configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `pp_json` - JSON string for post-processor config
///
/// # Returns
///
/// 0 on success, -1 on error
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `pp_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_post_processor(
    builder: *mut ConfigBuilder,
    pp_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_post_processor", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if pp_json.is_null() {
            set_last_error("Post-processor JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(pp_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in post-processor JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_post_processor_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set language detection configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `ld_json` - JSON string for language detection config
///
/// # Returns
///
/// 0 on success, -1 on error
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `ld_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_language_detection(
    builder: *mut ConfigBuilder,
    ld_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_language_detection", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if ld_json.is_null() {
            set_last_error("Language detection JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(ld_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in language detection JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_language_detection_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set layout detection configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `layout_json` - JSON string like `{"preset": "fast", "apply_heuristics": true}`
///
/// # Returns
///
/// 0 on success, -1 on error (check xberg_last_error)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `layout_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_layout(
    builder: *mut ConfigBuilder,
    layout_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_layout", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if layout_json.is_null() {
            set_last_error("Layout JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(layout_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in layout JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_layout_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set tree-sitter configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `ts_json` - JSON string like `{"languages": ["python", "rust"], "process": {"structure": true}}`
///
/// # Returns
///
/// 0 on success, -1 on error (check xberg_last_error)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `ts_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[cfg(feature = "tree-sitter")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_tree_sitter(
    builder: *mut ConfigBuilder,
    ts_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_tree_sitter", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if ts_json.is_null() {
            set_last_error("Tree-sitter JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(ts_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in tree-sitter JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_tree_sitter_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set acceleration configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `accel_json` - JSON string for acceleration config
///
/// # Returns
///
/// 0 on success, -1 on error (check xberg_last_error)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `accel_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_acceleration(
    builder: *mut ConfigBuilder,
    accel_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_acceleration", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if accel_json.is_null() {
            set_last_error("Acceleration JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(accel_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in acceleration JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_acceleration_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set content filter configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `cf_json` - JSON string for content filter config
///
/// # Returns
///
/// 0 on success, -1 on error (check xberg_last_error)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `cf_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_content_filter(
    builder: *mut ConfigBuilder,
    cf_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_content_filter", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if cf_json.is_null() {
            set_last_error("Content filter JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(cf_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in content filter JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_content_filter_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Set HTML output configuration from JSON.
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder
/// * `html_output_json` - JSON string for HTML output config
///
/// # Returns
///
/// 0 on success, -1 on error (check xberg_last_error)
///
/// # Safety
///
/// This function is meant to be called from C/FFI code. The caller must ensure:
/// - `builder` must be a valid, non-null pointer previously returned by `xberg_config_builder_new`
/// - The pointer must be properly aligned and point to a valid ConfigBuilder instance
/// - `html_output_json` must be a valid, non-null pointer to a null-terminated UTF-8 string
/// - The string pointer must remain valid for the duration of the function call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_set_html_output(
    builder: *mut ConfigBuilder,
    html_output_json: *const c_char,
) -> i32 {
    ffi_panic_guard_i32!("xberg_config_builder_set_html_output", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return -1;
        }
        if html_output_json.is_null() {
            set_last_error("HTML output JSON cannot be NULL".to_string());
            return -1;
        }

        clear_last_error();

        let json_str = match unsafe { CStr::from_ptr(html_output_json) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in HTML output JSON: {}", e));
                return -1;
            }
        };

        match unsafe { (*builder).set_html_output_from_json(json_str) } {
            Ok(()) => 0,
            Err(e) => {
                set_last_error(e);
                -1
            }
        }
    })
}

/// Build the final ExtractionConfig and consume the builder.
///
/// After calling this function, the builder pointer is invalid and must not be used.
/// The returned ExtractionConfig must be freed with xberg_config_free().
///
/// # Arguments
///
/// * `builder` - Non-null pointer to ConfigBuilder (will be consumed)
///
/// # Returns
///
/// Pointer to ExtractionConfig on success, NULL on error
///
/// # Safety
///
/// - `builder` is consumed and must not be used after this call
/// - Do NOT call xberg_config_builder_free() after this function
/// - The returned ExtractionConfig must be freed with xberg_config_free()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_build(builder: *mut ConfigBuilder) -> *mut ExtractionConfig {
    ffi_panic_guard!("xberg_config_builder_build", {
        if builder.is_null() {
            set_last_error("ConfigBuilder pointer cannot be NULL".to_string());
            return ptr::null_mut();
        }

        clear_last_error();
        let builder_box = unsafe { Box::from_raw(builder) };
        let config = builder_box.build();
        Box::into_raw(Box::new(config))
    })
}

/// Free a ConfigBuilder without building.
///
/// Use this to discard a builder without creating a config.
/// Do NOT call this after xberg_config_builder_build() (builder is already consumed).
///
/// # Arguments
///
/// * `builder` - Pointer to ConfigBuilder, can be NULL (no-op)
///
/// # Safety
///
/// - `builder` can be NULL (no-op)
/// - Do NOT call this after xberg_config_builder_build()
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_builder_free(builder: *mut ConfigBuilder) {
    if !builder.is_null() {
        unsafe { drop(Box::from_raw(builder)) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    #[test]
    fn test_builder_basic_flow() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            let result = xberg_config_builder_set_use_cache(builder, 1);
            assert_eq!(result, 0);

            let config = xberg_config_builder_build(builder);
            assert!(!config.is_null());

            assert!((*config).use_cache);

            let _ = Box::from_raw(config);
        }
    }

    #[test]
    fn test_builder_include_document_structure() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            let result = xberg_config_builder_set_include_document_structure(builder, 1);
            assert_eq!(result, 0);

            let config = xberg_config_builder_build(builder);
            assert!(!config.is_null());

            assert!((*config).include_document_structure);

            let _ = Box::from_raw(config);
        }
    }

    #[test]
    fn test_builder_with_ocr() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            let ocr_json = CString::new(r#"{"backend":"tesseract","languages":["en"]}"#).unwrap();
            let result = xberg_config_builder_set_ocr(builder, ocr_json.as_ptr());
            assert_eq!(result, 0);

            let config = xberg_config_builder_build(builder);
            assert!(!config.is_null());

            assert!((*config).ocr.is_some());

            let _ = Box::from_raw(config);
        }
    }

    #[test]
    fn test_builder_null_checks() {
        unsafe {
            let result = xberg_config_builder_set_use_cache(ptr::null_mut(), 1);
            assert_eq!(result, -1);

            let config = xberg_config_builder_build(ptr::null_mut());
            assert!(config.is_null());
        }
    }

    #[test]
    fn test_builder_free() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            // Free without building should not crash ~keep
            xberg_config_builder_free(builder);

            // Freeing NULL should not crash ~keep
            xberg_config_builder_free(ptr::null_mut());
        }
    }

    #[test]
    fn test_builder_with_content_filter() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            let cf_json = CString::new(
                r#"{"include_headers":true,"include_footers":false,"strip_repeating_text":true,"include_watermarks":false}"#,
            )
            .unwrap();
            let result = xberg_config_builder_set_content_filter(builder, cf_json.as_ptr());
            assert_eq!(result, 0);

            let config = xberg_config_builder_build(builder);
            assert!(!config.is_null());

            assert!((*config).content_filter.is_some());
            let cf = (*config).content_filter.as_ref().unwrap();
            assert!(cf.include_headers);
            assert!(!cf.include_footers);
            assert!(cf.strip_repeating_text);
            assert!(!cf.include_watermarks);

            let _ = Box::from_raw(config);
        }
    }

    #[test]
    fn test_builder_with_html_output() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            let html_json = CString::new(
                r#"{"theme":"github","class_prefix":"kb-","embed_css":true,"css":".kb-p { color: red; }"}"#,
            )
            .unwrap();
            let result = xberg_config_builder_set_html_output(builder, html_json.as_ptr());
            assert_eq!(result, 0);

            let config = xberg_config_builder_build(builder);
            assert!(!config.is_null());

            assert!((*config).html_output.is_some());
            let ho = (*config).html_output.as_ref().unwrap();
            assert_eq!(ho.css.as_deref(), Some(".kb-p { color: red; }"));
            assert!(ho.embed_css);

            let _ = Box::from_raw(config);
        }
    }

    #[test]
    fn test_builder_invalid_json() {
        unsafe {
            let builder = xberg_config_builder_new();
            assert!(!builder.is_null());

            let invalid_json = CString::new("not valid json").unwrap();
            let result = xberg_config_builder_set_ocr(builder, invalid_json.as_ptr());
            assert_eq!(result, -1);

            // Builder should still be usable ~keep
            let result = xberg_config_builder_set_use_cache(builder, 0);
            assert_eq!(result, 0);

            let config = xberg_config_builder_build(builder);
            assert!(!config.is_null());

            let _ = Box::from_raw(config);
        }
    }
}
