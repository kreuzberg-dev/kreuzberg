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

/// Element names that identify a markup fragment as HTML rather than generic XML.
///
/// Deliberately conservative: every entry is an element that exists in HTML and is not a
/// plausible root for the XML vocabularies this crate also extracts (DocBook, JATS, FB2,
/// OPML, RSS). Ambiguous names shared with those vocabularies — `title`, `table`, `para`,
/// `section`, `article`, `link`, `code` — are omitted on purpose, so a borderline document
/// keeps its current XML routing instead of being silently rerouted.
const HTML_FRAGMENT_ELEMENTS: &[&str] = &[
    "a",
    "b",
    "blockquote",
    "body",
    "br",
    "button",
    "div",
    "em",
    "figcaption",
    "figure",
    "footer",
    "form",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "head",
    "header",
    "hr",
    "i",
    "iframe",
    "img",
    "input",
    "label",
    "li",
    "main",
    "meta",
    "nav",
    "ol",
    "option",
    "p",
    "pre",
    "script",
    "select",
    "span",
    "strong",
    "style",
    "table",
    "tbody",
    "td",
    "textarea",
    "tfoot",
    "th",
    "thead",
    "tr",
    "ul",
];

/// Return `true` when `trimmed` opens as HTML rather than as generic XML.
///
/// Recognises the two document preambles case-insensitively (`<!doctype html>` is at least
/// as common in the wild as the uppercase spelling) and, for fragments that carry no
/// preamble at all, the name of the first element.
fn looks_like_html(trimmed: &str) -> bool {
    let lowered_prefix: String = trimmed.chars().take(16).flat_map(char::to_lowercase).collect();
    if lowered_prefix.starts_with("<!doctype html") || lowered_prefix.starts_with("<html") {
        return true;
    }

    let Some(after_bracket) = trimmed.strip_prefix('<') else {
        return false;
    };
    let name_length = after_bracket
        .find(|character: char| !character.is_ascii_alphanumeric())
        .unwrap_or(after_bracket.len());
    let (name, rest) = after_bracket.split_at(name_length);
    // Require the tag to actually close here, so `<tr:foo>` (a namespace prefix that happens
    // to collide with an HTML name) stays XML.
    if !matches!(
        rest.chars().next(),
        Some('>') | Some(' ') | Some('/') | Some('\t') | Some('\n') | Some('\r')
    ) {
        return false;
    }

    let name = name.to_ascii_lowercase();
    HTML_FRAGMENT_ELEMENTS.contains(&name.as_str())
}
pub(crate) const DOCBOOK_MIME_TYPE: &str = "application/docbook+xml";
pub(crate) const JATS_MIME_TYPE: &str = "application/x-jats+xml";

/// Return the XML vocabulary that `trimmed` declares, if it declares one.
///
/// Real DocBook and JATS documents use the `.xml` extension, so the extension
/// map alone routes them to the generic XML extractor and their structure and
/// equations are lost.
///
/// The test is structural, not a search of the text. A public identifier counts
/// only inside the DOCTYPE declaration, and a namespace counts only when the
/// root element declares it. A stylesheet, a schema or a catalog that merely
/// names DocBook keeps its generic XML routing.
fn xml_vocabulary(trimmed: &str) -> Option<&'static str> {
    let doctype = declaration_of(trimmed, "<!DOCTYPE");
    if let Some(doctype) = doctype {
        if doctype.contains("//OASIS//DTD DocBook") {
            return Some(DOCBOOK_MIME_TYPE);
        }
        if doctype.contains("//NLM//DTD JATS") || doctype.contains("//NLM//DTD Journal") {
            return Some(JATS_MIME_TYPE);
        }
    }
    let root = root_start_tag(trimmed)?;
    root_is_in_namespace(root, "http://docbook.org/ns/docbook").then_some(DOCBOOK_MIME_TYPE)
}

/// Report whether the root element itself belongs to `namespace`.
///
/// A declaration alone proves nothing: an XSL stylesheet that transforms
/// DocBook binds the namespace on its own root. The element belongs to the
/// namespace only when the binding it carries is the one its name uses.
fn root_is_in_namespace(root: &str, namespace: &str) -> bool {
    let name = root
        .trim_start_matches('<')
        .split([' ', '\t', '\n', '\r', '>', '/'])
        .next()
        .unwrap_or_default();
    let binding = match name.split_once(':') {
        Some((prefix, _)) => format!("xmlns:{prefix}="),
        None => "xmlns=".to_string(),
    };
    let Some(start) = root.find(&binding) else {
        return false;
    };
    let value = &root[start + binding.len()..];
    let Some(quote) = value.chars().next().filter(|c| *c == '"' || *c == '\'') else {
        return false;
    };
    value[1..].split(quote).next().is_some_and(|uri| uri == namespace)
}

