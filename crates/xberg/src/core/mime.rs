//! MIME type detection and validation.
//!
//! This module provides utilities for detecting MIME types from file extensions
//! and validating them against supported types.
//!
//! Format information is centralized in the `FORMATS` registry. All extension-to-MIME
//! mappings and supported MIME type validation are derived from this single source of truth.

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
use crate::extractors::security::SecurityLimits;
use crate::{Result, XbergError};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::io::Read;
#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
use std::io::{Seek, SeekFrom};
use std::path::Path;
use std::sync::LazyLock;

/// A supported document format entry.
///
/// Represents a file extension and its corresponding MIME type that Xberg can process.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct SupportedFormat {
    /// File extension (without leading dot), e.g., "pdf", "docx"
    pub extension: String,
    /// MIME type string, e.g., "application/pdf"
    pub mime_type: String,
}

#[cfg(feature = "api")]
pub(crate) const OCTET_STREAM_MIME_TYPE: &str = "application/octet-stream";
pub(crate) const HTML_MIME_TYPE: &str = "text/html";
pub(crate) const PDF_MIME_TYPE: &str = "application/pdf";
pub(crate) const PLAIN_TEXT_MIME_TYPE: &str = "text/plain";
pub(crate) const POWER_POINT_MIME_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";
pub(crate) const DOCX_MIME_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub(crate) const LEGACY_WORD_MIME_TYPE: &str = "application/msword";
pub(crate) const LEGACY_POWERPOINT_MIME_TYPE: &str = "application/vnd.ms-powerpoint";

pub(crate) const PST_MIME_TYPE: &str = "application/vnd.ms-outlook-pst";
pub(crate) const JSON_MIME_TYPE: &str = "application/json";
pub(crate) const XML_MIME_TYPE: &str = "application/xml";
#[cfg(feature = "tree-sitter")]
pub(crate) const SOURCE_CODE_MIME_TYPE: &str = "text/x-source-code";

pub(crate) const EXCEL_MIME_TYPE: &str = "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet";
pub(crate) const ODT_MIME_TYPE: &str = "application/vnd.oasis.opendocument.text";
pub(crate) const ODP_MIME_TYPE: &str = "application/vnd.oasis.opendocument.presentation";
pub(crate) const ODS_MIME_TYPE: &str = "application/vnd.oasis.opendocument.spreadsheet";
#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
const ZIP_MIME_TYPE: &str = "application/zip";

pub(crate) const HWPX_MIME_TYPE: &str = "application/haansofthwpx";
pub(crate) const IWORK_PAGES_MIME_TYPE: &str = "application/x-iwork-pages-sffpages";
pub(crate) const IWORK_NUMBERS_MIME_TYPE: &str = "application/x-iwork-numbers-sffnumbers";
pub(crate) const IWORK_KEYNOTE_MIME_TYPE: &str = "application/x-iwork-keynote-sffkey";

/// A format definition in the centralized registry.
///
/// Each entry defines a document format with its file extensions, primary MIME type,
/// and any MIME type aliases that should also be accepted for this format.
struct FormatEntry {
    /// File extensions (without leading dot). First is canonical.
    extensions: &'static [&'static str],
    /// Primary MIME type for this format.
    mime_type: &'static str,
    /// Additional MIME type aliases that should also be accepted.
    aliases: &'static [&'static str],
}

