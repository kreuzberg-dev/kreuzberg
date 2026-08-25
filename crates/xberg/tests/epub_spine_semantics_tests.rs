//! EPUB integration tests.
//!
//! These tests validate EPUB-specific spine and navigation semantics.

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::core::config::{ExtractionConfig, OutputFormat};
use xberg::extractors::EpubExtractor;
use xberg::plugins::InternalDocumentExtractor;
use xberg::types::internal::{ElementKind, InternalDocument};
use zip::write::FileOptions;

fn content(document: &InternalDocument) -> String {
    if let Some(content) = &document.pre_rendered_content {
        return content.clone();
    }

    document
        .elements
        .iter()
        .filter(|element| {
            !matches!(
                element.kind,
                ElementKind::ListStart { .. }
                    | ElementKind::ListEnd
                    | ElementKind::QuoteStart
                    | ElementKind::QuoteEnd
                    | ElementKind::GroupStart
                    | ElementKind::GroupEnd
                    | ElementKind::PageBreak
                    | ElementKind::Image { .. }
                    | ElementKind::Table { .. }
            )
        })
        .map(|element| element.text.as_str())
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn start_epub_writer(cursor: &mut Cursor<Vec<u8>>) -> zip::ZipWriter<&mut Cursor<Vec<u8>>> {
    let mut writer = zip::ZipWriter::new(cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);

    writer.start_file("mimetype", options).expect("zip start_file failed");
    writer
        .write_all(b"application/epub+zip")
        .expect("zip write mimetype failed");
    writer
        .add_directory("META-INF/", options)
        .expect("zip add_directory failed");

    writer
}

fn build_epub3_with_navigation_and_auxiliary_spine_items() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Spine Semantics Test Book</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="intro" href="intro.xhtml" media-type="application/xhtml+xml"/>
    <item id="nav" href="nav.xhtml" media-type="application/xhtml+xml" properties="nav"/>
    <item id="chapter1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
    <item id="appendix" href="appendix.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="intro"/>
    <itemref idref="nav"/>
    <itemref idref="chapter1"/>
    <itemref idref="appendix" linear="no"/>
  </spine>
</package>"#;

    let intro_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Intro</title></head>
  <body>
    <h1>Intro</h1>
    <p>Opening paragraph.</p>
  </body>
</html>"#;

    let nav_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Table of Contents</title></head>
  <body>
    <p>Reading note outside navigation.</p>
    <nav epub:type="toc" xmlns:epub="http://www.idpf.org/2007/ops">
      <h1>Table of Contents</h1>
      <ol>
        <li><a href="chapter1.xhtml">Chapter One</a></li>
      </ol>
    </nav>
  </body>
</html>"#;

    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Chapter One</title></head>
  <body>
    <h1>Chapter One</h1>
    <p>Main chapter text.</p>
  </body>
</html>"#;

    let appendix_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Appendix</title></head>
  <body>
    <h1>Appendix</h1>
    <p>Auxiliary back matter.</p>
  </body>
</html>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/content.opf", opf_xml),
        ("OEBPS/intro.xhtml", intro_xhtml),
        ("OEBPS/nav.xhtml", nav_xhtml),
        ("OEBPS/chapter1.xhtml", chapter_xhtml),
        ("OEBPS/appendix.xhtml", appendix_xhtml),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

fn build_epub2_with_guide_toc_in_spine() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="2.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>EPUB 2 TOC Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="toc-page" href="toc.xhtml" media-type="application/xhtml+xml"/>
    <item id="chapter1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
    <item id="appendix" href="appendix.xhtml" media-type="application/xhtml+xml"/>
    <item id="ncx" href="toc.ncx" media-type="application/x-dtbncx+xml"/>
  </manifest>
  <spine toc="ncx">
    <itemref idref="toc-page"/>
    <itemref idref="chapter1"/>
    <itemref idref="appendix" linear="no"/>
  </spine>
  <guide>
    <reference type="toc" title="Contents" href="./text/../toc.xhtml#toc"/>
  </guide>
</package>"#;

    let toc_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Contents</title></head>
  <body>
    <h1 id="toc">Contents</h1>
    <ol>
      <li><a href="chapter1.xhtml">Chapter One</a></li>
      <li><a href="appendix.xhtml">Appendix</a></li>
    </ol>
  </body>
</html>"#;

    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Chapter One</title></head>
  <body>
    <h1>Chapter One</h1>
    <p>Main chapter text.</p>
  </body>
</html>"#;

    let appendix_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Appendix</title></head>
  <body>
    <h1>Appendix</h1>
    <p>Supplemental material.</p>
  </body>
</html>"#;

    let ncx = r#"<?xml version="1.0" encoding="UTF-8"?>
<ncx xmlns="http://www.daisy.org/z3986/2005/ncx/" version="2005-1">
  <navMap>
    <navPoint id="chapter1" playOrder="1">
      <navLabel><text>Chapter One</text></navLabel>
      <content src="chapter1.xhtml"/>
    </navPoint>
  </navMap>
</ncx>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/content.opf", opf_xml),
        ("OEBPS/toc.xhtml", toc_xhtml),
        ("OEBPS/chapter1.xhtml", chapter_xhtml),
        ("OEBPS/appendix.xhtml", appendix_xhtml),
        ("OEBPS/toc.ncx", ncx),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

fn build_epub_with_fallback_content_document() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/package/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Fallback Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="chapter-foreign" href="../art/chapter.svg" media-type="image/svg+xml" fallback="chapter-xhtml"/>
    <item id="chapter-xhtml" href="../text/chapter.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="chapter-foreign"/>
  </spine>
</package>"#;

    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Fallback Chapter</title></head>
  <body>
    <h1>Fallback Chapter</h1>
    <p>Resolved through manifest fallback.</p>
  </body>
</html>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/package/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/text/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/art/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/package/content.opf", opf_xml),
        ("OEBPS/text/chapter.xhtml", chapter_xhtml),
        (
            "OEBPS/art/chapter.svg",
            "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        ),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

fn build_epub_with_root_escaping_manifest_href() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/package/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Invalid Path Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="chapter1" href="../../../chapter.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
    <itemref idref="chapter1"/>
  </spine>
</package>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/package/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/package/content.opf", opf_xml),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

fn build_epub_with_unused_invalid_manifest_asset() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/package/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Unused Asset Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="chapter1" href="../text/chapter.xhtml" media-type="application/xhtml+xml"/>
    <item id="unused-cover" href="../../../../cover.png" media-type="image/png"/>
  </manifest>
  <spine>
    <itemref idref="chapter1"/>
  </spine>
</package>"#;

    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Chapter One</title></head>
  <body>
    <h1>Chapter One</h1>
    <p>Main chapter text.</p>
  </body>
</html>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/package/", options)
        .expect("zip add_directory failed");
    writer
        .add_directory("OEBPS/text/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/package/content.opf", opf_xml),
        ("OEBPS/text/chapter.xhtml", chapter_xhtml),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

#[tokio::test]
async fn test_epub3_excludes_navigation_but_keeps_non_linear_spine_content() {
    let bytes = build_epub3_with_navigation_and_auxiliary_spine_items();
    let extractor = EpubExtractor;
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &config)
        .await
        .expect("EPUB extraction should succeed");

    assert!(
        result.processing_warnings.is_empty(),
        "Expected no warnings, got: {:?}",
        result.processing_warnings
    );
    assert!(
        !content(&result).contains("?xml version"),
        "XML declarations should not leak into Markdown output:\n{}",
        content(&result)
    );
    assert!(
        !content(&result).contains("Table of Contents"),
        "Navigation documents should not be rendered as body content:\n{}",
        content(&result)
    );
    assert!(
        content(&result).contains("Reading note outside navigation."),
        "Expected prose outside specialized nav content to be preserved:\n{}",
        content(&result)
    );
    assert!(content(&result).contains("# Intro"), "Expected intro heading");
    assert!(content(&result).contains("# Chapter One"), "Expected chapter heading");
    assert!(
        content(&result).contains("# Appendix"),
        "Non-linear spine content should still be extracted:\n{}",
        content(&result)
    );
    assert!(
        content(&result).contains("Auxiliary back matter."),
        "Expected non-linear appendix text in extracted content"
    );
}

#[tokio::test]
async fn test_epub3_plain_output_excludes_specialized_navigation_but_keeps_body_prose() {
    let bytes = build_epub3_with_navigation_and_auxiliary_spine_items();
    let extractor = EpubExtractor;

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &ExtractionConfig::default())
        .await
        .expect("EPUB extraction should succeed");

    assert!(
        content(&result).contains("Reading note outside navigation."),
        "Expected prose outside specialized nav content to be preserved:\n{}",
        content(&result)
    );
    assert!(
        !content(&result).contains("Table of Contents"),
        "Specialized navigation content should stay out of plain-text extraction:\n{}",
        content(&result)
    );
    assert!(
        content(&result).contains("Main chapter text."),
        "Expected real chapter body content in plain-text extraction:\n{}",
        content(&result)
    );
    assert!(
        content(&result).contains("Auxiliary back matter."),
        "Expected non-linear spine prose to remain in plain-text extraction:\n{}",
        content(&result)
    );
}

