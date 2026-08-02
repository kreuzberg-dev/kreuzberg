//! Image and configuration validation logic.
//!
//! This module handles validation of images, language files, and Tesseract configuration
//! before OCR processing begins.

use crate::ocr::error::OcrError;
use crate::ocr::validation::TESSERACT_SUPPORTED_LANGUAGE_CODES;
use std::borrow::Cow;
use std::env;
use std::path::{Path, PathBuf};

/// Validate language configuration and check for traineddata files.
///
/// This function validates that:
/// 1. Language string is not empty
/// 2. Traineddata files exist for all specified languages
///
/// # Arguments
///
/// * `language` - Language code(s) to validate (can be "eng" or "eng+fra" etc.)
/// * `tessdata_path` - Path to tessdata directory
///
/// # Returns
///
/// `Ok(())` if validation passes, otherwise returns an error
pub(super) fn validate_language_and_traineddata(language: &str, tessdata_path: &str) -> Result<(), OcrError> {
    // Validate language before initializing to prevent segfault ~keep
    if language.trim().is_empty() {
        return Err(OcrError::TesseractInitializationFailed(
            "Language cannot be empty. Please specify a valid language code (e.g., 'eng')".to_string(),
        ));
    }

    // Validate language file exists before initializing to prevent segfault ~keep
    if !tessdata_path.is_empty() {
        let languages: Vec<&str> = language.split('+').collect();
        for lang in languages {
            let lang = lang.trim();
            if lang.is_empty() {
                continue;
            }
            let traineddata_path = Path::new(tessdata_path).join(format!("{}.traineddata", lang));
            if !traineddata_path.exists() {
                return Err(OcrError::TesseractInitializationFailed(format!(
                    "Language '{}' not found. Traineddata file does not exist: {}",
                    lang,
                    traineddata_path.display()
                )));
            }
        }
    }

    Ok(())
}

/// Resolve tessdata path with runtime fallback chain and language validation.
///
/// Implements a multi-step resolution order. The first location that contains
/// every requested language wins:
/// 1. `OcrConfig.tessdata_path` override (`override_path` argument)
/// 2. `TESSDATA_PREFIX` environment variable
/// 3. `XBERG_CACHE_DIR/tessdata`
/// 4. `crate::cache_dir::resolve_cache_base().join("tessdata")`
/// 5. System fallback paths (macOS, Linux, Windows)
/// 6. If languages are still missing, materialize bundled `eng` bytes and
///    download any others from the GitHub `tessdata_fast` repo into the cache
///
/// # Arguments
///
/// * `languages` - List of language codes to resolve (e.g., `["eng", "fra"]`)
/// * `override_path` - Optional explicit tessdata directory from `OcrConfig.tessdata_path`,
///   checked before any environment or system location.
///
/// # Returns
///
/// `Ok(String)` with the path to a valid tessdata directory containing all
/// requested languages, or `Err(OcrError)` if resolution fails.
pub(super) fn resolve_tessdata_path(languages: &[String], override_path: Option<&Path>) -> Result<String, OcrError> {
    for dir in tessdata_search_dirs(override_path) {
        if all_languages_exist(&dir, languages)? {
            return Ok(dir);
        }
    }

    let download_dest = crate::cache_dir::resolve_cache_base().join("tessdata");
    std::fs::create_dir_all(&download_dest).map_err(|e| {
        OcrError::TesseractInitializationFailed(format!(
            "Failed to create tessdata cache directory '{}': {}",
            download_dest.display(),
            e
        ))
    })?;

    materialize_missing_languages(&download_dest, languages)?;

    let dest_str = download_dest
        .to_str()
        .ok_or_else(|| OcrError::TesseractInitializationFailed("Tessdata path is not valid UTF-8".to_string()))?;

    if all_languages_exist(dest_str, languages)? {
        Ok(dest_str.to_string())
    } else {
        Err(OcrError::TesseractInitializationFailed(format!(
            "Failed to resolve all requested languages: {:?}",
            languages
        )))
    }
}