/// Centralized format registry - the single source of truth for all supported formats.
///
/// Adding a new format requires only adding a single entry here. Both `EXT_TO_MIME`
/// (extension-to-MIME mapping) and `SUPPORTED_MIME_TYPES` (validation set) are
/// derived from this array automatically.
static FORMATS: &[FormatEntry] = &[
    FormatEntry {
        extensions: &["txt"],
        mime_type: "text/plain",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/troff",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-mdoc",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-pod",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-dokuwiki",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["md", "markdown"],
        mime_type: "text/markdown",
        aliases: &["text/x-markdown"],
    },
    FormatEntry {
        extensions: &["commonmark"],
        mime_type: "text/x-commonmark",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-gfm",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-markdown-extra",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-multimarkdown",
        aliases: &[],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-pandoc",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["qmd"],
        mime_type: "text/x-quarto",
        aliases: &["application/x-quarto"],
    },
    FormatEntry {
        extensions: &["rmd"],
        mime_type: "text/x-r-markdown",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["mdx"],
        mime_type: "text/mdx",
        aliases: &["text/x-mdx"],
    },
    FormatEntry {
        extensions: &["djot"],
        mime_type: "text/x-djot",
        aliases: &["text/djot"],
    },
    FormatEntry {
        extensions: &["pdf"],
        mime_type: "application/pdf",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["html", "htm"],
        mime_type: "text/html",
        aliases: &["application/xhtml+xml"],
    },
    FormatEntry {
        extensions: &["docx"],
        mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        aliases: &["application/docx"],
    },
    FormatEntry {
        extensions: &["docm"],
        mime_type: "application/vnd.ms-word.document.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["dotx"],
        mime_type: "application/vnd.openxmlformats-officedocument.wordprocessingml.template",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["dotm"],
        mime_type: "application/vnd.ms-word.template.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["doc", "dot"],
        mime_type: "application/msword",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["odt"],
        mime_type: ODT_MIME_TYPE,
        aliases: &[],
    },
    FormatEntry {
        extensions: &["odp"],
        mime_type: ODP_MIME_TYPE,
        aliases: &[],
    },
    FormatEntry {
        extensions: &["pptx"],
        mime_type: "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["ppsx"],
        mime_type: "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["pptm"],
        mime_type: "application/vnd.ms-powerpoint.presentation.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["potx"],
        mime_type: "application/vnd.openxmlformats-officedocument.presentationml.template",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["potm"],
        mime_type: "application/vnd.ms-powerpoint.template.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["ppt", "pot"],
        mime_type: "application/vnd.ms-powerpoint",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xlsx"],
        mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xltx"],
        mime_type: "application/vnd.openxmlformats-officedocument.spreadsheetml.template",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xls", "xlt"],
        mime_type: "application/vnd.ms-excel",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xlsm"],
        mime_type: "application/vnd.ms-excel.sheet.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xlsb"],
        mime_type: "application/vnd.ms-excel.sheet.binary.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xlam"],
        mime_type: "application/vnd.ms-excel.addin.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["xla"],
        mime_type: "application/vnd.ms-excel.template.macroEnabled.12",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["ods"],
        mime_type: ODS_MIME_TYPE,
        aliases: &[],
    },
    FormatEntry {
        extensions: &["dbf"],
        mime_type: "application/x-dbf",
        aliases: &["application/dbase"],
    },
    FormatEntry {
        extensions: &["hwp"],
        mime_type: "application/x-hwp",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["hwpx"],
        mime_type: "application/haansofthwpx",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["bmp"],
        mime_type: "image/bmp",
        aliases: &["image/x-bmp", "image/x-ms-bmp"],
    },
    FormatEntry {
        extensions: &["gif"],
        mime_type: "image/gif",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["jpg", "jpeg"],
        mime_type: "image/jpeg",
        aliases: &["image/pjpeg", "image/jpg"],
    },
    FormatEntry {
        extensions: &["png"],
        mime_type: "image/png",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["tiff", "tif"],
        mime_type: "image/tiff",
        aliases: &["image/x-tiff"],
    },
    FormatEntry {
        extensions: &["webp"],
        mime_type: "image/webp",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["jp2", "j2k", "j2c"],
        mime_type: "image/jp2",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["jpx"],
        mime_type: "image/jpx",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["jpm"],
        mime_type: "image/jpm",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["mj2"],
        mime_type: "image/mj2",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["jbig2", "jb2"],
        mime_type: "image/x-jbig2",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["heic", "heics"],
        mime_type: "image/heic",
        aliases: &["image/heic-sequence"],
    },
    FormatEntry {
        extensions: &["heif"],
        mime_type: "image/heif",
        aliases: &["image/heif-sequence"],
    },
    FormatEntry {
        extensions: &["avif"],
        mime_type: "image/avif",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["avcs"],
        mime_type: "image/avcs",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["pnm"],
        mime_type: "image/x-portable-anymap",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["pbm"],
        mime_type: "image/x-portable-bitmap",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["pgm"],
        mime_type: "image/x-portable-graymap",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["ppm"],
        mime_type: "image/x-portable-pixmap",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["csv"],
        mime_type: "text/csv",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["tsv"],
        mime_type: "text/tab-separated-values",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["json"],
        mime_type: "application/json",
        aliases: &["text/json"],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "application/csl+json",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["jsonl", "ndjson"],
        mime_type: "application/x-ndjson",
        aliases: &["application/jsonl", "application/x-jsonlines"],
    },
    FormatEntry {
        extensions: &["yaml", "yml"],
        mime_type: "application/x-yaml",
        aliases: &["text/yaml", "text/x-yaml", "application/yaml"],
    },
    FormatEntry {
        extensions: &["toml"],
        mime_type: "application/toml",
        aliases: &["text/toml"],
    },
    FormatEntry {
        extensions: &["xml"],
        mime_type: "application/xml",
        aliases: &["text/xml"],
    },
    FormatEntry {
        extensions: &["svg"],
        mime_type: "image/svg+xml",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["eml"],
        mime_type: "message/rfc822",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["msg"],
        mime_type: "application/vnd.ms-outlook",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["pst"],
        mime_type: "application/vnd.ms-outlook-pst",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["zip"],
        mime_type: "application/zip",
        aliases: &["application/x-zip-compressed"],
    },
    FormatEntry {
        extensions: &["tar"],
        mime_type: "application/x-tar",
        aliases: &["application/tar", "application/x-gtar", "application/x-ustar"],
    },
    FormatEntry {
        extensions: &["gz", "tgz"],
        mime_type: "application/gzip",
        aliases: &["application/x-gzip"],
    },
    FormatEntry {
        extensions: &["7z"],
        mime_type: "application/x-7z-compressed",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["rst"],
        mime_type: "text/x-rst",
        aliases: &["text/prs.fallenstein.rst"],
    },
    FormatEntry {
        extensions: &["org"],
        mime_type: "text/x-org",
        aliases: &["text/org", "application/x-org"],
    },
    FormatEntry {
        extensions: &["epub"],
        mime_type: "application/epub+zip",
        aliases: &["application/x-epub+zip", "application/vnd.epub+zip"],
    },
    FormatEntry {
        extensions: &["rtf"],
        mime_type: "application/rtf",
        aliases: &["text/rtf"],
    },
    FormatEntry {
        extensions: &["bib"],
        mime_type: "application/x-bibtex",
        aliases: &["text/x-bibtex", "application/x-biblatex"],
    },
    FormatEntry {
        extensions: &["ris"],
        mime_type: "application/x-research-info-systems",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["nbib"],
        mime_type: "application/x-pubmed",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["enw"],
        mime_type: "application/x-endnote+xml",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["fb2"],
        mime_type: "application/x-fictionbook+xml",
        aliases: &["application/x-fictionbook", "text/x-fictionbook"],
    },
    FormatEntry {
        extensions: &["opml"],
        mime_type: "application/xml+opml",
        aliases: &["application/x-opml+xml", "text/x-opml"],
    },
    FormatEntry {
        extensions: &["dbk", "docbook", "docbook4", "docbook5"],
        mime_type: "application/docbook+xml",
        aliases: &["text/docbook"],
    },
    FormatEntry {
        extensions: &["jats"],
        mime_type: "application/x-jats+xml",
        aliases: &["text/jats"],
    },
    FormatEntry {
        extensions: &["ipynb"],
        mime_type: "application/x-ipynb+json",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["tex", "latex"],
        mime_type: "application/x-latex",
        aliases: &["text/x-tex"],
    },
    FormatEntry {
        extensions: &["typst", "typ"],
        mime_type: "application/x-typst",
        aliases: &["text/x-typst"],
    },
    FormatEntry {
        extensions: &["pages"],
        mime_type: "application/x-iwork-pages-sffpages",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["numbers"],
        mime_type: "application/x-iwork-numbers-sffnumbers",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["key"],
        mime_type: "application/x-iwork-keynote-sffkey",
        aliases: &[],
    },
    FormatEntry {
        extensions: &["mp3", "mpga"],
        mime_type: "audio/mpeg",
        aliases: &["audio/mp3"],
    },
    FormatEntry {
        extensions: &["m4a"],
        mime_type: "audio/mp4",
        aliases: &["audio/x-m4a"],
    },
    FormatEntry {
        extensions: &["wav"],
        mime_type: "audio/wav",
        aliases: &["audio/x-wav"],
    },
    FormatEntry {
        extensions: &["webm"],
        mime_type: "audio/webm",
        aliases: &["video/webm"],
    },
    FormatEntry {
        extensions: &["mp4", "mpeg"],
        mime_type: "video/mp4",
        aliases: &["video/mpeg"],
    },
    FormatEntry {
        extensions: &[],
        mime_type: "text/x-source-code",
        aliases: &[],
    },
];

