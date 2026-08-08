//! Regression tests for issues #127 and #140 (EPUB extractor).
//!
//! #127: `extractors/epub/mod.rs`'s structural-node walk dropped `DefinitionItem`,
//! `Citation`, `Admonition`, `Footnote`, `Title`, and `PageBreak` node kinds into a
//! catch-all `_ => {}`, and separately dropped the `Quote` container marker via
//! `NodeContent::Quote => continue`. All are now handled explicitly.
//! #140: `extractors/epub/content.rs`'s plain-text fallback walker skipped `<svg>`
//! subtrees (and therefore their real `<title>`/`<desc>` alt-text) and `<object>`/
//! `<embed>`/`<iframe>` fallback content entirely.
//!
//! Requires `--features office` (EPUB is gated behind it).

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::ExtractInput;
use xberg::core::config::ExtractionConfig;
use xberg::extractors::EpubExtractor;
use xberg::plugins::DocumentExtractor;
use zip::write::FileOptions;

/// Build a minimal single-chapter EPUB, following the pattern in
/// `epub_mathml_extractor_tests.rs`.
fn build_epub_bytes(chapter_xhtml: &str) -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Issue Test Book</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="c1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="c1"/>
  </spine>
</package>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = zip::ZipWriter::new(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

    writer.start_file("mimetype", options).expect("zip start_file failed");
    writer
        .write_all(b"application/epub+zip")
        .expect("zip write mimetype failed");
    writer
        .add_directory("META-INF/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/content.opf", opf_xml),
        ("OEBPS/chapter1.xhtml", chapter_xhtml),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

async fn extract(chapter_xhtml: &str) -> String {
    let bytes = build_epub_bytes(chapter_xhtml);
    let extractor = EpubExtractor;
    let config = ExtractionConfig::default();
    let input = ExtractInput::from_bytes(bytes, "application/epub+zip", None);
    extractor
        .extract(input, &config)
        .await
        .expect("epub extraction should succeed")
        .content
}

/// #127: a `<dl>/<dt>/<dd>` definition list survives (both term and definition text),
/// exercising the structural-walker `NodeContent::DefinitionItem` arm.
#[tokio::test]
async fn issue_127_definition_list_preserved() {
    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <body>
    <h1>Glossary</h1>
    <dl>
      <dt>DEFTERM</dt>
      <dd>DEFDESCRIPTION explaining the term.</dd>
    </dl>
  </body>
</html>"#;
    let content = extract(chapter_xhtml).await;
    assert!(content.contains("DEFTERM"), "definition term dropped: {content}");
    assert!(
        content.contains("DEFDESCRIPTION"),
        "definition description dropped: {content}"
    );
}

/// #127: a `<blockquote>` still has its (previously-dropped) container markers recorded,
/// and its quoted text survives.
#[tokio::test]
async fn issue_127_blockquote_content_preserved() {
    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <body>
    <p>Before the quote.</p>
    <blockquote><p>QUOTEDTEXT inside the blockquote.</p></blockquote>
    <p>After the quote.</p>
  </body>
</html>"#;
    let content = extract(chapter_xhtml).await;
    assert!(content.contains("QUOTEDTEXT"), "quoted text dropped: {content}");
    assert!(content.contains("Before the quote"));
    assert!(content.contains("After the quote"));
}

/// #140: an inline `<svg>`'s `<title>` (real accessible alt-text) is recovered instead of
/// the whole subtree being skipped.
#[tokio::test]
async fn issue_140_svg_title_recovered() {
    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <body>
    <p>Diagram below.</p>
    <svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 10 10">
      <title>SVGTITLEMARKER accessible description</title>
      <rect width="10" height="10"/>
    </svg>
  </body>
</html>"#;
    let content = extract(chapter_xhtml).await;
    assert!(
        content.contains("SVGTITLEMARKER"),
        "SVG title alt-text dropped: {content}"
    );
    assert!(
        !content.to_ascii_lowercase().contains("<rect"),
        "raw SVG markup should not leak into content: {content}"
    );
}

/// #140: `<object>` fallback content (rendered when the referenced resource can't be
/// displayed) is recovered instead of the whole element being skipped.
#[tokio::test]
async fn issue_140_object_fallback_text_recovered() {
    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <body>
    <object data="unsupported.bin" type="application/octet-stream">
      <p>OBJECTFALLBACKMARKER: unsupported content.</p>
    </object>
  </body>
</html>"#;
    let content = extract(chapter_xhtml).await;
    assert!(
        content.contains("OBJECTFALLBACKMARKER"),
        "object fallback text dropped: {content}"
    );
}
