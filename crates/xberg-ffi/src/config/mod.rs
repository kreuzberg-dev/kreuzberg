//! Centralized FFI configuration parsing module.
//!
//! This module consolidates all configuration parsing logic that was previously
//! duplicated across all language bindings (Python, TypeScript, Ruby, Java, Go, C#).
//!
//! Instead of each binding reimplementing config parsing from JSON, they now
//! call the FFI functions provided here, ensuring:
//! - Single source of truth for validation rules
//! - Consistent behavior across all languages
//! - Elimination of drift/inconsistencies
//! - Better performance (no JSON round-trips in language bindings)

mod html;
mod loader;
mod merge;
mod parse;
mod serialize;

pub use loader::{discover_config_as_json, load_config_as_json, load_config_from_file};
#[cfg(feature = "embeddings")]
pub use loader::{get_embedding_preset, list_embedding_presets};
pub use merge::merge_configs;
pub use parse::parse_extraction_config_from_json;
pub use serialize::{config_to_json_string, get_field_as_json, json_to_c_string};

use crate::ffi_panic_guard;
#[cfg(feature = "embeddings")]
use crate::helpers::string_to_c_string;
use crate::helpers::{clear_last_error, set_last_error};
use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::Path;
use std::ptr;
use xberg::core::config::ExtractionConfig;

/// Parse an ExtractionConfig from a JSON string.
///
/// This is the primary FFI entry point for all language bindings to parse
/// configuration from JSON. Replaces the need for each binding to implement
/// its own JSON parsing logic.
///
/// # Arguments
///
/// * `json_config` - Null-terminated C string containing JSON configuration
///
/// # Returns
///
/// A pointer to an ExtractionConfig struct that MUST be freed with
/// `xberg_config_free`, or NULL on error (check xberg_last_error).
///
/// # Safety
///
/// - `json_config` must be a valid null-terminated C string
/// - The returned pointer must be freed with `xberg_config_free`
/// - Returns NULL if parsing fails (error available via `xberg_last_error`)
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_from_json(json_config: *const c_char) -> *mut ExtractionConfig {
    if json_config.is_null() {
        set_last_error("Config JSON cannot be NULL".to_string());
        return ptr::null_mut();
    }

    clear_last_error();

    let json_str = match unsafe { CStr::from_ptr(json_config) }.to_str() {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid UTF-8 in config JSON: {}", e));
            return ptr::null_mut();
        }
    };

    match parse_extraction_config_from_json(json_str) {
        Ok(config) => Box::into_raw(Box::new(config)),
        Err(e) => {
            set_last_error(e);
            ptr::null_mut()
        }
    }
}

/// Free an ExtractionConfig allocated by xberg_config_from_json or similar.
///
/// # Safety
///
/// - `config` must be a pointer previously returned by a config creation function
/// - `config` can be NULL (no-op)
/// - `config` must not be used after this call
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_free(config: *mut ExtractionConfig) {
    if !config.is_null() {
        let _ = unsafe { Box::from_raw(config) };
    }
}

/// Validate a JSON config string without parsing it.
///
/// # Returns
///
/// - 1 if valid (would parse successfully)
/// - 0 if invalid (check `xberg_last_error` for details)
///
/// # Safety
///
/// - `json_config` must be a valid null-terminated C string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_is_valid(json_config: *const c_char) -> i32 {
    if json_config.is_null() {
        set_last_error("Config JSON cannot be NULL".to_string());
        return 0;
    }

    clear_last_error();

    let json_str = match unsafe { CStr::from_ptr(json_config) }.to_str() {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid UTF-8 in config JSON: {}", e));
            return 0;
        }
    };

    match parse_extraction_config_from_json(json_str) {
        Ok(_) => 1,
        Err(e) => {
            set_last_error(e);
            0
        }
    }
}

/// Serialize an ExtractionConfig to JSON string.
///
/// # Safety
///
/// - `config` must be a valid pointer to an ExtractionConfig
/// - The returned pointer must be freed with `xberg_free_string`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_to_json(config: *const ExtractionConfig) -> *mut c_char {
    if config.is_null() {
        set_last_error("Config cannot be NULL".to_string());
        return ptr::null_mut();
    }

    clear_last_error();

    match config_to_json_string(unsafe { &*config }) {
        Some(json) => json_to_c_string(json),
        None => ptr::null_mut(),
    }
}

/// Get a specific field from config as JSON string.
///
/// # Safety
///
/// - `config` must be a valid pointer to an ExtractionConfig
/// - `field_name` must be a valid null-terminated C string
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_get_field(
    config: *const ExtractionConfig,
    field_name: *const c_char,
) -> *mut c_char {
    if config.is_null() {
        set_last_error("Config cannot be NULL".to_string());
        return ptr::null_mut();
    }

    if field_name.is_null() {
        set_last_error("Field name cannot be NULL".to_string());
        return ptr::null_mut();
    }

    clear_last_error();

    let field_str = match unsafe { CStr::from_ptr(field_name) }.to_str() {
        Ok(s) => s,
        Err(e) => {
            set_last_error(format!("Invalid UTF-8 in field name: {}", e));
            return ptr::null_mut();
        }
    };

    match get_field_as_json(unsafe { &*config }, field_str) {
        Some(json) => json_to_c_string(json),
        None => ptr::null_mut(),
    }
}