/// Extension to MIME type mapping, derived from [`FORMATS`].
static EXT_TO_MIME: LazyLock<HashMap<&'static str, &'static str>> = LazyLock::new(|| {
    let mut m = HashMap::new();
    for entry in FORMATS {
        for ext in entry.extensions {
            m.insert(*ext, entry.mime_type);
        }
    }
    m
});

/// All supported MIME types (primary + aliases), derived from [`FORMATS`].
static SUPPORTED_MIME_TYPES: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    let mut set = HashSet::new();
    for entry in FORMATS {
        set.insert(entry.mime_type);
        for alias in entry.aliases {
            set.insert(*alias);
        }
    }
    set
});

/// Detect MIME type from a file path.
///
/// Uses file extension to determine MIME type. Falls back to `mime_guess` crate
/// if extension-based detection fails.
///
/// # Arguments
///
/// * `path` - Path to the file
/// * `check_exists` - Whether to verify file existence
///
/// # Returns
///
/// The detected MIME type string.
///
/// # Errors
///
/// Returns `XbergError::Io` if file doesn't exist (when `check_exists` is true).
/// Returns `XbergError::UnsupportedFormat` if MIME type cannot be determined.
pub fn detect_mime_type(path: impl AsRef<Path>, check_exists: bool) -> Result<String> {
    let path = path.as_ref();

    if check_exists && !path.exists() {
        return Err(XbergError::from(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("File does not exist: {}", path.display()),
        )));
    }

    let extension = path.extension().and_then(|ext| ext.to_str()).map(|s| s.to_lowercase());
    tracing::debug!(path = %path.display(), extension = ?extension, "detecting MIME type from path");

    if let Some(ext) = &extension
        && let Some(mime_type) = EXT_TO_MIME.get(ext.as_str())
    {
        tracing::debug!(ext = %ext, mime_type = %mime_type, "matched via EXT_TO_MIME");
        return Ok(mime_type.to_string());
    }

    #[cfg(feature = "tree-sitter")]
    {
        if let Some(ext) = &extension {
            let lang = tree_sitter_language_pack::detect_language_from_extension(ext);
            tracing::debug!(ext = %ext, detected_language = ?lang, "tree-sitter extension detection");
            if lang.is_some() {
                return Ok(SOURCE_CODE_MIME_TYPE.to_string());
            }
        }
    }

    let guess = mime_guess::from_path(path).first();
    tracing::debug!(guess = ?guess, "mime_guess fallback");
    if let Some(mime) = guess {
        return Ok(mime.to_string());
    }

    if let Some(ext) = extension {
        return Err(XbergError::UnsupportedFormat(format!("Unknown extension: .{}", ext)));
    }

    Err(XbergError::validation(format!(
        "Could not determine MIME type from file path: {}",
        path.display()
    )))
}

/// Validate that a MIME type is supported.
///
/// # Arguments
///
/// * `mime_type` - The MIME type to validate
///
/// # Returns
///
/// The validated MIME type (may be normalized).
///
/// # Errors
///
/// Returns `XbergError::UnsupportedFormat` if not supported.
#[cfg_attr(alef, alef(skip))]
pub fn validate_mime_type(mime_type: &str) -> Result<String> {
    if SUPPORTED_MIME_TYPES.contains(mime_type) {
        tracing::trace!(mime_type = %mime_type, "MIME type validated (exact match)");
        return Ok(mime_type.to_string());
    }

    if mime_type.starts_with("image/") {
        tracing::trace!(mime_type = %mime_type, "MIME type validated (image prefix)");
        return Ok(mime_type.to_string());
    }

    let lower = mime_type.to_ascii_lowercase();
    for supported in SUPPORTED_MIME_TYPES.iter() {
        if supported.to_ascii_lowercase() == lower {
            tracing::trace!(mime_type = %mime_type, matched = %supported, "MIME type validated (case-insensitive)");
            return Ok(supported.to_string());
        }
    }

    tracing::debug!(mime_type = %mime_type, "MIME type not in supported set");
    Err(XbergError::UnsupportedFormat(mime_type.to_string()))
}

/// Detect or validate MIME type.
///
/// If `mime_type` is provided, validates it. Otherwise, detects from `path`.
///
/// # Arguments
///
/// * `path` - Optional path to detect MIME type from
/// * `mime_type` - Optional explicit MIME type to validate
///
/// # Returns
///
/// The validated MIME type string.
pub(crate) fn detect_or_validate(path: Option<&str>, mime_type: Option<&str>) -> Result<String> {
    if let Some(mime) = mime_type {
        tracing::debug!(mime_type = %mime, "validating caller-provided MIME type");
        validate_mime_type(mime)
    } else if let Some(p) = path.map(Path::new) {
        let detected = detect_mime_type(p, true)?;
        let resolved = match magic_override(p, &detected) {
            Some(from_magic) => {
                tracing::debug!(path = %p.display(), extension_mime = %detected, magic_mime = %from_magic,
                    "extension/content MIME disagree; preferring content");
                from_magic
            }
            None => detected,
        };
        validate_mime_type(&resolved)
    } else {
        Err(XbergError::validation(
            "Must provide either path or mime_type".to_string(),
        ))
    }
}

/// If the file's magic bytes confidently indicate a different supported MIME
/// type than the extension did, return it. Returns `None` when the content has
/// no signature, the read fails, or content and extension agree.
fn magic_override(path: &Path, extension_mime: &str) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let mut header = vec![0u8; 4096];
    let n = file.read(&mut header).ok()?;
    header.truncate(n);
    if header.is_empty() {
        return None;
    }

    let from_magic = detect_mime_type_from_bytes(&header).ok()?;
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    if (from_magic == ZIP_MIME_TYPE || from_magic.starts_with("application/vnd.oasis.opendocument."))
        && let Some(odf_mime) = detect_odf_mime_from_zip(std::fs::File::open(path).ok()?)
    {
        return (odf_mime != extension_mime).then(|| odf_mime.to_string());
    }

    if from_magic == PLAIN_TEXT_MIME_TYPE {
        return None;
    }
    if from_magic != extension_mime && validate_mime_type(&from_magic).is_ok() {
        Some(from_magic)
    } else {
        None
    }
}