#[tokio::test]
async fn test_epub_document_structure_excludes_navigation_but_keeps_non_linear_spine_content() {
    let bytes = build_epub3_with_navigation_and_auxiliary_spine_items();
    let extractor = EpubExtractor;
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        include_document_structure: true,
        ..Default::default()
    };

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &config)
        .await
        .expect("EPUB extraction should succeed");

    let all_text = content(&result);

    assert!(
        all_text.contains("Intro"),
        "Expected intro content in document structure"
    );
    assert!(
        all_text.contains("Chapter One"),
        "Expected chapter content in document structure"
    );
    assert!(
        all_text.contains("Reading note outside navigation."),
        "Expected non-nav prose from the navigation document in document structure"
    );
    assert!(
        all_text.contains("Appendix"),
        "Expected non-linear appendix content in document structure"
    );
    assert!(
        !all_text.contains("Table of Contents"),
        "Navigation documents should be excluded from document structure:\n{}",
        all_text
    );
}

#[tokio::test]
async fn test_epub2_guide_toc_document_is_excluded_but_auxiliary_content_remains() {
    let bytes = build_epub2_with_guide_toc_in_spine();
    let extractor = EpubExtractor;

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &ExtractionConfig::default())
        .await
        .expect("EPUB extraction should succeed");

    assert!(
        !content(&result).contains("Contents"),
        "EPUB 2 guide TOC document should not be rendered as body content:\n{}",
        content(&result)
    );
    assert!(
        content(&result).contains("Chapter One"),
        "Expected main chapter content in EPUB 2 extraction"
    );
    assert!(
        content(&result).contains("Supplemental material."),
        "Expected non-linear appendix content in EPUB 2 extraction"
    );
}