/// Candidate tessdata directories in the resolver's priority order:
/// `override_path` (`OcrConfig.tessdata_path`), `TESSDATA_PREFIX`,
/// `XBERG_CACHE_DIR/tessdata`, the xberg cache base, then system paths.
///
/// Shared by [`resolve_tessdata_path`] and doctor's check-only probe so both
/// report on the same search chain.
pub(crate) fn tessdata_search_dirs(override_path: Option<&Path>) -> Vec<String> {
    let mut dirs = Vec::new();

    if let Some(path) = override_path
        && let Some(path_str) = path.to_str()
        && !path_str.is_empty()
    {
        dirs.push(path_str.to_string());
    }

    if let Ok(path) = env::var("TESSDATA_PREFIX")
        && !path.is_empty()
    {
        dirs.push(path);
    }

    if let Ok(cache_dir) = env::var("XBERG_CACHE_DIR") {
        dirs.push(PathBuf::from(cache_dir).join("tessdata").to_string_lossy().into_owned());
    }

    dirs.push(
        crate::cache_dir::resolve_cache_base()
            .join("tessdata")
            .to_string_lossy()
            .into_owned(),
    );

    for path in SYSTEM_TESSDATA_PATHS {
        dirs.push((*path).to_string());
    }

    dirs
}

const SYSTEM_TESSDATA_PATHS: &[&str] = &[
    "/opt/homebrew/share/tessdata",
    "/opt/homebrew/opt/tesseract/share/tessdata",
    "/usr/local/opt/tesseract/share/tessdata",
    "/usr/share/tesseract-ocr/5/tessdata",
    "/usr/share/tesseract-ocr/4/tessdata",
    "/usr/share/tessdata",
    "/usr/local/share/tessdata",
    r#"C:\Program Files\Tesseract-OCR\tessdata"#,
    r#"C:\ProgramData\Tesseract-OCR\tessdata"#,
];

/// Check if all languages in the list exist as traineddata files in the given directory.
fn all_languages_exist(tessdata_path: &str, languages: &[String]) -> Result<bool, OcrError> {
    if tessdata_path.is_empty() || languages.is_empty() {
        return Ok(false);
    }

    let tessdata_dir = Path::new(tessdata_path);
    if !tessdata_dir.exists() {
        return Ok(false);
    }

    for lang in languages {
        let traineddata_path = tessdata_dir.join(format!("{}.traineddata", lang));
        if !traineddata_path.exists() {
            return Ok(false);
        }
    }

    Ok(true)
}

/// Materialize missing language files by attempting to use bundled 'eng' or download from GitHub.
#[cfg(not(target_arch = "wasm32"))]
fn materialize_missing_languages(tessdata_path: &Path, languages: &[String]) -> Result<(), OcrError> {
    use crate::ocr::tessdata_download::download_language_pack;

    for lang in languages {
        let traineddata_path = tessdata_path.join(format!("{}.traineddata", lang));
        if traineddata_path.exists() {
            continue;
        }

        if lang == "eng"
            && let Some(bundled_bytes) = xberg_tesseract::bundled_eng_traineddata()
        {
            std::fs::write(&traineddata_path, bundled_bytes).map_err(|e| {
                OcrError::TesseractInitializationFailed(format!(
                    "Failed to write bundled eng.traineddata to '{}': {}",
                    traineddata_path.display(),
                    e
                ))
            })?;
            tracing::info!("Materialized bundled eng.traineddata to '{}'", tessdata_path.display());
            continue;
        }

        download_language_pack(lang, tessdata_path)?;
    }

    Ok(())
}

/// WASM stub: no network access, no file writes.
#[cfg(target_arch = "wasm32")]
fn materialize_missing_languages(_tessdata_path: &Path, languages: &[String]) -> Result<(), OcrError> {
    Err(OcrError::TesseractInitializationFailed(format!(
        "Cannot download language packs on WASM. Requested: {:?}",
        languages
    )))
}