/// Return the text of the first declaration that opens with `opener`.
///
/// The declaration ends at its own `>`. An internal subset may hold a `>`
/// inside brackets, so the scan tracks bracket depth rather than searching for
/// a `]` anywhere in the document: a `]` in the body would otherwise stretch
/// the declaration over the whole file.
fn declaration_of<'a>(trimmed: &'a str, opener: &str) -> Option<&'a str> {
    let start = trimmed.find(opener)?;
    let rest = &trimmed[start..];
    let tail = &rest[opener.len()..];
    let end = crate::utils::xml_utils::doctype_end(tail)?;
    Some(&rest[..opener.len() + end])
}

/// Return the start tag of the root element, skipping the prolog.
///
/// The scan stops at the first element, so a namespace bound deeper in the
/// document cannot claim the file.
fn root_start_tag(trimmed: &str) -> Option<&str> {
    let mut rest = trimmed;
    loop {
        let open = rest.find('<')?;
        rest = &rest[open..];
        if rest.starts_with("<?") || rest.starts_with("<!") {
            let skip = if rest.starts_with("<!DOCTYPE") {
                declaration_of(rest, "<!DOCTYPE").map(str::len)?
            } else {
                rest.find('>')?
            };
            debug_assert!(skip < rest.len(), "a declaration ends inside the input");
            rest = &rest[skip + 1..];
            continue;
        }
        let end = rest.find('>')?;
        return Some(&rest[..=end]);
    }
}

pub(crate) const PDF_MIME_TYPE: &str = "application/pdf";
pub(crate) const PLAIN_TEXT_MIME_TYPE: &str = "text/plain";
pub(crate) const POWER_POINT_MIME_TYPE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation";
pub(crate) const DOCX_MIME_TYPE: &str = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
pub(crate) const LEGACY_WORD_MIME_TYPE: &str = "application/msword";
pub(crate) const LEGACY_POWERPOINT_MIME_TYPE: &str = "application/vnd.ms-powerpoint";

pub(crate) const PST_MIME_TYPE: &str = "application/vnd.ms-outlook-pst";
pub(crate) const WPD_MIME_TYPE: &str = "application/vnd.wordperfect";
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

#[cfg(feature = "hwpx")]
pub(crate) const HWPX_MIME_TYPE: &str = "application/haansofthwpx";
pub(crate) const IWORK_PAGES_MIME_TYPE: &str = "application/x-iwork-pages-sffpages";
pub(crate) const IWORK_NUMBERS_MIME_TYPE: &str = "application/x-iwork-numbers-sffnumbers";
pub(crate) const IWORK_KEYNOTE_MIME_TYPE: &str = "application/x-iwork-keynote-sffkey";

/// Docling DocTags. The format has no registered media type, and its files are
/// conventionally named `*.doctags.txt`, so callers reading those will need to
/// pass this explicitly rather than relying on the extension.
pub(crate) const DOCTAGS_MIME_TYPE: &str = "text/vnd.docling.doctags";

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
        extensions: &["adoc", "asciidoc"],
        mime_type: "text/asciidoc",
        aliases: &["text/x-asciidoc"],
    },
    FormatEntry {
        extensions: &["vtt"],
        mime_type: "text/vtt",
        aliases: &[],
    },
    // text/troff, text/x-mdoc, text/x-pod and text/x-dokuwiki were removed here (#228). They
    // carried no extensions, so they were unreachable except via a caller-supplied MIME, and
    // the only "extractor" behind them was the plain-text one — which BOM-stripped and split
    // on blank lines, turning roff macros and POD commands into prose that looked like a
    // successful extraction. Listing them made `validate_mime_type` return Ok for formats
    // nothing could actually parse, which is the advertised-but-unroutable half of GH#1387.
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
        extensions: &["doctags"],
        mime_type: DOCTAGS_MIME_TYPE,
        aliases: &["application/vnd.docling.doctags"],
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
        extensions: &["wpd", "wp", "wp5", "wp6"],
        mime_type: WPD_MIME_TYPE,
        aliases: &["application/wordperfect"],
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
        extensions: &["jats", "nxml"],
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
    if from_magic == ZIP_MIME_TYPE || from_magic.starts_with("application/vnd.oasis.opendocument.") {
        if let Some(package_mime) = detect_zip_mimetype_entry(std::fs::File::open(path).ok()?) {
            return (package_mime != extension_mime).then(|| package_mime.to_string());
        }
        // The header holds only the first entries of the archive, and a real
        // document names its main part far later: `ppt/presentation.xml` sits
        // 107 KB into a 27-slide deck. Reading the archive directory finds the
        // part wherever it is, so the document is not mistaken for a plain ZIP.
        if let Some(office_mime) = detect_office_format_from_archive(std::fs::File::open(path).ok()?) {
            return (office_mime != extension_mime).then(|| office_mime.to_string());
        }
    }

    if from_magic == PLAIN_TEXT_MIME_TYPE {
        return None;
    }
    if is_generic_xml_mime(&from_magic) && is_specific_xml_mime(extension_mime) {
        return None;
    }
    if from_magic == JSON_MIME_TYPE && is_specific_json_mime(extension_mime) {
        return None;
    }
    if from_magic != extension_mime && validate_mime_type(&from_magic).is_ok() {
        Some(from_magic)
    } else {
        None
    }
}