#[tokio::test]
async fn test_epub_ignores_invalid_unused_manifest_assets_when_body_content_is_valid() {
    let bytes = build_epub_with_unused_invalid_manifest_asset();
    let extractor = EpubExtractor;

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &ExtractionConfig::default())
        .await
        .expect("EPUB extraction should succeed");

    assert!(
        content(&result).contains("Chapter One"),
        "Expected valid spine content to be extracted"
    );
    assert!(
        content(&result).contains("Main chapter text."),
        "Expected chapter body text to be extracted"
    );
}

#[tokio::test]
async fn test_epub_manifest_fallback_resolves_renderable_body_document() {
    let bytes = build_epub_with_fallback_content_document();
    let extractor = EpubExtractor;
    let config = ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..Default::default()
    };

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &config)
        .await
        .expect("EPUB extraction should succeed");

    assert!(
        result.processing_warnings.is_empty(),
        "Expected fallback resolution without warnings, got: {:?}",
        result.processing_warnings
    );
    assert!(
        content(&result).contains("# Fallback Chapter"),
        "Expected heading from fallback XHTML document:\n{}",
        content(&result)
    );
    assert!(
        content(&result).contains("Resolved through manifest fallback."),
        "Expected body text from fallback XHTML document"
    );
}

#[tokio::test]
async fn test_epub_skips_manifest_paths_that_escape_package_root_with_a_warning() {
    let bytes = build_epub_with_root_escaping_manifest_href();
    let extractor = EpubExtractor;

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &ExtractionConfig::default())
        .await
        .expect("one unsafe spine href must not fail the whole book");

    assert!(content(&result).trim().is_empty(), "the escaping item must not be read");
    assert!(
        result
            .processing_warnings
            .iter()
            .any(|warning| warning.message.contains("escapes the package root")),
        "Expected a root-escape warning, got: {:?}",
        result.processing_warnings
    );
}

