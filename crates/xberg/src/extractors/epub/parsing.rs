//! EPUB ZIP archive and XML parsing utilities.
//!
//! Provides low-level parsing functionality for EPUB container structure,
//! including ZIP archive operations and container.xml parsing.

use crate::Result;
use roxmltree;
use std::io::Cursor;
use zip::ZipArchive;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CanonicalHref {
    pub(super) path: String,
    pub(super) fragment: Option<String>,
}

/// Parse container.xml to find the OPF file path
pub(super) fn parse_container_xml(xml: &str) -> Result<String> {
    match roxmltree::Document::parse(xml) {
        Ok(doc) => {
            for node in doc.descendants() {
                if node.tag_name().name() == "rootfile"
                    && let Some(full_path) = node.attribute("full-path")
                {
                    return Ok(full_path.to_string());
                }
            }
            Err(crate::XbergError::Parsing {
                message: "No rootfile found in container.xml".to_string(),
                source: None,
            })
        }
        Err(e) => Err(crate::XbergError::Parsing {
            message: format!("Failed to parse container.xml: {}", e),
            source: None,
        }),
    }
}

/// Read a file from the ZIP archive
pub(super) fn read_file_from_zip(archive: &mut ZipArchive<Cursor<Vec<u8>>>, path: &str) -> Result<String> {
    match archive.by_name(path) {
        Ok(mut file) => {
            let mut content = String::new();
            match std::io::Read::read_to_string(&mut file, &mut content) {
                Ok(_) => Ok(content),
                Err(e) => Err(crate::XbergError::Parsing {
                    message: format!("Failed to read file from EPUB: {}", e),
                    source: None,
                }),
            }
        }
        Err(e) => Err(crate::XbergError::Parsing {
            message: format!("File not found in EPUB: {} ({})", path, e),
            source: None,
        }),
    }
}

fn split_href(href: &str) -> (&str, Option<&str>) {
    href.split_once('#')
        .map_or((href, None), |(path, fragment)| (path, Some(fragment)))
}

/// Resolve an EPUB href relative to the OPF directory.
///
/// The returned path is package-relative and normalized. Attempts to escape the
/// EPUB package root via leading `..` segments are rejected.
pub(super) fn resolve_path(base_dir: &str, href: &str) -> Result<CanonicalHref> {
    let (relative_path, fragment) = split_href(href);
    // An href is a URL, and a ZIP entry name is not. A real package writes a
    // colon in a file name as `%3A`, so the two only match once the href is
    // decoded.
    let decoded = urlencoding::decode(relative_path).unwrap_or(std::borrow::Cow::Borrowed(relative_path));
    let relative_path: &str = &decoded;
    let combined = if relative_path.starts_with('/') {
        relative_path.trim_start_matches('/').to_string()
    } else if base_dir.is_empty() || base_dir == "." {
        relative_path.to_string()
    } else {
        format!("{}/{}", base_dir.trim_end_matches('/'), relative_path)
    };

    let mut normalized = Vec::new();
    for segment in combined.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                if normalized.pop().is_none() {
                    return Err(crate::XbergError::Parsing {
                        message: format!("EPUB href '{}' escapes the package root", href),
                        source: None,
                    });
                }
            }
            _ => normalized.push(segment),
        }
    }

    let path = normalized.join("/");
    if path.is_empty() {
        return Err(crate::XbergError::Parsing {
            message: format!("EPUB href '{}' does not contain a resolvable path", href),
            source: None,
        });
    }

    Ok(CanonicalHref {
        path,
        fragment: fragment.filter(|value| !value.is_empty()).map(ToString::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real package writes a colon in a file name as `%3A` in the manifest,
    /// while the ZIP entry keeps the character itself.
    #[test]
    fn test_resolve_path_decodes_a_percent_encoded_href() {
        let resolved = resolve_path("contents", "a7b196ae@d22925c%3Ab4e683fb.xhtml").expect("resolves");
        assert_eq!(resolved.path, "contents/a7b196ae@d22925c:b4e683fb.xhtml");
    }

    /// A space is written `%20`, which must not stay literal either.
    #[test]
    fn test_resolve_path_decodes_a_space() {
        let resolved = resolve_path("OEBPS", "chapter%201.xhtml").expect("resolves");
        assert_eq!(resolved.path, "OEBPS/chapter 1.xhtml");
    }

    #[test]
    fn test_resolve_path_with_base_dir() {
        let result = resolve_path("OEBPS", "chapter.xhtml").expect("path should resolve");
        assert_eq!(result.path, "OEBPS/chapter.xhtml");
        assert_eq!(result.fragment, None);
    }

    #[test]
    fn test_resolve_path_absolute() {
        let result = resolve_path("OEBPS", "/chapter.xhtml").expect("path should resolve");
        assert_eq!(result.path, "chapter.xhtml");
        assert_eq!(result.fragment, None);
    }

    #[test]
    fn test_resolve_path_empty_base() {
        let result = resolve_path("", "chapter.xhtml").expect("path should resolve");
        assert_eq!(result.path, "chapter.xhtml");
        assert_eq!(result.fragment, None);
    }

    #[test]
    fn test_resolve_path_parent_segment() {
        let result = resolve_path("OEBPS/text", "../images/cover.xhtml").expect("path should resolve");
        assert_eq!(result.path, "OEBPS/images/cover.xhtml");
        assert_eq!(result.fragment, None);
    }

    #[test]
    fn test_resolve_path_preserves_fragment() {
        let result = resolve_path("OEBPS", "toc.xhtml#nav").expect("path should resolve");
        assert_eq!(result.path, "OEBPS/toc.xhtml");
        assert_eq!(result.fragment.as_deref(), Some("nav"));
    }

    #[test]
    fn test_resolve_path_rejects_root_escape() {
        let err = resolve_path("", "../chapter.xhtml").expect_err("path should be rejected");
        assert!(err.to_string().contains("escapes the package root"));
    }
}