/// Generic XML signatures cannot distinguish specialized XML vocabularies.
/// Preserve a supported extension-specific XML MIME so extractor selection can
/// route formats such as FictionBook and DocBook to their semantic parsers.
fn is_specific_xml_mime(mime_type: &str) -> bool {
    mime_type != XML_MIME_TYPE && (mime_type.ends_with("+xml") || mime_type.contains("xml+"))
}

fn is_generic_xml_mime(mime_type: &str) -> bool {
    matches!(mime_type, XML_MIME_TYPE | "text/xml")
}

/// Generic JSON detection cannot distinguish JSON-based document formats.
/// Preserve extension-specific routing for notebooks and line-delimited JSON. ~keep
fn is_specific_json_mime(mime_type: &str) -> bool {
    mime_type != JSON_MIME_TYPE
        && (mime_type.ends_with("+json")
            || matches!(
                mime_type,
                "application/x-ndjson" | "application/jsonl" | "application/x-jsonlines"
            ))
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
            return Ok(detect_zip_mimetype_entry(std::io::Cursor::new(content))
                .unwrap_or(ZIP_MIME_TYPE)
                .to_string());
        }

        if mime_type == "application/zip"
            && let Some(office_mime) = detect_office_format_from_zip(content)
        {
            return Ok(office_mime.to_string());
        }

        if SUPPORTED_MIME_TYPES.contains(mime_type) || mime_type.starts_with("image/") {
            // `infer` reads the `<?xml` declaration and stops at generic XML, so
            // the vocabulary check has to run before that result is returned.
            // A caller may pass a truncated header, so decode lossily: a split
            // multi-byte character must not suppress the check.
            let prolog = String::from_utf8_lossy(&content[..content.len().min(8192)]);
            if is_generic_xml_mime(mime_type)
                && let Some(vocabulary) = xml_vocabulary(prolog.trim_start())
            {
                return Ok(vocabulary.to_string());
            }
            return Ok(mime_type.to_string());
        }
    }

    if content.len() >= 4 && content[..4] == [0x21, 0x42, 0x44, 0x4E] {
        return Ok(PST_MIME_TYPE.to_string());
    }

    // WordPerfect (Windows/DOS variants): magic bytes `\xffWPC`. The Mac
    // WordPerfect variant has no reliable magic bytes and is routed by the
    // `.wpd` extension via `EXT_TO_MIME` instead.
    if content.len() >= 4 && content[..4] == [0xFF, 0x57, 0x50, 0x43] {
        return Ok(WPD_MIME_TYPE.to_string());
    }

    if let Ok(text) = std::str::from_utf8(content) {
        let trimmed = text.trim_start();

        if (trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<serde_json::Value>(text).is_ok()
        {
            return Ok(JSON_MIME_TYPE.to_string());
        }

        // The HTML checks must precede the generic `<` fallback. They used to follow it,
        // where `trimmed.starts_with('<')` matched every tag first and made them dead code
        // (#235). HTML still routed correctly for whole documents only because `infer::get`
        // recognises those earlier in this function; a bare fragment reached here and was
        // typed `application/xml`, then handed to the XML extractor. ~keep
        if !trimmed.starts_with("<?xml") && looks_like_html(trimmed) {
            return Ok(HTML_MIME_TYPE.to_string());
        }

        if trimmed.starts_with("<?xml") || trimmed.starts_with('<') {
            if let Some(vocabulary) = xml_vocabulary(trimmed) {
                return Ok(vocabulary.to_string());
            }
            return Ok(XML_MIME_TYPE.to_string());
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
    const KEYNOTE_SLIDE_MARKERS: &[&[u8]] = &[b"Index/Slide-", b"Index/Slide_"];

    #[cfg(feature = "hwpx")]
    const HWPX_MARKER: &[u8] = b"Contents/content.hpf";
    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    if let Some(package_mime) = detect_zip_mimetype_entry(std::io::Cursor::new(content)) {
        return Some(package_mime);
    }

    #[cfg(feature = "hwpx")]
    if contains_subsequence(content, HWPX_MARKER) {
        return Some(HWPX_MIME_TYPE);
    }

    // A Numbers package carries `Index/Document.iwa` as well, so the
    // discriminating parts are tested before it.
    if contains_subsequence(content, NUMBERS_MARKER) {
        return Some(IWORK_NUMBERS_MIME_TYPE);
    }
    // ~keep: Minimal Keynote packages may contain slide archives without a Presentation.iwa index.
    if contains_subsequence(content, KEYNOTE_MARKER)
        || KEYNOTE_SLIDE_MARKERS
            .iter()
            .any(|marker| contains_subsequence(content, marker))
    {
        return Some(IWORK_KEYNOTE_MIME_TYPE);
    }
    if contains_subsequence(content, PAGES_MARKER) {
        return Some(IWORK_PAGES_MIME_TYPE);
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

/// Read the `mimetype` entry a ZIP-based document package declares.
///
/// OpenDocument and HWPX both store their own media type in an uncompressed
/// `mimetype` entry, which is the format's authoritative identifier. An HWPX
/// package that carries no `Contents/content.hpf` is still identified here.
/// A package with two `mimetype` entries is rejected rather than guessed at.
/// Identify a ZIP-based office format from the names in the archive directory.
///
/// `detect_office_format_from_zip` searches raw bytes, so it only sees the part
/// of the archive it was given. A caller that reads a fixed-size header misses
/// every part written after it.
#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn detect_office_format_from_archive<R: Read + Seek>(mut reader: R) -> Option<&'static str> {
    let limits = SecurityLimits::default();
    if !zip_central_directory_within_limits(&mut reader, &limits) {
        return None;
    }
    reader.seek(SeekFrom::Start(0)).ok()?;
    let mut archive = zip::ZipArchive::new(reader).ok()?;

    let has = |archive: &mut zip::ZipArchive<R>, name: &str| archive.index_for_name(name).is_some();
    #[cfg(feature = "hwpx")]
    if has(&mut archive, "Contents/content.hpf") {
        return Some(HWPX_MIME_TYPE);
    }
    if has(&mut archive, "word/document.xml") {
        return Some(DOCX_MIME_TYPE);
    }
    if has(&mut archive, "xl/workbook.xml") {
        return Some(EXCEL_MIME_TYPE);
    }
    if has(&mut archive, "ppt/presentation.xml") {
        return Some(POWER_POINT_MIME_TYPE);
    }
    // A Numbers package also carries `Index/Document.iwa`, so the discriminating
    // parts are tested first. Otherwise a spreadsheet is read as a Pages
    // document and yields no sheets at all.
    if has(&mut archive, "Index/CalculationEngine.iwa") {
        return Some(IWORK_NUMBERS_MIME_TYPE);
    }
    if has(&mut archive, "Index/Presentation.iwa")
        || archive.file_names().any(|n| n.starts_with("Index/Slide-") || n.starts_with("Index/Slide_"))
    {
        return Some(IWORK_KEYNOTE_MIME_TYPE);
    }
    if has(&mut archive, "Index/Document.iwa") {
        return Some(IWORK_PAGES_MIME_TYPE);
    }
    None
}

#[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
fn detect_zip_mimetype_entry<R: Read + Seek>(mut reader: R) -> Option<&'static str> {
    /// The value an HWPX package stores in its `mimetype` entry.
    const HWPX_PACKAGE_MIMETYPE: &[u8] = b"application/hwp+zip";
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

    #[cfg(feature = "hwpx")]
    let has_hwpx_manifest = archive.index_for_name("Contents/content.hpf").is_some();

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
        // The HWPX reader needs the manifest, so a package without one keeps its
        // ZIP routing and its members stay readable. The entry is looked up in
        // the archive directory, because Hangul writes it near the end of the
        // file, past any header a caller may have truncated to.
        #[cfg(feature = "hwpx")]
        value if value == HWPX_PACKAGE_MIMETYPE && has_hwpx_manifest => Some(HWPX_MIME_TYPE),
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

    for entry in FORMATS {
        if entry.mime_type == mime_type || entry.aliases.contains(&mime_type) {
            extensions.extend(entry.extensions.iter().map(|extension| (*extension).to_string()));
        }
    }

    if !extensions.is_empty() {
        extensions.sort();
        extensions.dedup();
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
/// The static `EXT_TO_MIME` table lists every format the *codebase* knows how
/// to describe, regardless of which Cargo features were compiled in. Advertising
/// that table directly would claim support for extractors that may not exist in
/// this build (see GH#1387). To keep the advertised catalogue honest, the table
/// is intersected with the document extractor registry: an extension is only
/// included if some registered extractor actually claims its MIME type in this
/// build. This can never drift from reality and automatically covers
/// third-party extractors registered at runtime.
///
/// The list is sorted alphabetically by file extension.
///
/// # Returns
///
/// A vector of [`SupportedFormat`] entries sorted by extension, limited to
/// formats with a registered extractor in this build.
///
/// # Example
///
/// ```
/// use xberg::core::mime::list_supported_formats;
///
/// let formats = list_supported_formats();
/// assert!(!formats.is_empty());
/// ```
pub fn list_supported_formats() -> Vec<SupportedFormat> {
    if let Err(error) = crate::extractors::ensure_initialized() {
        tracing::warn!(%error, "failed to initialize document extractor registry before listing formats");
    }

    let registry = crate::plugins::registry::get_document_extractor_registry();
    let registry_guard = registry.read();

    let mut formats: Vec<SupportedFormat> = EXT_TO_MIME
        .iter()
        .filter(|(_ext, mime)| registry_guard.get(mime).is_ok())
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

    #[cfg(all(feature = "xml", feature = "tokio-runtime", not(target_arch = "wasm32")))]
    async fn assert_specialized_xml_routes_through_real_extractor(
        extension: &str,
        content: &str,
        expected_mime: &str,
        expected_text: &str,
    ) {
        let dir = tempdir().unwrap();
        let file_path = dir.path().join(format!("routing.{extension}"));
        std::fs::write(&file_path, content).unwrap();

        let config = crate::core::config::ExtractionConfig {
            use_cache: false,
            ..Default::default()
        };
        let result = crate::core::extractor::extract_file(&file_path, None, &config)
            .await
            .unwrap();

        assert_eq!(result.mime_type, expected_mime);
        assert!(
            result.content.contains(expected_text),
            "specialized extractor lost expected text: {}",
            result.content
        );
        assert!(
            !result.content.contains("<article") && !result.content.contains("<FictionBook"),
            "generic XML markup leaked into extracted content: {}",
            result.content
        );
    }

    #[cfg(all(feature = "office", feature = "xml", feature = "tokio-runtime"))]
    #[tokio::test]
    async fn should_route_fb2_extension_to_fictionbook_extractor() {
        let content = r#"<?xml version="1.0" encoding="utf-8"?>
<FictionBook xmlns="http://www.gribuser.ru/xml/fictionbook/2.0">
  <description><title-info><book-title>Routing Test</book-title></title-info></description>
  <body><section><title><p>First Chapter</p></title><p>FictionBook semantic text.</p></section></body>
</FictionBook>"#;

        assert_specialized_xml_routes_through_real_extractor(
            "fb2",
            content,
            "application/x-fictionbook+xml",
            "FictionBook semantic text.",
        )
        .await;
    }

    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    #[test]
    fn should_detect_hwpx_without_a_content_hpf_from_its_mimetype_entry() {
        // Real Hangul packages carry `version.xml` and `Contents/section0.xml`
        // but no `Contents/content.hpf`, so only the `mimetype` entry names them.
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let stored =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("mimetype", stored).unwrap();
            std::io::Write::write_all(&mut writer, b"application/hwp+zip").unwrap();
            writer
                .start_file("Contents/section0.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, b"<hs:sec/>").unwrap();
            // Written last, as Hangul does, so a truncated header cannot see it.
            writer
                .start_file("Contents/content.hpf", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, b"<opf:package/>").unwrap();
            writer.finish().unwrap();
        }

        assert_eq!(
            detect_mime_type_from_bytes(&buffer).unwrap(),
            "application/haansofthwpx"
        );
    }

    #[test]
    fn should_detect_docbook_by_namespace_when_extension_is_plain_xml() {
        // Real DocBook ships as `.xml`, so only the namespace identifies it.
        let content = br#"<!DOCTYPE refentry [ <!ENTITY % mathent SYSTEM "math.ent"> %mathent; ]>
<refentry xmlns="http://docbook.org/ns/docbook" version="5.0" xml:id="exp">
  <refsect1><para>Text.</para></refsect1>
</refentry>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "application/docbook+xml");
    }

    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    #[test]
    fn should_detect_a_deck_whose_main_part_is_written_late_in_the_archive() {
        // A real 27-slide deck names `ppt/presentation.xml` 107 KB in, so a
        // detector that reads a fixed-size header sees a plain ZIP and the
        // presentation extracts as a list of archive members.
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let options = zip::write::SimpleFileOptions::default();
            for index in 0..12 {
                writer.start_file(format!("ppt/media/image{index}.bin"), options).unwrap();
                std::io::Write::write_all(&mut writer, &vec![index as u8; 8192]).unwrap();
            }
            writer.start_file("ppt/presentation.xml", options).unwrap();
            std::io::Write::write_all(&mut writer, b"<p:presentation/>").unwrap();
            writer.finish().unwrap();
        }
        // Name it `.zip` so the extension does not answer the question. That is
        // the path a real deck takes: the header search fails, and only the
        // archive directory can identify it.
        let path = std::env::temp_dir().join("xberg_late_part_test.zip");
        std::fs::write(&path, &buffer).unwrap();

        let detected = detect_or_validate(path.to_str(), None).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(
            detected,
            "application/vnd.openxmlformats-officedocument.presentationml.presentation",
            "the deck is identified by its parts, not by its name"
        );
    }

    #[cfg(any(feature = "office", feature = "hwpx", feature = "iwork", feature = "archives"))]
    #[test]
    fn should_keep_zip_routing_for_an_hwpx_package_without_its_manifest() {
        // `unhwp` needs `Contents/content.hpf`. Without it the HWPX extractor
        // fails outright, so the package stays on the ZIP route and its members
        // remain readable.
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buffer));
            let stored =
                zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
            writer.start_file("mimetype", stored).unwrap();
            std::io::Write::write_all(&mut writer, b"application/hwp+zip").unwrap();
            writer
                .start_file("Contents/section0.xml", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, b"<hs:sec/>").unwrap();
            writer.finish().unwrap();
        }

        assert_eq!(detect_mime_type_from_bytes(&buffer).unwrap(), "application/zip");
    }

    #[test]
    fn should_keep_generic_xml_for_a_stylesheet_that_only_names_docbook() {
        // A DocBook XSL customization layer binds the namespace on a foreign
        // root. It is not a DocBook document, and the DocBook extractor drops
        // every element it does not know.
        let content = br#"<?xml version="1.0"?>
<xsl:stylesheet xmlns:xsl="http://www.w3.org/1999/XSL/Transform" xmlns:d="http://docbook.org/ns/docbook" version="1.0">
  <xsl:template match="d:para"><p><xsl:apply-templates/></p></xsl:template>
</xsl:stylesheet>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "text/xml");
    }

    #[test]
    fn should_keep_generic_xml_for_a_catalog_that_has_a_doctype_and_names_the_docbook_dtd() {
        // The declaration ends at its own `>`. A `]` later in the body must not
        // stretch it over the public identifier that follows.
        let content = br#"<?xml version="1.0"?>
<!DOCTYPE catalog PUBLIC "-//OASIS//DTD Entity Resolution XML Catalog V1.0//EN" "catalog.dtd">
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <public publicId="-//OASIS//DTD DocBook XML V4.5//EN" uri="docbookx.dtd"/>
  <note>index a[0] and b[1]</note>
</catalog>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "text/xml");
    }

    #[test]
    fn should_detect_docbook_when_the_body_holds_a_bracket() {
        // A `]` in the body must not make the root element unreachable.
        let content = br#"<?xml version="1.0"?>
<!DOCTYPE book SYSTEM "docbook.dtd">
<book xmlns="http://docbook.org/ns/docbook" version="5.0">
  <chapter><para>The value a[0] is first.</para></chapter>
</book>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "application/docbook+xml");
    }

    #[test]
    fn should_keep_generic_xml_for_a_catalog_that_lists_the_docbook_dtd() {
        let content = br#"<?xml version="1.0"?>
<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog">
  <public publicId="-//OASIS//DTD DocBook XML V4.5//EN" uri="docbookx.dtd"/>
</catalog>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "text/xml");
    }

    #[test]
    fn should_detect_docbook_when_the_namespace_is_bound_to_a_prefix() {
        let content = br#"<?xml version="1.0"?>
<db:book xmlns:db="http://docbook.org/ns/docbook" version="5.0">
  <db:chapter><db:para>Text.</db:para></db:chapter>
</db:book>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "application/docbook+xml");
    }

    #[test]
    fn should_detect_docbook_by_dtd_public_identifier() {
        let content = br#"<?xml version="1.0"?>
<!DOCTYPE article PUBLIC "-//OASIS//DTD DocBook XML V4.4//EN" "http://www.oasis-open.org/docbook/xml/4.4/docbookx.dtd">
<article><para>Text.</para></article>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "application/docbook+xml");
    }

    #[test]
    fn should_detect_jats_by_dtd_public_identifier() {
        let content = br#"<?xml version="1.0"?>
<!DOCTYPE article PUBLIC "-//NLM//DTD JATS (Z39.96) Journal Archiving DTD v1.0 20120330//EN" "JATS-archivearticle1.dtd">
<article><body><p>Text.</p></body></article>"#;

        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "application/x-jats+xml");
    }

    #[test]
    fn should_keep_generic_xml_without_a_vocabulary_declaration() {
        let content = br#"<?xml version="1.0"?><catalog><item>Text.</item></catalog>"#;

        // `text/xml` is the registered alias `infer` returns for a declaration.
        assert_eq!(detect_mime_type_from_bytes(content).unwrap(), "text/xml");
    }

    #[cfg(all(feature = "office", feature = "xml", feature = "tokio-runtime"))]
    #[tokio::test]
    async fn should_route_docbook_extensions_to_docbook_extractor() {
        let content = r#"<?xml version="1.0" encoding="utf-8"?>
<article xmlns="http://docbook.org/ns/docbook" version="5.0">
  <title>Routing Test</title><para>DocBook semantic text.</para>
</article>"#;

        for extension in ["docbook", "dbk"] {
            assert_specialized_xml_routes_through_real_extractor(
                extension,
                content,
                "application/docbook+xml",
                "DocBook semantic text.",
            )
            .await;
        }
    }

    #[cfg(all(feature = "xml", feature = "tokio-runtime", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn should_route_nxml_extension_to_jats_extractor() {
        let content = r#"<?xml version="1.0" encoding="utf-8"?>