fn build_epub_with_manifest_fallback_cycle() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Fallback Cycle Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="item-a" href="a.svg" media-type="image/svg+xml" fallback="item-b"/>
    <item id="item-b" href="b.svg" media-type="image/svg+xml" fallback="item-a"/>
  </manifest>
  <spine>
    <itemref idref="item-a"/>
  </spine>
</package>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .add_directory("OEBPS/", options)
        .expect("zip add_directory failed");

    for (path, contents) in [
        ("META-INF/container.xml", container_xml),
        ("OEBPS/content.opf", opf_xml),
        ("OEBPS/a.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
        ("OEBPS/b.svg", "<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
    ] {
        writer.start_file(path, options).expect("zip start_file failed");
        writer.write_all(contents.as_bytes()).expect("zip write file failed");
    }

    writer.finish().expect("zip finish failed");
    cursor.into_inner()
}

fn build_epub_with_empty_spine() -> Vec<u8> {
    let container_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

    let opf_xml = r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
    <dc:title>Empty Spine Test</dc:title>
    <dc:language>en</dc:language>
  </metadata>
  <manifest>
    <item id="chapter1" href="chapter1.xhtml" media-type="application/xhtml+xml"/>
  </manifest>
  <spine>
  </spine>
</package>"#;

    let chapter_xhtml = r#"<?xml version="1.0" encoding="UTF-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
  <head><title>Chapter One</title></head>
  <body>
    <h1>Chapter One</h1>
    <p>This content is in the manifest but not in the spine.</p>
  </body>
</html>"#;

    let mut cursor = Cursor::new(Vec::<u8>::new());
    let mut writer = start_epub_writer(&mut cursor);
    let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
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

#[tokio::test]
async fn test_epub_manifest_fallback_cycle_produces_warning_without_panic() {
    let bytes = build_epub_with_manifest_fallback_cycle();
    let extractor = EpubExtractor;

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &ExtractionConfig::default())
        .await
        .expect("EPUB extraction should not panic on fallback cycles");

    let has_cycle_warning = result
        .processing_warnings
        .iter()
        .any(|w| w.message.contains("fallback cycle"));
    assert!(
        has_cycle_warning,
        "Expected a warning about fallback cycle, got warnings: {:?}",
        result.processing_warnings
    );
}

#[tokio::test]
async fn test_epub_empty_spine_produces_empty_content_without_error() {
    let bytes = build_epub_with_empty_spine();
    let extractor = EpubExtractor;

    let result = extractor
        .extract_content(&bytes, "application/epub+zip", &ExtractionConfig::default())
        .await
        .expect("EPUB extraction should succeed with empty spine");

    assert!(
        content(&result).trim().is_empty(),
        "Expected empty or whitespace-only content for empty spine, got: '{}'",
        content(&result)
    );
}
