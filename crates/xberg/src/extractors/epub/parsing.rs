//! EPUB ZIP archive and XML parsing utilities.
//!
//! Provides low-level parsing functionality for EPUB container structure,
//! including ZIP archive operations and container.xml parsing.

use crate::Result;
use roxmltree;
use std::io::Cursor;
use zip::ZipArchive;

/// Maximum bytes read from any single ZIP member (container.xml, the OPF, a spine
/// XHTML document, or an embedded image), mirroring the `.take()` precedent set by
/// DOCX (`crate::extraction::docx::MAX_UNCOMPRESSED_FILE_SIZE`).
///
/// `ZipBombValidator::validate` (called once at archive open, see
/// `EpubExtractor::extract_content`) already bounds every entry's *declared*
/// uncompressed size via the central directory -- but it trusts that header
/// completely, never decompressing anything itself. This constant instead bounds
/// the actual bytes read via `.take()`, so a member whose real decompressed stream
/// exceeds what its header claims (a lying header, not just an honest large file)
/// still cannot exhaust memory.
///
/// 16 MiB is generous for any real EPUB: a spine XHTML chapter is normally tens of
/// KB of text, and even an unusually large embedded cover image rarely exceeds a
/// few MB. No legitimate EPUB member is expected to approach this ceiling.
pub(super) const MAX_EPUB_MEMBER_SIZE: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CanonicalHref {
    pub(super) path: String,
    pub(super) fragment: Option<String>,
}

/// Parse EPUB packaging XML such as `container.xml` and OPF package documents.
///
/// Legacy EPUBs may contain DTD declarations. `roxmltree` does not resolve external
/// entities, and retains its entity-expansion limits when DTD parsing is enabled.
pub(super) fn parse_packaging_xml(xml: &str) -> std::result::Result<roxmltree::Document<'_>, roxmltree::Error> {
    roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
}