/// Resolve all installed Tesseract languages from the tessdata directory.
///
/// Scans the tessdata directory for `*.traineddata` files, filters against
/// known Tesseract language codes (excluding non-language files like `osd`),
/// and returns a `+`-separated language string (e.g., `"eng+fra+deu"`).
///
/// # Arguments
///
/// * `tessdata_path` - Path to the tessdata directory
///
/// # Returns
///
/// A `+`-separated string of installed language codes, or an error if no languages are found.
pub(super) fn resolve_all_installed_languages(tessdata_path: &str) -> Result<String, OcrError> {
    if tessdata_path.is_empty() {
        return Err(OcrError::TesseractInitializationFailed(
            "Cannot resolve installed languages: tessdata path is empty. \
             Set TESSDATA_PREFIX or install Tesseract with language data."
                .to_string(),
        ));
    }

    let tessdata_dir = Path::new(tessdata_path);
    if !tessdata_dir.exists() {
        return Err(OcrError::TesseractInitializationFailed(format!(
            "Tessdata directory does not exist: {}",
            tessdata_path
        )));
    }

    let entries = std::fs::read_dir(tessdata_dir).map_err(|e| {
        OcrError::TesseractInitializationFailed(format!("Failed to read tessdata directory '{}': {}", tessdata_path, e))
    })?;

    const EXCLUDED: &[&str] = &["osd", "equ"];

    let mut languages: Vec<String> = entries
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let file_name = path.file_name()?.to_str()?;
            let lang = file_name.strip_suffix(".traineddata")?;
            if EXCLUDED.contains(&lang) {
                return None;
            }
            if TESSERACT_SUPPORTED_LANGUAGE_CODES.contains(lang) {
                Some(lang.to_string())
            } else {
                None
            }
        })
        .collect();

    if languages.is_empty() {
        return Err(OcrError::TesseractInitializationFailed(format!(
            "No installed Tesseract languages found in '{}'",
            tessdata_path
        )));
    }

    languages.sort();
    Ok(languages.join("+"))
}

