//! EPUB extraction hardening tests.
//!
//! Each test builds a minimal EPUB in memory and checks one behaviour that
//! earlier releases got wrong: dropped chapters, silent failures, and lost
//! metadata.

#![cfg(feature = "office")]

use std::io::{Cursor, Write};
use xberg::core::config::{ExtractionConfig, OutputFormat};
use xberg::extractors::EpubExtractor;
use xberg::plugins::InternalDocumentExtractor;
use xberg::types::internal::{ElementKind, InternalDocument};
use zip::write::FileOptions;

const CONTAINER_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<container version="1.0" xmlns="urn:oasis:names:tc:opendocument:xmlns:container">
  <rootfiles>
    <rootfile full-path="OEBPS/content.opf" media-type="application/oebps-package+xml"/>
  </rootfiles>
</container>"#;

fn chapter(body: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml"><head><title>Chapter</title></head><body>{body}</body></html>"#
    )
}

/// Build an EPUB with the given manifest items (`(id, href, media-type, extra attrs)`)
/// placed in the spine in order, plus arbitrary members.
fn build_epub(metadata: &str, items: &[(&str, &str, &str, &str)], members: &[(&str, &[u8])]) -> Vec<u8> {
    let manifest = items
        .iter()
        .map(|(id, href, media_type, extra)| {
            format!(r#"<item id="{id}" href="{href}" media-type="{media_type}" {extra}/>"#)
        })
        .collect::<String>();
    let spine = items
        .iter()
        .map(|(id, _, _, _)| format!(r#"<itemref idref="{id}"/>"#))
        .collect::<String>();
    let opf = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<package version="3.0" unique-identifier="bookid" xmlns="http://www.idpf.org/2007/opf">
  <metadata xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:opf="http://www.idpf.org/2007/opf">{metadata}</metadata>
  <manifest>{manifest}</manifest>
  <spine>{spine}</spine>
</package>"#
    );

    let mut cursor = Cursor::new(Vec::<u8>::new());
    {
        let mut writer = zip::ZipWriter::new(&mut cursor);
        let options = FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        writer.start_file("mimetype", options).expect("zip start_file failed");
        writer.write_all(b"application/epub+zip").expect("zip write failed");
        writer.start_file("META-INF/container.xml", options).expect("zip start_file failed");
        writer.write_all(CONTAINER_XML.as_bytes()).expect("zip write failed");
        writer.start_file("OEBPS/content.opf", options).expect("zip start_file failed");
        writer.write_all(opf.as_bytes()).expect("zip write failed");
        for (path, bytes) in members {
            writer.start_file(*path, options).expect("zip start_file failed");
            writer.write_all(bytes).expect("zip write failed");
        }
        writer.finish().expect("zip finish failed");
    }
    cursor.into_inner()
}

const BASIC_METADATA: &str = r#"<dc:title>Hardening Test</dc:title><dc:language>en</dc:language>"#;

async fn extract(bytes: &[u8], config: &ExtractionConfig) -> InternalDocument {
    EpubExtractor
        .extract_content(bytes, "application/epub+zip", config)
        .await
        .expect("extraction failed")
}

fn plain_text(document: &InternalDocument) -> String {
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

fn markdown_config() -> ExtractionConfig {
    ExtractionConfig {
        output_format: OutputFormat::Markdown,
        ..ExtractionConfig::default()
    }
}

// ---- #1489: spine gating -------------------------------------------------

#[tokio::test]
async fn test_link_list_chapters_are_kept_in_plain_and_markdown_output() {
    let further_reading = chapter(
        r#"<h1>Further Reading</h1><ul><li><a href="http://a">Smith, Alpha</a></li><li><a href="http://b">Doe, Beta</a></li></ul>"#,
    );
    let body = chapter("<p>Real chapter one text.</p>");
    let bytes = build_epub(
        BASIC_METADATA,
        &[
            ("c1", "c1.xhtml", "application/xhtml+xml", ""),
            ("bib", "bib.xhtml", "application/xhtml+xml", ""),
        ],
        &[("OEBPS/c1.xhtml", body.as_bytes()), ("OEBPS/bib.xhtml", further_reading.as_bytes())],
    );

    let plain = extract(&bytes, &ExtractionConfig::default()).await;
    let text = plain_text(&plain);
    assert!(text.contains("Real chapter one text."), "got: {text}");
    assert!(text.contains("Smith, Alpha"), "link-list chapter dropped from plain output: {text}");

    let markdown = extract(&bytes, &markdown_config()).await;
    let text = plain_text(&markdown);
    assert!(text.contains("Smith, Alpha"), "link-list chapter dropped from markdown output: {text}");
}

#[tokio::test]
async fn test_nav_property_excludes_the_navigation_document_without_a_heuristic() {
    let nav = chapter(
        r#"<h1>Contents</h1><p>Intro paragraph.</p><p>Another.</p><p>Third.</p><ol><li><a href="c1.xhtml">One</a></li><li><a href="c2.xhtml">Two</a></li></ol>"#,
    );
    let body = chapter("<p>Body text.</p>");
    let bytes = build_epub(
        BASIC_METADATA,
        &[
            ("nav", "nav.xhtml", "application/xhtml+xml", r#"properties="nav""#),
            ("c1", "c1.xhtml", "application/xhtml+xml", ""),
        ],
        &[("OEBPS/nav.xhtml", nav.as_bytes()), ("OEBPS/c1.xhtml", body.as_bytes())],
    );

    let document = extract(&bytes, &ExtractionConfig::default()).await;
    let text = plain_text(&document);
    assert!(text.contains("Body text."), "got: {text}");
    assert!(!text.contains("Intro paragraph."), "nav document leaked into content: {text}");
}

#[tokio::test]
async fn test_image_only_spine_page_emits_its_image() {
    let cover_page = chapter(r#"<div><img src="cover.png" alt="Front cover"/></div>"#);
    let body = chapter("<p>Body text.</p>");
    let png = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 0];
    let bytes = build_epub(
        BASIC_METADATA,
        &[
            ("cover", "cover.xhtml", "application/xhtml+xml", ""),
            ("c1", "c1.xhtml", "application/xhtml+xml", ""),
        ],
        &[
            ("OEBPS/cover.xhtml", cover_page.as_bytes()),
            ("OEBPS/cover.png", &png),
            ("OEBPS/c1.xhtml", body.as_bytes()),
        ],
    );

    let document = extract(&bytes, &ExtractionConfig::default()).await;
    assert_eq!(document.images.len(), 1, "cover page image was not extracted");
    assert_eq!(document.images[0].description.as_deref(), Some("Front cover"));
    assert!(plain_text(&document).contains("Body text."));
}

#[tokio::test]
async fn test_svg_spine_document_text_is_extracted() {
    let svg = r#"<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><title>Plate one</title><text x="1" y="1">SVG spine text</text></svg>"#;
    let body = chapter("<p>Body text.</p>");
    let bytes = build_epub(
        BASIC_METADATA,
        &[
            ("plate", "plate.svg", "image/svg+xml", ""),
            ("c1", "c1.xhtml", "application/xhtml+xml", ""),
        ],
        &[("OEBPS/plate.svg", svg.as_bytes()), ("OEBPS/c1.xhtml", body.as_bytes())],
    );

    let document = extract(&bytes, &ExtractionConfig::default()).await;
    let text = plain_text(&document);
    assert!(text.contains("SVG spine text"), "got: {text}");
    assert!(document.processing_warnings.is_empty(), "got: {:?}", document.processing_warnings);
}

// ---- #1488: entities and byte order marks --------------------------------

#[tokio::test]
async fn test_chapter_with_html_entity_and_style_attribute_is_extracted() {
    let body = r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE html PUBLIC "-//W3C//DTD XHTML 1.1//EN" "http://www.w3.org/TR/xhtml11/DTD/xhtml11.dtd">
<html xmlns="http://www.w3.org/1999/xhtml"><head><link rel="stylesheet" href="s.css"/><title>HEADTITLE</title></head>
<body style="margin:0"><p style="text-indent:0">First&nbsp;para &mdash; dash</p><p>Second para</p></body></html>"#;
    let bytes = build_epub(
        BASIC_METADATA,
        &[("c1", "c1.xhtml", "application/xhtml+xml", "")],
        &[("OEBPS/c1.xhtml", body.as_bytes())],
    );

    for config in [ExtractionConfig::default(), markdown_config()] {
        let document = extract(&bytes, &config).await;
        let text = plain_text(&document);
        assert!(text.contains("First\u{a0}para \u{2014} dash"), "got: {text}");
        assert!(text.contains("Second para"), "got: {text}");
        assert!(!text.contains("HEADTITLE"), "head title leaked: {text}");
        assert!(document.processing_warnings.is_empty(), "got: {:?}", document.processing_warnings);
    }
}

#[tokio::test]
async fn test_chapter_with_byte_order_mark_is_extracted_without_the_prelude() {
    let mut body = b"\xEF\xBB\xBF".to_vec();
    body.extend_from_slice(chapter("<p>Bom body</p>").as_bytes());
    let bytes = build_epub(
        BASIC_METADATA,
        &[("c1", "c1.xhtml", "application/xhtml+xml", "")],
        &[("OEBPS/c1.xhtml", &body)],
    );

    for config in [ExtractionConfig::default(), markdown_config()] {
        let document = extract(&bytes, &config).await;
        let text = plain_text(&document);
        assert!(text.contains("Bom body"), "got: {text}");
        assert!(!text.contains("xml version"), "prelude leaked: {text}");
        assert!(!text.contains('\u{FEFF}'), "BOM leaked: {text:?}");
    }
}

// ---- #1490: depth ----------------------------------------------------------

#[tokio::test]
async fn test_deeply_nested_chapter_does_not_end_the_process() {
    let depth = 60_000;
    let mut inner = String::new();
    for _ in 0..depth {
        inner.push_str("<span>");
    }
    inner.push_str("deep");
    for _ in 0..depth {
        inner.push_str("</span>");
    }
    let body = chapter(&format!("<p>lead</p><p>{inner}</p><p>tail</p>"));
    let bytes = build_epub(
        BASIC_METADATA,
        &[("c1", "c1.xhtml", "application/xhtml+xml", "")],
        &[("OEBPS/c1.xhtml", body.as_bytes())],
    );

    for config in [ExtractionConfig::default(), markdown_config()] {
        let document = extract(&bytes, &config).await;
        assert!(plain_text(&document).contains("lead"));
    }
}

// ---- #1491: per-item failures ---------------------------------------------

#[tokio::test]
async fn test_iteration_limit_reports_skipped_spine_items() {
    let body = chapter("<p>Body text.</p>");
    let items: Vec<(String, String)> = (0..20).map(|i| (format!("c{i}"), format!("c{i}.xhtml"))).collect();
    let item_refs: Vec<(&str, &str, &str, &str)> = items
        .iter()
        .map(|(id, href)| (id.as_str(), href.as_str(), "application/xhtml+xml", ""))
        .collect();
    let members: Vec<(String, &[u8])> = items
        .iter()
        .map(|(_, href)| (format!("OEBPS/{href}"), body.as_bytes()))
        .collect();
    let member_refs: Vec<(&str, &[u8])> = members.iter().map(|(p, b)| (p.as_str(), *b)).collect();
    let bytes = build_epub(BASIC_METADATA, &item_refs, &member_refs);

    let config = ExtractionConfig {
        security_limits: Some(xberg::extractors::security::SecurityLimits {
            max_iterations: 60,
            ..Default::default()
        }),
        ..ExtractionConfig::default()
    };
    let document = extract(&bytes, &config).await;
    assert!(
        document
            .processing_warnings
            .iter()
            .any(|warning| warning.message.contains("Iteration limit reached")),
        "got: {:?}",
        document.processing_warnings
    );
}