/// Detect MIME type from raw file bytes.
///
/// Uses magic byte signatures to detect file type from content.
/// Falls back to `infer` crate for comprehensive detection.
///
/// For ZIP-based files, inspects contents to distinguish Office Open XML
/// formats (DOCX, XLSX, PPTX) from plain ZIP archives.
///
/// # Arguments
///
/// * `content` - Raw file bytes
///
/// # Returns
///
/// The detected MIME type string.
///
/// # Errors
///
/// Returns `XbergError::UnsupportedFormat` if MIME type cannot be determined.
pub fn detect_mime_type_from_bytes(content: &[u8]) -> Result<String> {
    if let Some(kind) = infer::get(content) {
        let mime_type = kind.mime_type();

        #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
        if mime_type.starts_with("application/vnd.oasis.opendocument.") {
            return Ok(detect_odf_mime_from_zip(std::io::Cursor::new(content))
                .unwrap_or(ZIP_MIME_TYPE)
                .to_string());
        }

        if mime_type == "application/zip"
            && let Some(office_mime) = detect_office_format_from_zip(content)
        {
            return Ok(office_mime.to_string());
        }

        if SUPPORTED_MIME_TYPES.contains(mime_type) || mime_type.starts_with("image/") {
            return Ok(mime_type.to_string());
        }
    }

    if content.len() >= 4 && content[..4] == [0x21, 0x42, 0x44, 0x4E] {
        return Ok(PST_MIME_TYPE.to_string());
    }

    if let Ok(text) = std::str::from_utf8(content) {
        let trimmed = text.trim_start();

        if (trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<serde_json::Value>(text).is_ok()
        {
            return Ok(JSON_MIME_TYPE.to_string());
        }

        if trimmed.starts_with("<?xml") || trimmed.starts_with('<') {
            return Ok(XML_MIME_TYPE.to_string());
        }

        if trimmed.starts_with("<!DOCTYPE html") || trimmed.starts_with("<html") {
            return Ok(HTML_MIME_TYPE.to_string());
        }

        if trimmed.starts_with("%PDF") {
            return Ok(PDF_MIME_TYPE.to_string());
        }

        #[cfg(feature = "tree-sitter")]
        if tree_sitter_language_pack::detect_language_from_content(trimmed).is_some() {
            return Ok(SOURCE_CODE_MIME_TYPE.to_string());
        }

        return Ok(PLAIN_TEXT_MIME_TYPE.to_string());
    }

    Err(XbergError::UnsupportedFormat(
        "Could not determine MIME type from bytes".to_string(),
    ))
}

/// Detect Office Open XML format from ZIP content by scanning for marker files.
///
/// Office Open XML formats (DOCX, XLSX, PPTX) are ZIP archives containing specific
/// XML files that identify the format:
/// - DOCX: contains `word/document.xml`
/// - XLSX: contains `xl/workbook.xml`
/// - PPTX: contains `ppt/presentation.xml`
///
/// Apple iWork formats (2013+) also use ZIP with IWA files:
/// - Pages: contains `Index/Document.iwa`
/// - Numbers: contains `Index/CalculationEngine.iwa`
/// - Keynote: contains `Index/Presentation.iwa`
///
/// This function scans the ZIP's local file headers without fully parsing the archive,
/// making it efficient for MIME type detection.
fn detect_office_format_from_zip(content: &[u8]) -> Option<&'static str> {
    const DOCX_MARKER: &[u8] = b"word/document.xml";
    const XLSX_MARKER: &[u8] = b"xl/workbook.xml";
    const PPTX_MARKER: &[u8] = b"ppt/presentation.xml";
    const PAGES_MARKER: &[u8] = b"Index/Document.iwa";
    const NUMBERS_MARKER: &[u8] = b"Index/CalculationEngine.iwa";
    const KEYNOTE_MARKER: &[u8] = b"Index/Presentation.iwa";

    const HWPX_MARKER: &[u8] = b"Contents/content.hpf";
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    if let Some(odf_mime) = detect_odf_mime_from_zip(std::io::Cursor::new(content)) {
        return Some(odf_mime);
    }

    if contains_subsequence(content, HWPX_MARKER) {
        return Some(HWPX_MIME_TYPE);
    }

    if contains_subsequence(content, PAGES_MARKER) {
        return Some(IWORK_PAGES_MIME_TYPE);
    }
    if contains_subsequence(content, NUMBERS_MARKER) {
        return Some(IWORK_NUMBERS_MIME_TYPE);
    }
    if contains_subsequence(content, KEYNOTE_MARKER) {
        return Some(IWORK_KEYNOTE_MIME_TYPE);
    }

    if contains_subsequence(content, DOCX_MARKER) {
        return Some(DOCX_MIME_TYPE);
    }
    if contains_subsequence(content, XLSX_MARKER) {
        return Some(EXCEL_MIME_TYPE);
    }
    if contains_subsequence(content, PPTX_MARKER) {
        return Some(POWER_POINT_MIME_TYPE);
    }
    None
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn detect_odf_mime_from_zip<R: Read + Seek>(mut reader: R) -> Option<&'static str> {
    const MAX_MIMETYPE_LENGTH: u64 = ODP_MIME_TYPE.len() as u64;

    let limits = SecurityLimits::default();
    if !zip_central_directory_within_limits(&mut reader, &limits) {
        return None;
    }
    reader.seek(SeekFrom::Start(0)).ok()?;

    let mut archive = zip::ZipArchive::new(reader).ok()?;

    let mut mimetype_index = None;
    for index in 0..archive.len() {
        if archive.by_index(index).ok()?.name() == "mimetype" && mimetype_index.replace(index).is_some() {
            return None;
        }
    }

    let mimetype = archive.by_index(mimetype_index?).ok()?;
    if mimetype.size() > MAX_MIMETYPE_LENGTH {
        return None;
    }

    let mut value = Vec::with_capacity(mimetype.size() as usize);
    mimetype.take(MAX_MIMETYPE_LENGTH + 1).read_to_end(&mut value).ok()?;
    match value.as_slice() {
        value if value == ODT_MIME_TYPE.as_bytes() => Some(ODT_MIME_TYPE),
        value if value == ODP_MIME_TYPE.as_bytes() => Some(ODP_MIME_TYPE),
        value if value == ODS_MIME_TYPE.as_bytes() => Some(ODS_MIME_TYPE),
        _ => None,
    }
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
struct ZipCentralDirectory {
    offset: u64,
    size: usize,
    entries: u16,
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn read_zip_central_directory<R: Read + Seek>(reader: &mut R, limits: &SecurityLimits) -> Option<ZipCentralDirectory> {
    const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
    const EOCD_MIN_LENGTH: u64 = 22;
    const MAX_ZIP_COMMENT_LENGTH: u64 = u16::MAX as u64;

    let archive_length = reader.seek(SeekFrom::End(0)).ok()?;
    if archive_length < EOCD_MIN_LENGTH || archive_length > limits.max_archive_size as u64 {
        return None;
    }

    let tail_length = archive_length.min(EOCD_MIN_LENGTH + MAX_ZIP_COMMENT_LENGTH);
    reader.seek(SeekFrom::End(-(tail_length as i64))).ok()?;
    let mut tail = vec![0; tail_length as usize];
    reader.read_exact(&mut tail).ok()?;

    let eocd_offset = tail
        .windows(EOCD_SIGNATURE.len())
        .rposition(|window| window == EOCD_SIGNATURE)?;
    let eocd = &tail[eocd_offset..];
    if eocd.len() < EOCD_MIN_LENGTH as usize {
        return None;
    }

    let disk_number = u16::from_le_bytes([eocd[4], eocd[5]]);
    let central_directory_disk = u16::from_le_bytes([eocd[6], eocd[7]]);
    let entries_on_disk = u16::from_le_bytes([eocd[8], eocd[9]]);
    let entries = u16::from_le_bytes([eocd[10], eocd[11]]);
    let size = u32::from_le_bytes([eocd[12], eocd[13], eocd[14], eocd[15]]) as usize;
    let offset = u32::from_le_bytes([eocd[16], eocd[17], eocd[18], eocd[19]]) as u64;
    let comment_length = u16::from_le_bytes([eocd[20], eocd[21]]) as usize;
    let is_valid = eocd.len() == EOCD_MIN_LENGTH as usize + comment_length
        && disk_number == 0
        && central_directory_disk == 0
        && entries_on_disk == entries
        && entries != u16::MAX
        && entries as usize <= limits.max_files_in_archive
        && size <= limits.max_content_size
        && offset.checked_add(size as u64).is_some_and(|end| end <= archive_length);
    is_valid.then_some(ZipCentralDirectory { offset, size, entries })
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn read_central_directory_entry<R: Read + Seek>(reader: &mut R) -> Option<(Vec<u8>, usize)> {
    const HEADER_SIGNATURE: &[u8; 4] = b"PK\x01\x02";
    const HEADER_LENGTH: usize = 46;

    let mut header = [0; HEADER_LENGTH];
    reader.read_exact(&mut header).ok()?;
    (&header[..4] == HEADER_SIGNATURE).then_some(())?;

    let name_length = u16::from_le_bytes([header[28], header[29]]) as usize;
    let extra_length = u16::from_le_bytes([header[30], header[31]]) as usize;
    let comment_length = u16::from_le_bytes([header[32], header[33]]) as usize;
    let entry_length = HEADER_LENGTH
        .checked_add(name_length)?
        .checked_add(extra_length)?
        .checked_add(comment_length)?;

    let mut name = vec![0; name_length];
    reader.read_exact(&mut name).ok()?;
    reader
        .seek(SeekFrom::Current((extra_length + comment_length) as i64))
        .ok()?;
    Some((name, entry_length))
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn central_directory_has_unique_mimetype<R: Read + Seek>(reader: &mut R, directory: &ZipCentralDirectory) -> bool {
    if reader.seek(SeekFrom::Start(directory.offset)).is_err() {
        return false;
    }

    let mut bytes_read = 0usize;
    let mut mimetype_entries = 0usize;
    for _ in 0..directory.entries {
        let Some((name, entry_length)) = read_central_directory_entry(reader) else {
            return false;
        };
        let Some(next_bytes_read) = bytes_read.checked_add(entry_length) else {
            return false;
        };
        if next_bytes_read > directory.size {
            return false;
        }
        if name == b"mimetype" {
            mimetype_entries += 1;
            if mimetype_entries > 1 {
                return false;
            }
        }
        bytes_read = next_bytes_read;
    }

    true
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn zip_central_directory_within_limits<R: Read + Seek>(reader: &mut R, limits: &SecurityLimits) -> bool {
    read_zip_central_directory(reader, limits)
        .is_some_and(|directory| central_directory_has_unique_mimetype(reader, &directory))
}

/// Check if `haystack` contains `needle` as a subsequence.
#[inline]
fn contains_subsequence(haystack: &[u8], needle: &[u8]) -> bool {
    memchr::memmem::find(haystack, needle).is_some()
}

/// Get file extensions for a given MIME type.
///
/// Returns all known file extensions that map to the specified MIME type.
///
/// # Arguments
///
/// * `mime_type` - The MIME type to look up
///
/// # Returns
///
/// A vector of file extensions (without leading dot) for the MIME type.
///
/// # Example
///
/// ```
/// use xberg::core::mime::get_extensions_for_mime;
///
/// let extensions = get_extensions_for_mime("application/pdf").unwrap();
/// assert_eq!(extensions, vec!["pdf"]);
///
/// let doc_extensions = get_extensions_for_mime("application/vnd.openxmlformats-officedocument.wordprocessingml.document").unwrap();
/// assert!(doc_extensions.contains(&"docx".to_string()));
/// ```
pub fn get_extensions_for_mime(mime_type: &str) -> Result<Vec<String>> {
    let mut extensions = Vec::new();

    for (ext, mime) in EXT_TO_MIME.iter() {
        if *mime == mime_type {
            extensions.push(ext.to_string());
        }
    }

    if !extensions.is_empty() {
        return Ok(extensions);
    }

    let guessed = mime_guess::get_mime_extensions_str(mime_type);
    if let Some(exts) = guessed {
        return Ok(exts.iter().map(|s| s.to_string()).collect());
    }

    Err(XbergError::UnsupportedFormat(format!(
        "No known extensions for MIME type: {}",
        mime_type
    )))
}

/// List all supported document formats.
///
/// Returns every file extension Xberg recognizes together with its
/// corresponding MIME type, derived from the central format registry.
/// Formats that have no registered file extension (such as source code,
/// which is detected dynamically) are not included.
///
/// The list is sorted alphabetically by file extension.
///
/// # Returns
///
/// A vector of [`SupportedFormat`] entries sorted by extension.
///
/// # Example
///
/// ```
/// use xberg::core::mime::list_supported_formats;
///
/// let formats = list_supported_formats();
/// assert!(!formats.is_empty());
/// assert!(formats.iter().any(|f| f.extension == "pdf"));
/// ```
pub fn list_supported_formats() -> Vec<SupportedFormat> {
    let mut formats: Vec<SupportedFormat> = EXT_TO_MIME
        .iter()
        .map(|(ext, mime)| SupportedFormat {
            extension: ext.to_string(),
            mime_type: mime.to_string(),
        })
        .collect();
    formats.sort_by(|a, b| a.extension.cmp(&b.extension));
    formats
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    use std::io::{Cursor, Write};
    use tempfile::tempdir;
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    use zip::write::FileOptions;

    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    fn build_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = FileOptions::<'_, ()>::default().compression_method(zip::CompressionMethod::Stored);
        for (name, content) in entries {
            archive.start_file(*name, options).unwrap();
            archive.write_all(content).unwrap();
        }
        archive.finish().unwrap().into_inner()
    }

    #[test]
    fn test_detect_mime_type_pdf() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.pdf");
        File::create(&file_path).unwrap();

        let mime = detect_mime_type(&file_path, true).unwrap();
        assert_eq!(mime, "application/pdf");
    }

    #[test]
    fn test_detect_mime_type_images() {
        let dir = tempdir().unwrap();

        let test_cases = vec![
            ("test.png", "image/png"),
            ("test.jpg", "image/jpeg"),
            ("test.jpeg", "image/jpeg"),
            ("test.gif", "image/gif"),
            ("test.bmp", "image/bmp"),
            ("test.webp", "image/webp"),
            ("test.tiff", "image/tiff"),
        ];

        for (filename, expected_mime) in test_cases {
            let file_path = dir.path().join(filename);
            File::create(&file_path).unwrap();
            let mime = detect_mime_type(&file_path, true).unwrap();
            assert_eq!(mime, expected_mime, "Failed for {}", filename);
        }
    }

    #[test]
    fn test_detect_mime_type_office() {
        let dir = tempdir().unwrap();

        let test_cases = vec![
            ("test.xlsx", EXCEL_MIME_TYPE),
            ("test.xls", "application/vnd.ms-excel"),
            ("test.pptx", POWER_POINT_MIME_TYPE),
            (
                "test.ppsx",
                "application/vnd.openxmlformats-officedocument.presentationml.slideshow",
            ),
            (
                "test.pptm",
                "application/vnd.ms-powerpoint.presentation.macroEnabled.12",
            ),
            ("test.ppt", LEGACY_POWERPOINT_MIME_TYPE),
            ("test.docx", DOCX_MIME_TYPE),
            ("test.doc", LEGACY_WORD_MIME_TYPE),
        ];

        for (filename, expected_mime) in test_cases {
            let file_path = dir.path().join(filename);
            File::create(&file_path).unwrap();
            let mime = detect_mime_type(&file_path, true).unwrap();
            assert_eq!(mime, expected_mime, "Failed for {}", filename);
        }
    }

    #[test]
    fn test_detect_mime_type_data_formats() {
        let dir = tempdir().unwrap();

        let test_cases = vec![
            ("test.json", JSON_MIME_TYPE),
            ("test.yaml", "application/x-yaml"),
            ("test.toml", "application/toml"),
            ("test.xml", XML_MIME_TYPE),
            ("test.csv", "text/csv"),
        ];

        for (filename, expected_mime) in test_cases {
            let file_path = dir.path().join(filename);
            File::create(&file_path).unwrap();
            let mime = detect_mime_type(&file_path, true).unwrap();
            assert_eq!(mime, expected_mime, "Failed for {}", filename);
        }
    }

    #[test]
    fn test_detect_mime_type_text_formats() {
        let dir = tempdir().unwrap();

        let test_cases = vec![
            ("test.txt", PLAIN_TEXT_MIME_TYPE),
            ("test.md", "text/markdown"),
            ("test.qmd", "text/x-quarto"),
            ("test.Rmd", "text/x-r-markdown"),
            ("test.rmd", "text/x-r-markdown"),
            ("test.html", HTML_MIME_TYPE),
            ("test.htm", HTML_MIME_TYPE),
        ];

        for (filename, expected_mime) in test_cases {
            let file_path = dir.path().join(filename);
            File::create(&file_path).unwrap();
            let mime = detect_mime_type(&file_path, true).unwrap();
            assert_eq!(mime, expected_mime, "Failed for {}", filename);
        }
    }

    #[test]
    fn test_detect_mime_type_email() {
        let dir = tempdir().unwrap();

        let test_cases = vec![
            ("test.eml", "message/rfc822"),
            ("test.msg", "application/vnd.ms-outlook"),
            ("test.pst", PST_MIME_TYPE),
        ];

        for (filename, expected_mime) in test_cases {
            let file_path = dir.path().join(filename);
            File::create(&file_path).unwrap();
            let mime = detect_mime_type(&file_path, true).unwrap();
            assert_eq!(mime, expected_mime, "Failed for {}", filename);
        }
    }

    #[test]
    fn test_validate_mime_type_exact() {
        assert!(validate_mime_type("application/pdf").is_ok());
        assert!(validate_mime_type("text/plain").is_ok());
        assert!(validate_mime_type("text/html").is_ok());
    }

    #[test]
    fn test_validate_mime_type_images() {
        assert!(validate_mime_type("image/jpeg").is_ok());
        assert!(validate_mime_type("image/png").is_ok());
        assert!(validate_mime_type("image/gif").is_ok());
        assert!(validate_mime_type("image/webp").is_ok());

        assert!(validate_mime_type("image/custom-format").is_ok());
    }

    #[test]
    fn test_validate_mime_type_unsupported() {
        assert!(validate_mime_type("application/unknown").is_err());
    }

    #[test]
    fn test_validate_mime_type_audio_video() {
        assert!(validate_mime_type("audio/mpeg").is_ok());
        assert!(validate_mime_type("audio/mp4").is_ok());
        assert!(validate_mime_type("audio/wav").is_ok());
        assert!(validate_mime_type("audio/webm").is_ok());
        assert!(validate_mime_type("video/mp4").is_ok());
        assert!(validate_mime_type("video/webm").is_ok());
    }

    #[test]
    fn test_file_not_exists() {
        let result = detect_mime_type("/nonexistent/file.pdf", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_file_no_extension() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("testfile");
        File::create(&file_path).unwrap();

        let _result = detect_mime_type(&file_path, true);
    }

    #[test]
    fn test_detect_or_validate_with_mime() {
        let result = detect_or_validate(None, Some("application/pdf"));
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "application/pdf");
    }

    #[test]
    fn test_detect_or_validate_with_path() {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join("test.pdf");
        File::create(&file_path).unwrap();

        let result = detect_or_validate(file_path.to_str(), None);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "application/pdf");
    }

    /// Regression for #1223: a file whose content is a DOCX but whose extension
    /// says .pdf must route by content, matching the bytes entry point — the
    /// path detector previously trusted the extension and picked the PDF
    /// extractor.
    #[test]
    fn misnamed_file_routes_by_content_not_extension() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/office/merged_cells.docx");
        let Ok(docx_bytes) = std::fs::read(&fixture) else {
            eprintln!("skipping: fixture not present at {fixture:?}");
            return;
        };
        let dir = tempdir().unwrap();
        let misnamed = dir.path().join("report.pdf");
        std::fs::write(&misnamed, &docx_bytes).unwrap();

        let detected = detect_or_validate(misnamed.to_str(), None).unwrap();
        assert_eq!(
            detected, "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "DOCX content named .pdf must detect as DOCX, not PDF"
        );
    }

    #[test]
    fn test_detect_or_validate_neither() {
        let result = detect_or_validate(None, None);
        assert!(result.is_err());
    }

    #[test]
    fn test_case_insensitive_extensions() {
        let dir = tempdir().unwrap();

        let file_path = dir.path().join("test.PDF");
        File::create(&file_path).unwrap();
        let mime = detect_mime_type(&file_path, true).unwrap();
        assert_eq!(mime, "application/pdf");

        let file_path2 = dir.path().join("test.XLSX");
        File::create(&file_path2).unwrap();
        let mime2 = detect_mime_type(&file_path2, true).unwrap();
        assert_eq!(mime2, EXCEL_MIME_TYPE);
    }

    #[test]
    fn test_detect_office_format_from_zip_bytes() {
        let docx_bytes: &[u8] = &[
            0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x11, 0x00, 0x00, 0x00, b'w', b'o', b'r', b'd', b'/', b'd',
            b'o', b'c', b'u', b'm', b'e', b'n', b't', b'.', b'x', b'm', b'l',
        ];
        let mime = detect_mime_type_from_bytes(docx_bytes).unwrap();
        assert_eq!(
            mime, DOCX_MIME_TYPE,
            "Should detect DOCX from ZIP with word/document.xml"
        );

        let xlsx_bytes: &[u8] = &[
            0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x0f, 0x00, 0x00, 0x00, b'x', b'l', b'/', b'w', b'o', b'r',
            b'k', b'b', b'o', b'o', b'k', b'.', b'x', b'm', b'l',
        ];
        let mime = detect_mime_type_from_bytes(xlsx_bytes).unwrap();
        assert_eq!(
            mime, EXCEL_MIME_TYPE,
            "Should detect XLSX from ZIP with xl/workbook.xml"
        );

        let pptx_bytes: &[u8] = &[
            0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x14, 0x00, 0x00, 0x00, b'p', b'p', b't', b'/', b'p', b'r',
            b'e', b's', b'e', b'n', b't', b'a', b't', b'i', b'o', b'n', b'.', b'x', b'm', b'l',
        ];
        let mime = detect_mime_type_from_bytes(pptx_bytes).unwrap();
        assert_eq!(
            mime, POWER_POINT_MIME_TYPE,
            "Should detect PPTX from ZIP with ppt/presentation.xml"
        );

        #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
        {
            for expected_mime in [ODT_MIME_TYPE, ODP_MIME_TYPE, ODS_MIME_TYPE] {
                let open_document_bytes = build_zip(&[("mimetype", expected_mime.as_bytes())]);
                let mime = detect_mime_type_from_bytes(&open_document_bytes).unwrap();
                assert_eq!(mime, expected_mime, "Should detect exact OpenDocument mimetype entry");
            }
        }

        let plain_zip_bytes: &[u8] = &[
            0x50, 0x4b, 0x03, 0x04, 0x14, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
            0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x08, 0x00, 0x00, 0x00, b't', b'e', b's', b't', b'.', b't',
            b'x', b't',
        ];
        let mime = detect_mime_type_from_bytes(plain_zip_bytes).unwrap();
        assert_eq!(mime, "application/zip", "Plain ZIP should remain as application/zip");
    }

    #[test]
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    fn reordered_open_document_mimetype_routes_by_exact_entry() {
        const PADDING: &[u8] = &[b'x'; 5_000];
        let dir = tempdir().unwrap();

        for (extension, expected_mime) in [("odt", ODT_MIME_TYPE), ("odp", ODP_MIME_TYPE), ("ods", ODS_MIME_TYPE)] {
            let bytes = build_zip(&[("padding.bin", PADDING), ("mimetype", expected_mime.as_bytes())]);
            assert_eq!(detect_mime_type_from_bytes(&bytes).unwrap(), expected_mime);

            let path = dir.path().join(format!("reordered.{extension}"));
            std::fs::write(&path, bytes).unwrap();
            assert_eq!(detect_or_validate(path.to_str(), None).unwrap(), expected_mime);
        }
    }

    #[test]
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    fn odf_detection_rejects_decoys_and_invalid_mimetype_entries() {
        let generic_zip = build_zip(&[("decoy.txt", ODT_MIME_TYPE.as_bytes())]);
        assert_eq!(detect_mime_type_from_bytes(&generic_zip).unwrap(), ZIP_MIME_TYPE);

        let epub = build_zip(&[("mimetype", b"application/epub+zip")]);
        assert_eq!(detect_mime_type_from_bytes(&epub).unwrap(), "application/epub+zip");

        let mixed = build_zip(&[
            ("mimetype", ODT_MIME_TYPE.as_bytes()),
            ("decoy.txt", ODS_MIME_TYPE.as_bytes()),
        ]);
        assert_eq!(detect_mime_type_from_bytes(&mixed).unwrap(), ODT_MIME_TYPE);

        let oversized = build_zip(&[("mimetype", b"application/vnd.oasis.opendocument.text-extra")]);
        assert_eq!(detect_mime_type_from_bytes(&oversized).unwrap(), ZIP_MIME_TYPE);

        let mut duplicate = build_zip(&[
            ("mimetypa", ODT_MIME_TYPE.as_bytes()),
            ("mimetypb", ODP_MIME_TYPE.as_bytes()),
        ]);
        for offset in 0..duplicate.len().saturating_sub(b"mimetypa".len()) {
            let name = &duplicate[offset..offset + b"mimetypa".len()];
            if name == b"mimetypa" || name == b"mimetypb" {
                duplicate[offset..offset + b"mimetype".len()].copy_from_slice(b"mimetype");
            }
        }
        assert_eq!(detect_mime_type_from_bytes(&duplicate).unwrap(), ZIP_MIME_TYPE);

        let truncated = &mixed[..mixed.len() / 2];
        assert_eq!(detect_mime_type_from_bytes(truncated).unwrap(), ZIP_MIME_TYPE);
    }

    #[test]
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    fn odf_zip_preflight_rejects_excessive_entry_count() {
        let archive = build_zip(&[("content.txt", b"content")]);
        let default_limits = SecurityLimits::default();
        assert!(zip_central_directory_within_limits(
            &mut Cursor::new(&archive),
            &default_limits
        ));

        let restricted_limits = SecurityLimits {
            max_files_in_archive: 0,
            ..default_limits
        };
        assert!(!zip_central_directory_within_limits(
            &mut Cursor::new(archive),
            &restricted_limits
        ));
    }

    #[test]
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    fn odf_extension_does_not_override_generic_zip_content() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("not-an-open-document.odt");
        std::fs::write(&path, build_zip(&[("content.txt", b"plain archive")])).unwrap();

        assert_eq!(detect_or_validate(path.to_str(), None).unwrap(), ZIP_MIME_TYPE);
    }

    #[test]
    fn test_detect_pst_from_bytes() {
        let pst_bytes: &[u8] = &[0x21, 0x42, 0x44, 0x4E, 0x00, 0x00, 0x00, 0x00];
        let mime = detect_mime_type_from_bytes(pst_bytes).unwrap();
        assert_eq!(mime, PST_MIME_TYPE, "Should detect PST from magic bytes");
    }

    #[test]
    fn test_list_supported_formats_not_empty() {
        let formats = list_supported_formats();
        assert!(!formats.is_empty(), "Supported formats list should not be empty");
    }

    #[test]
    fn test_list_supported_formats_sorted() {
        let formats = list_supported_formats();
        let extensions: Vec<&str> = formats.iter().map(|f| f.extension.as_str()).collect();
        let mut sorted = extensions.clone();
        sorted.sort();
        assert_eq!(extensions, sorted, "Formats should be sorted by extension");
    }

    #[test]
    fn test_list_supported_formats_includes_common_formats() {
        let formats = list_supported_formats();
        let extensions: Vec<&str> = formats.iter().map(|f| f.extension.as_str()).collect();

        assert!(extensions.contains(&"pdf"), "Should include pdf");
        assert!(extensions.contains(&"md"), "Should include md");
        assert!(extensions.contains(&"docx"), "Should include docx");
        assert!(extensions.contains(&"html"), "Should include html");
        assert!(extensions.contains(&"txt"), "Should include txt");
        assert!(extensions.contains(&"csv"), "Should include csv");
        assert!(extensions.contains(&"json"), "Should include json");
        assert!(extensions.contains(&"xlsx"), "Should include xlsx");
    }

    #[test]
    fn test_list_supported_formats_has_valid_mime_types() {
        let formats = list_supported_formats();
        for format in &formats {
            assert!(!format.extension.is_empty(), "Extension should not be empty");
            assert!(!format.mime_type.is_empty(), "MIME type should not be empty");
            assert!(format.mime_type.contains('/'), "MIME type should contain '/'");
        }
    }

    #[test]
    fn test_formats_registry_consistency() {
        for (ext, mime) in EXT_TO_MIME.iter() {
            assert!(
                SUPPORTED_MIME_TYPES.contains(mime),
                "Extension '{}' maps to MIME '{}' which is not in SUPPORTED_MIME_TYPES",
                ext,
                mime
            );
        }
    }

    #[test]
    fn test_formats_registry_mdx() {
        assert_eq!(EXT_TO_MIME.get("mdx"), Some(&"text/mdx"));
        assert!(SUPPORTED_MIME_TYPES.contains("text/mdx"));
        assert!(SUPPORTED_MIME_TYPES.contains("text/x-mdx"));
    }

    #[test]
    fn test_formats_registry_aliases() {
        assert!(
            SUPPORTED_MIME_TYPES.contains("text/x-markdown"),
            "text/x-markdown alias"
        );
        assert!(SUPPORTED_MIME_TYPES.contains("text/json"), "text/json alias");
        assert!(SUPPORTED_MIME_TYPES.contains("text/yaml"), "text/yaml alias");
        assert!(SUPPORTED_MIME_TYPES.contains("text/xml"), "text/xml alias");
        assert!(SUPPORTED_MIME_TYPES.contains("application/xhtml+xml"), "xhtml alias");
        assert!(SUPPORTED_MIME_TYPES.contains("image/pjpeg"), "pjpeg alias");
        assert!(SUPPORTED_MIME_TYPES.contains("image/x-bmp"), "x-bmp alias");
        assert!(
            SUPPORTED_MIME_TYPES.contains("application/x-zip-compressed"),
            "zip alias"
        );
        assert!(SUPPORTED_MIME_TYPES.contains("text/rtf"), "rtf alias");
        assert!(SUPPORTED_MIME_TYPES.contains("text/x-typst"), "typst alias");
    }
}