/// Strip control characters from text, preserving whitespace.
///
/// Removes control characters (0x00-0x1F, 0x7F) except for newlines, carriage returns, and tabs.
///
/// # Arguments
///
/// * `text` - Text to clean
///
/// # Returns
///
/// Cleaned text with control characters removed
pub(super) fn strip_control_characters(text: &str) -> Cow<'_, str> {
    if text
        .chars()
        .any(|c| matches!(c, '\u{0000}'..='\u{001F}' | '\u{007F}') && c != '\n' && c != '\r' && c != '\t')
    {
        Cow::Owned(
            text.chars()
                .filter(|c| !matches!(c, '\u{0000}'..='\u{001F}' | '\u{007F}') || matches!(c, '\n' | '\r' | '\t'))
                .collect(),
        )
    } else {
        Cow::Borrowed(text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_all_installed_languages_success() {
        let dir = tempfile::tempdir().unwrap();
        let tessdata = dir.path();

        std::fs::write(tessdata.join("eng.traineddata"), b"").unwrap();
        std::fs::write(tessdata.join("fra.traineddata"), b"").unwrap();
        std::fs::write(tessdata.join("deu.traineddata"), b"").unwrap();

        let result = resolve_all_installed_languages(tessdata.to_str().unwrap()).unwrap();
        assert_eq!(result, "deu+eng+fra");
    }

    #[test]
    fn test_resolve_all_installed_languages_excludes_osd() {
        let dir = tempfile::tempdir().unwrap();
        let tessdata = dir.path();

        std::fs::write(tessdata.join("eng.traineddata"), b"").unwrap();
        std::fs::write(tessdata.join("osd.traineddata"), b"").unwrap();

        let result = resolve_all_installed_languages(tessdata.to_str().unwrap()).unwrap();
        assert_eq!(result, "eng");
    }

    #[test]
    fn test_resolve_all_installed_languages_excludes_equ() {
        let dir = tempfile::tempdir().unwrap();
        let tessdata = dir.path();

        std::fs::write(tessdata.join("eng.traineddata"), b"").unwrap();
        std::fs::write(tessdata.join("equ.traineddata"), b"").unwrap();

        let result = resolve_all_installed_languages(tessdata.to_str().unwrap()).unwrap();
        assert_eq!(result, "eng");
    }

    #[test]
    fn test_resolve_all_installed_languages_excludes_unknown() {
        let dir = tempfile::tempdir().unwrap();
        let tessdata = dir.path();

        std::fs::write(tessdata.join("eng.traineddata"), b"").unwrap();
        std::fs::write(tessdata.join("notareal.traineddata"), b"").unwrap();

        let result = resolve_all_installed_languages(tessdata.to_str().unwrap()).unwrap();
        assert_eq!(result, "eng");
    }

    #[test]
    fn test_resolve_all_installed_languages_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let result = resolve_all_installed_languages(dir.path().to_str().unwrap());
        assert!(result.is_err());
    }

    #[test]
    fn test_resolve_all_installed_languages_empty_path() {
        let result = resolve_all_installed_languages("");
        assert!(result.is_err());
    }

    #[test]
    fn test_all_languages_exist_returns_true_when_present() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("eng.traineddata"), b"x").unwrap();
        std::fs::write(dir.path().join("fra.traineddata"), b"x").unwrap();

        let langs = vec!["eng".to_string(), "fra".to_string()];
        assert!(all_languages_exist(dir.path().to_str().unwrap(), &langs).unwrap());
    }

    #[test]
    fn test_all_languages_exist_returns_false_when_one_missing() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("eng.traineddata"), b"x").unwrap();

        let langs = vec!["eng".to_string(), "deu".to_string()];
        assert!(!all_languages_exist(dir.path().to_str().unwrap(), &langs).unwrap());
    }

    #[test]
    fn test_all_languages_exist_false_for_empty_path_or_langs() {
        assert!(!all_languages_exist("", &["eng".to_string()]).unwrap());
        let dir = tempfile::tempdir().unwrap();
        assert!(!all_languages_exist(dir.path().to_str().unwrap(), &[]).unwrap());
    }

    #[test]
    fn test_all_languages_exist_false_for_missing_dir() {
        let langs = vec!["eng".to_string()];
        assert!(!all_languages_exist("/nonexistent/path/xyz", &langs).unwrap());
    }

    #[test]
    fn test_resolve_tessdata_path_prefers_override() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("eng.traineddata"), b"x").unwrap();

        let langs = vec!["eng".to_string()];
        let resolved = resolve_tessdata_path(&langs, Some(dir.path())).unwrap();
        assert_eq!(resolved, dir.path().to_str().unwrap());
    }

    #[test]
    fn test_resolve_tessdata_path_skips_override_missing_lang() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("eng.traineddata"), b"x").unwrap();

        let langs = vec!["deu".to_string()];
        let result = resolve_tessdata_path(&langs, Some(dir.path()));
        if let Ok(path) = result {
            assert_ne!(path, dir.path().to_str().unwrap());
        }
    }

    #[cfg(feature = "bundle-tessdata-eng")]
    #[test]
    fn test_materialize_eng_from_bundled_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let langs = vec!["eng".to_string()];

        materialize_missing_languages(dir.path(), &langs).unwrap();
        assert!(dir.path().join("eng.traineddata").exists());
        assert!(all_languages_exist(dir.path().to_str().unwrap(), &langs).unwrap());
    }

    #[test]
    fn test_strip_control_characters() {
        let input = "Hello\x00World\x01Test";
        let output = strip_control_characters(input);
        assert_eq!(output, "HelloWorldTest");

        let input_with_newlines = "Hello\nWorld\rTest\t!";
        let output = strip_control_characters(input_with_newlines);
        assert_eq!(output, "Hello\nWorld\rTest\t!");
    }

    #[test]
    fn test_strip_control_characters_all_control() {
        let input = "\x00\x01\x02\x03";
        let output = strip_control_characters(input);
        assert_eq!(output, "");
    }

    #[test]
    fn test_strip_control_characters_no_control() {
        let input = "Hello World Test";
        let output = strip_control_characters(input);
        assert_eq!(output, "Hello World Test");
    }

    #[test]
    fn test_strip_control_characters_delete_char() {
        let input = "Hello\x7FWorld";
        let output = strip_control_characters(input);
        assert_eq!(output, "HelloWorld");
    }
}