/// Parse container.xml to find the OPF file path
pub(super) fn parse_container_xml(xml: &str) -> Result<String> {
    match parse_packaging_xml(xml) {
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
    // A manifest writes its hrefs as URLs, so the path arrives decoded. A
    // package whose entry name holds a literal `%` needs the name as written,
    // so the encoded form is tried when the decoded one is absent.
    let path = if archive.index_for_name(path).is_some() {
        std::borrow::Cow::Borrowed(path)
    } else {
        let encoded = urlencoding::encode(path).replace("%2F", "/");
        if archive.index_for_name(&encoded).is_some() {
            std::borrow::Cow::Owned(encoded)
        } else {
            std::borrow::Cow::Borrowed(path)
        }
    };
    let path: &str = &path;
    match archive.by_name(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            let mut bounded = std::io::Read::take(file, MAX_EPUB_MEMBER_SIZE);
            match std::io::Read::read_to_end(&mut bounded, &mut bytes) {
                Ok(_) => decode_member_text(&bytes),
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

/// Decode an XML member to text. A byte order mark selects UTF-16 or UTF-8.
/// Valid UTF-8 is taken as is. Anything else goes through the
/// `<?xml encoding="..."?>` declaration, or charset detection when the
/// declaration is missing. Bytes that do not decode in the selected encoding
/// are an error, not replacement characters: an encrypted or binary member
/// must not surface as text.
pub(super) fn decode_member_text(bytes: &[u8]) -> Result<String> {
    if let Some((encoding, bom_len)) = encoding_rs::Encoding::for_bom(bytes) {
        let (decoded, had_errors) = encoding.decode_without_bom_handling(&bytes[bom_len..]);
        if had_errors {
            return Err(crate::XbergError::Parsing {
                message: format!("Failed to decode file from EPUB as {}", encoding.name()),
                source: None,
            });
        }
        return Ok(decoded.into_owned());
    }

    if let Ok(text) = std::str::from_utf8(bytes) {
        return Ok(text.to_string());
    }

    let (text, replaced_characters) = crate::utils::xml_utils::decode_xml_to_utf8(bytes);
    if replaced_characters {
        return Err(crate::XbergError::Parsing {
            message: "Failed to decode file from EPUB: the bytes are not valid text in the declared encoding"
                .to_string(),
            source: None,
        });
    }
    Ok(text)
}

/// Font obfuscation algorithms. A package that lists only these in
/// `META-INF/encryption.xml` is not DRM-protected.
const FONT_OBFUSCATION_ALGORITHMS: &[&str] = &["http://www.idpf.org/2008/embedding", "http://ns.adobe.com/pdf/enc#RC"];

/// Return the package-relative paths of the members that
/// `META-INF/encryption.xml` encrypts with an algorithm other than font
/// obfuscation.
pub(super) fn parse_encrypted_members(xml: &str) -> std::collections::BTreeSet<String> {
    let mut encrypted = std::collections::BTreeSet::new();
    let Ok(doc) = parse_packaging_xml(xml) else {
        return encrypted;
    };
    for data in doc
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "EncryptedData")
    {
        let algorithm = data
            .descendants()
            .find(|node| node.tag_name().name() == "EncryptionMethod")
            .and_then(|node| node.attribute("Algorithm"))
            .unwrap_or("");
        if FONT_OBFUSCATION_ALGORITHMS.contains(&algorithm) {
            continue;
        }
        for uri in data
            .descendants()
            .filter(|node| node.tag_name().name() == "CipherReference")
            .filter_map(|node| node.attribute("URI"))
        {
            if let Ok(resolved) = resolve_path("", uri) {
                encrypted.insert(resolved.path);
            }
        }
    }
    encrypted
}

fn split_href(href: &str) -> (&str, Option<&str>) {
    href.split_once('#')
        .map_or((href, None), |(path, fragment)| (path, Some(fragment)))
}

/// Resolve an EPUB href relative to the OPF directory.
///
/// The returned path is package-relative and normalized. This is a thin wrapper around the
/// shared [`crate::extractors::security::resolve_container_entry`], which implements the
/// boundary-relative rule this function pioneered: an in-bounds `..` (leaving and returning
/// without crossing the package root) is allowed, and only a `..` that pops past the root is
/// rejected. Percent-decoding happens here, before the call, and not inside the shared
/// helper -- decoding after resolution would let a decoded `../` slip past a boundary check
/// that already ran.
pub(super) fn resolve_path(base_dir: &str, href: &str) -> Result<CanonicalHref> {
    let (relative_path, fragment) = split_href(href);
    // An href is a URL, and a ZIP entry name is not. A real package writes a
    // colon in a file name as `%3A`, so the two only match once the href is
    // decoded. A package whose entry name holds a literal `%` is served by
    // `read_file_from_zip`, which retries with the undecoded name.
    let decoded = urlencoding::decode(relative_path).unwrap_or(std::borrow::Cow::Borrowed(relative_path));
    let relative_path: &str = &decoded;

    let path = crate::extractors::security::resolve_container_entry(base_dir, relative_path).map_err(|error| {
        use crate::extractors::security::PathTraversalError;
        // Preserve the exact wording the two pre-existing failure modes always had; the
        // other variants (NUL byte, drive/UNC prefix) are new checks this migration adds,
        // so they get the shared helper's generic message.
        let detail = match error {
            PathTraversalError::EscapesRoot => "escapes the package root".to_string(),
            PathTraversalError::EmptyResult => "does not contain a resolvable path".to_string(),
            other => other.to_string(),
        };
        crate::XbergError::Parsing {
            message: format!("EPUB href '{}' {}", href, detail),
            source: None,
        }
    })?;

    Ok(CanonicalHref {
        path,
        fragment: fragment.filter(|value| !value.is_empty()).map(ToString::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_container_with_legacy_packaging_dtd() {
        let xml = r#"<?xml version="1.0"?>
            <!DOCTYPE container PUBLIC
                "-//IDPF//DTD OEB 1.2 Package//EN"
                "http://openebook.org/dtds/oeb-1.2/oebpkg12.dtd">
            <container>
                <rootfiles>
                    <rootfile full-path="OEBPS/content.opf"/>
                </rootfiles>
            </container>"#;

        let path = parse_container_xml(xml).expect("legacy packaging DTD should be accepted");

        assert_eq!(path, "OEBPS/content.opf");
    }

    #[test]
    fn should_not_resolve_external_packaging_entities() {
        let xml = r#"<!DOCTYPE container [
                <!ENTITY external SYSTEM "file:///etc/passwd">
            ]>
            <container>
                <rootfiles>
                    <rootfile full-path="&external;"/>
                </rootfiles>
            </container>"#;

        let error = parse_container_xml(xml).expect_err("external entities must not be resolved in packaging XML");

        assert!(error.to_string().contains("unknown entity reference"));
    }

    #[test]
    fn should_reject_packaging_entity_amplification() {
        let xml = r#"<!DOCTYPE container [
                <!ENTITY a "x">
                <!ENTITY b "&a;&a;&a;&a;&a;&a;&a;&a;&a;&a;">
                <!ENTITY c "&b;&b;&b;&b;&b;&b;&b;&b;&b;&b;">
                <!ENTITY d "&c;&c;&c;&c;&c;&c;&c;&c;&c;&c;">
            ]>
            <container>
                <rootfiles>
                    <rootfile full-path="&d;"/>
                </rootfiles>
            </container>"#;

        let error = parse_container_xml(xml).expect_err("entity amplification must be rejected in packaging XML");

        assert!(error.to_string().contains("entity reference loop"));
    }

    /// A package whose entry name holds a literal `%` is read by the name as
    /// written, after the decoded name misses.
    #[test]
    fn test_read_file_from_zip_falls_back_to_the_encoded_name() {
        let mut buffer = Vec::new();
        {
            let mut writer = zip::ZipWriter::new(Cursor::new(&mut buffer));
            writer
                .start_file("OEBPS/chapter%201.xhtml", zip::write::SimpleFileOptions::default())
                .unwrap();
            std::io::Write::write_all(&mut writer, b"<p>text</p>").unwrap();
            writer.finish().unwrap();
        }
        let mut archive = ZipArchive::new(Cursor::new(buffer)).unwrap();

        // `resolve_path` decodes `%20` to a space, which is not the entry name.
        let content = read_file_from_zip(&mut archive, "OEBPS/chapter 1.xhtml").expect("reads");
        assert!(content.contains("text"));
    }

    /// A real package writes a colon in a file name as `%3A` in the manifest,
    /// while the ZIP entry keeps the character itself.
    #[test]
    fn test_resolve_path_decodes_a_percent_encoded_href() {
        let resolved = resolve_path("contents", "a7b196ae@d22925c%3Ab4e683fb.xhtml").expect("resolves");
        assert_eq!(resolved.path, "contents/a7b196ae@d22925c:b4e683fb.xhtml");
    }

    #[test]
    fn should_decode_latin1_and_utf16_members() {
        let latin1 = b"<?xml version=\"1.0\" encoding=\"iso-8859-1\"?><p>caf\xe9</p>";
        assert_eq!(
            decode_member_text(latin1).expect("latin-1 decodes"),
            "<?xml version=\"1.0\" encoding=\"iso-8859-1\"?><p>caf\u{e9}</p>"
        );

        let mut utf16 = vec![0xFF, 0xFE];
        for unit in "<p>caf\u{e9}</p>".encode_utf16() {
            utf16.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(decode_member_text(&utf16).expect("utf-16 decodes"), "<p>caf\u{e9}</p>");

        let utf8_bom = b"\xEF\xBB\xBF<p>x</p>";
        assert_eq!(decode_member_text(utf8_bom).expect("utf-8 decodes"), "<p>x</p>");
    }

    #[test]
    fn should_reject_bytes_that_are_not_text() {
        let declared_utf8 = b"<?xml version=\"1.0\" encoding=\"utf-8\"?><p>\x80\xFF\x00\xC3</p>";
        let error = decode_member_text(declared_utf8).expect_err("invalid UTF-8 under a UTF-8 declaration is not text");
        assert!(error.to_string().contains("Failed to decode"));
    }

    #[test]
    fn should_list_encrypted_members_but_not_obfuscated_fonts() {
        let xml = r#"<encryption xmlns="urn:oasis:names:tc:opendocument:xmlns:container" xmlns:enc="http://www.w3.org/2001/04/xmlenc#">
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/>
    <enc:CipherData><enc:CipherReference URI="OEBPS/fonts/a.otf"/></enc:CipherData>
  </enc:EncryptedData>
  <enc:EncryptedData>
    <enc:EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/>
    <enc:CipherData><enc:CipherReference URI="OEBPS/c1.xhtml"/></enc:CipherData>
  </enc:EncryptedData>
</encryption>"#;
        let encrypted = parse_encrypted_members(xml);
        assert_eq!(encrypted.into_iter().collect::<Vec<_>>(), vec!["OEBPS/c1.xhtml"]);
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

    /// Deliberate behaviour change from the path-traversal unification: a backslash in an
    /// href is now normalised to `/` (matching PPTX/DOCX target handling) instead of being
    /// treated as an ordinary character inside one literal path segment. Real EPUB hrefs are
    /// URLs and never contain a literal backslash, so this only changes how a malformed href
    /// resolves -- it does not affect any well-formed package.
    #[test]
    fn test_resolve_path_normalises_backslashes_like_forward_slashes() {
        let result = resolve_path("OEBPS", "sub\\chapter.xhtml").expect("path should resolve");
        assert_eq!(result.path, "OEBPS/sub/chapter.xhtml");
    }

    /// New hardening from the path-traversal unification: a NUL byte can never appear in a
    /// legitimate ZIP entry name, so it is now rejected outright rather than passed through
    /// to a `by_name` lookup that would simply miss. Observationally inert for any real EPUB.
    #[test]
    fn test_resolve_path_rejects_nul_byte() {
        let err = resolve_path("OEBPS", "chapter\0.xhtml").expect_err("path should be rejected");
        assert!(err.to_string().contains("NUL byte"));
    }

    /// Absolute paths from the OPC/EPUB convention (root-relative) still resolve when the
    /// base directory has more than one path segment -- proving the shared helper's
    /// root-relative branch does not accidentally retain any part of a multi-segment base.
    #[test]
    fn test_resolve_path_absolute_ignores_a_multi_segment_base() {
        let result = resolve_path("OEBPS/text/nested", "/images/cover.png").expect("path should resolve");
        assert_eq!(result.path, "images/cover.png");
    }
}