/// Merge two configs (override takes precedence over base).
///
/// # Returns
///
/// - 1 on success
/// - 0 on error (check `xberg_last_error`)
///
/// # Safety
///
/// - `base` must be a valid mutable pointer to an ExtractionConfig
/// - `override_config` must be a valid pointer to an ExtractionConfig
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_merge(
    base: *mut ExtractionConfig,
    override_config: *const ExtractionConfig,
) -> i32 {
    if base.is_null() {
        set_last_error("Base config cannot be NULL".to_string());
        return 0;
    }

    if override_config.is_null() {
        set_last_error("Override config cannot be NULL".to_string());
        return 0;
    }

    clear_last_error();

    merge_configs(unsafe { &mut *base }, unsafe { &*override_config });

    1
}

/// Load an ExtractionConfig from a file (returns JSON string).
///
/// # Safety
///
/// - `file_path` must be a valid null-terminated C string
/// - The returned string must be freed with `xberg_free_string`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_load_extraction_config_from_file(file_path: *const c_char) -> *mut c_char {
    ffi_panic_guard!("xberg_load_extraction_config_from_file", {
        clear_last_error();

        if file_path.is_null() {
            set_last_error("file_path cannot be NULL".to_string());
            return ptr::null_mut();
        }

        let path_str = match unsafe { CStr::from_ptr(file_path) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in file path: {}", e));
                return ptr::null_mut();
            }
        };

        match load_config_as_json(path_str) {
            Ok(json) => match CString::new(json) {
                Ok(cstr) => cstr.into_raw(),
                Err(e) => {
                    set_last_error(format!("Failed to create C string: {}", e));
                    ptr::null_mut()
                }
            },
            Err(e) => {
                set_last_error(e);
                ptr::null_mut()
            }
        }
    })
}

/// Load an ExtractionConfig from a file (returns pointer to config struct).
///
/// # Safety
///
/// - `path` must be a valid null-terminated C string
/// - The returned pointer must be freed with `xberg_config_free`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_from_file(path: *const c_char) -> *mut ExtractionConfig {
    ffi_panic_guard!("xberg_config_from_file", {
        clear_last_error();

        if path.is_null() {
            set_last_error("Config path cannot be NULL".to_string());
            return ptr::null_mut();
        }

        let path_str = match unsafe { CStr::from_ptr(path) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in config path: {}", e));
                return ptr::null_mut();
            }
        };

        let path_buf = Path::new(path_str);

        match load_config_from_file(path_buf) {
            Ok(config) => Box::into_raw(Box::new(config)),
            Err(e) => {
                set_last_error(e);
                ptr::null_mut()
            }
        }
    })
}

/// Discover and load an ExtractionConfig by searching parent directories.
///
/// # Safety
///
/// - The returned string must be freed with `xberg_free_string`
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_config_discover() -> *mut c_char {
    ffi_panic_guard!("xberg_config_discover", {
        clear_last_error();

        match discover_config_as_json() {
            Some(json) => match CString::new(json) {
                Ok(cstr) => cstr.into_raw(),
                Err(e) => {
                    set_last_error(format!("Failed to serialize config: {}", e));
                    ptr::null_mut()
                }
            },
            None => ptr::null_mut(),
        }
    })
}

/// List available embedding preset names.
///
/// # Safety
///
/// - Returned string is a JSON array and must be freed with `xberg_free_string`
#[cfg(feature = "embeddings")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_list_embedding_presets() -> *mut c_char {
    ffi_panic_guard!("xberg_list_embedding_presets", {
        clear_last_error();

        match list_embedding_presets() {
            Ok(json) => match string_to_c_string(json) {
                Ok(ptr) => ptr,
                Err(e) => {
                    set_last_error(e);
                    ptr::null_mut()
                }
            },
            Err(e) => {
                set_last_error(e);
                ptr::null_mut()
            }
        }
    })
}

/// Get a specific embedding preset by name.
///
/// # Safety
///
/// - `name` must be a valid null-terminated C string
/// - Returned string is JSON object and must be freed with `xberg_free_string`
#[cfg(feature = "embeddings")]
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xberg_get_embedding_preset(name: *const c_char) -> *mut c_char {
    ffi_panic_guard!("xberg_get_embedding_preset", {
        clear_last_error();

        if name.is_null() {
            set_last_error("preset name cannot be NULL".to_string());
            return ptr::null_mut();
        }

        let preset_name = match unsafe { CStr::from_ptr(name) }.to_str() {
            Ok(s) => s,
            Err(e) => {
                set_last_error(format!("Invalid UTF-8 in preset name: {}", e));
                return ptr::null_mut();
            }
        };

        match get_embedding_preset(preset_name) {
            Ok(json) => match string_to_c_string(json) {
                Ok(ptr) => ptr,
                Err(e) => {
                    set_last_error(e);
                    ptr::null_mut()
                }
            },
            Err(e) => {
                set_last_error(e);
                ptr::null_mut()
            }
        }
    })
}