<article>
<front><article-meta><title-group><article-title>Routing Test</article-title></title-group></article-meta></front>
<body><sec><title>Results</title><p>NXML semantic text.</p></sec></body></article>"#;

        assert_specialized_xml_routes_through_real_extractor(
            "nxml",
            content,
            "application/x-jats+xml",
            "NXML semantic text.",
        )
        .await;
    }

    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    #[tokio::test]
    async fn should_route_benchmark_text_extensions_to_plain_text_extractor() {
        let test_cases = [
            ("adoc", "text/asciidoc", "AsciiDoc short-extension routing text."),
            ("asciidoc", "text/asciidoc", "AsciiDoc routing text."),
            ("vtt", "text/vtt", "WebVTT routing text."),
        ];

        for (extension, expected_mime, expected_text) in test_cases {
            let dir = tempdir().unwrap();
            let file_path = dir.path().join(format!("routing.{extension}"));
            std::fs::write(&file_path, expected_text).unwrap();

            let config = crate::core::config::ExtractionConfig {
                use_cache: false,
                ..Default::default()
            };
            let result = crate::core::extractor::extract_file(&file_path, None, &config)
                .await
                .unwrap();

            assert_eq!(result.mime_type, expected_mime);
            assert!(result.content.contains(expected_text));
        }
    }

    #[test]
    fn should_resolve_registered_mime_alias_to_extensions() {
        assert_eq!(
            get_extensions_for_mime("text/x-asciidoc").unwrap(),
            vec!["adoc".to_string(), "asciidoc".to_string()]
        );
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
    fn specialized_json_extensions_are_not_overridden_by_generic_json_detection() {
        let dir = tempdir().unwrap();
        let cases = [
            ("document.json", br#"{"value":1}"#.as_slice(), JSON_MIME_TYPE),
            ("records.jsonl", br#"{"value":1}"#.as_slice(), "application/x-ndjson"),
            (
                "notebook.ipynb",
                br#"{"cells":[],"metadata":{},"nbformat":4,"nbformat_minor":5}"#.as_slice(),
                "application/x-ipynb+json",
            ),
        ];

        for (filename, content, expected_mime) in cases {
            let path = dir.path().join(filename);
            std::fs::write(&path, content).unwrap();
            assert_eq!(
                detect_or_validate(path.to_str(), None).unwrap(),
                expected_mime,
                "{filename} should retain its extension-specific JSON MIME type"
            );
        }
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

    /// The headline "N formats · M file extensions" is hand-typed in the README
    /// templates and the docs site, and nothing derived it from [`FORMATS`] — so it
    /// drifted to "101 formats · 115 file extensions" against a table holding 100
    /// entries and 120 extensions, and propagated into every generated README.
    ///
    /// Mirrors `core::formats::tests::test_known_formats_count`. If this fails because
    /// a format was legitimately added or removed, update the numbers here **and** in
    /// the copy listed below, which is where the published figures come from:
    ///
    /// - `templates/readme/root.md`, `cli.md`, `rust.md`,
    ///   `templates/readme/partials/features.md.jinja`, `templates/docs/llms-body.md.jinja`
    /// - `docs-site/src/content/docs/`: `index.mdx`, `features.mdx`, `ecosystem.md`,
    ///   `cli/usage.mdx`, `guides/extraction.mdx`, `guides/rust-core-api.md`,
    ///   `integrations/langchain.mdx`, `integrations/txtai.md`
    ///
    /// The generated READMEs (root `README.md`, `packages/*/README.md`, the crate
    /// READMEs) pick the change up on the next alef regen -- do not hand-edit those.
    #[test]
    fn format_and_extension_counts_match_the_published_headline() {
        const PUBLISHED_FORMATS: usize = 100;
        const PUBLISHED_EXTENSIONS: usize = 120;

        let extensions: HashSet<&str> = FORMATS
            .iter()
            .flat_map(|entry| entry.extensions.iter().copied())
            .collect();

        assert_eq!(
            FORMATS.len(),
            PUBLISHED_FORMATS,
            "FORMATS has {} entries but the docs advertise {PUBLISHED_FORMATS} formats",
            FORMATS.len()
        );
        assert_eq!(
            extensions.len(),
            PUBLISHED_EXTENSIONS,
            "FORMATS covers {} unique extensions but the docs advertise {PUBLISHED_EXTENSIONS}",
            extensions.len()
        );
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
        // `list_supported_formats` now filters against the registered extractor set
        // (#308), so assertions for extensions gated behind optional Cargo features
        // only hold when those features are compiled in.
        let formats = list_supported_formats();
        let extensions: Vec<&str> = formats.iter().map(|f| f.extension.as_str()).collect();

        #[cfg(feature = "pdf")]
        assert!(extensions.contains(&"pdf"), "Should include pdf");
        assert!(extensions.contains(&"md"), "Should include md");
        #[cfg(feature = "office")]
        assert!(extensions.contains(&"docx"), "Should include docx");
        #[cfg(feature = "html")]
        assert!(extensions.contains(&"html"), "Should include html");
        assert!(extensions.contains(&"txt"), "Should include txt");
        assert!(extensions.contains(&"csv"), "Should include csv");
        assert!(extensions.contains(&"json"), "Should include json");
        #[cfg(any(feature = "excel", feature = "excel-wasm"))]
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

    /// Every alias in [`FORMATS`] must route to the same extractor as its canonical MIME
    /// type.
    ///
    /// `validate_mime_type` accepts an alias verbatim — it does not normalize it to the
    /// canonical form — and `DocumentExtractorRegistry::get` resolves by exact string with
    /// no alias awareness. So an alias that no extractor lists in `supported_mime_types()`
    /// is advertised as supported by `list_supported_formats()` and then rejected as
    /// `UnsupportedFormat` at extraction time (#229, and #289 for the same shape).
    ///
    /// Formats whose canonical MIME has no registered extractor are skipped, so this stays
    /// valid under any feature set: it only ever asserts that an alias is no worse off than
    /// the canonical name beside it.
    #[test]
    fn every_declared_alias_resolves_to_the_same_extractor_as_its_canonical_mime() {
        crate::extractors::ensure_initialized().expect("failed to initialize default extractors");
        let registry = crate::plugins::registry::get_document_extractor_registry();
        let registry = registry.read();

        let mut unclaimed = Vec::new();
        for format in FORMATS {
            let Ok(canonical) = registry.get(format.mime_type) else {
                continue;
            };
            for alias in format.aliases {
                match registry.get(alias) {
                    Ok(aliased) if aliased.name() == canonical.name() => {}
                    Ok(aliased) => unclaimed.push(format!(
                        "{alias} (alias of {}) resolves to {}, not {}",
                        format.mime_type,
                        aliased.name(),
                        canonical.name()
                    )),
                    Err(_) => unclaimed.push(format!(
                        "{alias} (alias of {}) resolves to no extractor, but {} does",
                        format.mime_type, format.mime_type
                    )),
                }
            }
        }

        assert!(
            unclaimed.is_empty(),
            "declared alias MIME types are advertised as supported but unroutable:\n  {}",
            unclaimed.join("\n  ")
        );
    }
}
