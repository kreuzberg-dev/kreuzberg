//! OPML parsing and content extraction.
//!
//! This module handles XML parsing, metadata extraction from the `<head>` section,
//! and recursive processing of `<outline>` elements in the `<body>` section.

use crate::Result;
use crate::extractors::security::SecurityBudget;
use crate::text::utf8_validation;
use crate::types::uri::ExtractedUri;
use ahash::AHashMap;
use std::borrow::Cow;

#[cfg(feature = "office")]
use roxmltree::Node;
use serde_json;

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
#[cfg(feature = "office")]
const OPML_WARNING_SOURCE: &str = "opml";

/// Extract OPML content and metadata from raw bytes.
///
/// Parses the XML document structure, extracts metadata from the `<head>` section,
/// and processes the outline hierarchy in the `<body>` section.
///
/// # Returns
///
/// A tuple containing:
/// - Extracted content as a String (outline hierarchy with indentation)
/// - Metadata HashMap with key-value pairs from the head section
#[cfg(feature = "office")]
pub(crate) fn extract_content_and_metadata(
    content: &[u8],
    budget: &mut SecurityBudget,
) -> Result<(String, AHashMap<Cow<'static, str>, serde_json::Value>)> {
    let doc = roxmltree::Document::parse(
        utf8_validation::from_utf8(content)
            .map_err(|e| crate::XbergError::Other(format!("Invalid UTF-8 in OPML: {}", e)))?,
    )
    .map_err(|e| crate::XbergError::Other(format!("Failed to parse OPML: {}", e)))?;

    let mut extracted_content = String::new();
    let mut metadata = AHashMap::new();

    if let Some(opml) = doc.root().children().find(|n| n.tag_name().name() == "opml") {
        budget.enter()?;
        if let Some(head) = opml.children().find(|n| n.tag_name().name() == "head") {
            budget.enter()?;
            extract_metadata_from_head(head, &mut metadata, budget)?;
            budget.leave();
        }

        if let Some(body) = opml.children().find(|n| n.tag_name().name() == "body") {
            budget.enter()?;
            if let Some(title) = metadata.get("title").and_then(|v| v.as_str()) {
                budget.account_text(title.len())?;
                extracted_content.push_str(title);
                extracted_content.push('\n');
                extracted_content.push('\n');
            }

            let mut feed_urls = Vec::new();
            for outline in body.children().filter(|n| n.tag_name().name() == "outline") {
                budget.step()?;
                process_outline(outline, 0, &mut extracted_content, budget)?;
                collect_feed_urls(outline, &mut feed_urls, budget)?;
            }

            if !feed_urls.is_empty() {
                metadata.insert(Cow::Borrowed("feed_urls"), serde_json::json!(feed_urls));
            }
            budget.leave();
        }
        budget.leave();
    }

    Ok((extracted_content.trim().to_string(), metadata))
}

/// Extract metadata from the OPML `<head>` section.
///
/// Extracts standard OPML metadata fields:
/// - title: The document title
/// - dateCreated: Creation date
/// - dateModified: Last modification date
/// - ownerName: Document owner's name
/// - ownerEmail: Document owner's email
#[cfg(feature = "office")]
fn extract_metadata_from_head(
    head: Node,
    metadata: &mut AHashMap<Cow<'static, str>, serde_json::Value>,
    budget: &mut SecurityBudget,
) -> Result<()> {
    for child in head.children().filter(|n| n.is_element()) {
        budget.step()?;
        let tag = child.tag_name().name();
        let text = child.text().unwrap_or("").trim();

        if text.is_empty() {
            continue;
        }

        budget.check_entity(text)?;
        budget.account_text(text.len())?;

        match tag {
            "title" => {
                metadata.insert(Cow::Borrowed("title"), serde_json::json!(text));
            }
            "dateCreated" => {
                metadata.insert(Cow::Borrowed("dateCreated"), serde_json::json!(text));
            }
            "dateModified" => {
                metadata.insert(Cow::Borrowed("dateModified"), serde_json::json!(text));
            }
            "ownerName" => {
                metadata.insert(Cow::Borrowed("ownerName"), serde_json::json!(text));
            }
            "ownerEmail" => {
                metadata.insert(Cow::Borrowed("ownerEmail"), serde_json::json!(text));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Process outline elements recursively.
///
/// Extracts text content from outline hierarchy while preserving nesting depth
/// through indentation. URL attributes are excluded from the main content.
///
/// # Arguments
///
/// * `node` - The outline node to process
/// * `depth` - Current nesting depth (for indentation)
/// * `output` - Output string buffer to append content to
#[cfg(feature = "office")]
pub(crate) fn process_outline(
    node: Node,
    depth: usize,
    output: &mut String,
    budget: &mut SecurityBudget,
) -> Result<()> {
    budget.enter()?;
    let text = node.attribute("text").unwrap_or("").trim();

    if !text.is_empty() {
        budget.check_entity(text)?;
        budget.account_text(text.len())?;
        let indent = "  ".repeat(depth);
        output.push_str(&indent);
        output.push_str(text);
        output.push('\n');
    }

    for child in node.children().filter(|n| n.tag_name().name() == "outline") {
        budget.step()?;
        process_outline(child, depth + 1, output, budget)?;
    }
    budget.leave();
    Ok(())
}

/// Recursively collect feed URLs (xmlUrl) from outline elements into metadata.
#[cfg(feature = "office")]
fn collect_feed_urls(node: Node, urls: &mut Vec<serde_json::Value>, budget: &mut SecurityBudget) -> Result<()> {
    if let Some(xml_url) = node.attribute("xmlUrl") {
        let trimmed = xml_url.trim();
        if !trimmed.is_empty() {
            budget.check_attr("xmlUrl", trimmed)?;
            let mut entry = serde_json::Map::new();
            entry.insert("xmlUrl".to_string(), serde_json::json!(trimmed));
            if let Some(text) = node.attribute("text") {
                let t = text.trim();
                if !t.is_empty() {
                    budget.check_attr("text", t)?;
                    entry.insert("text".to_string(), serde_json::json!(t));
                }
            }
            if let Some(html_url) = node.attribute("htmlUrl") {
                let h = html_url.trim();
                if !h.is_empty() {
                    budget.check_attr("htmlUrl", h)?;
                    entry.insert("htmlUrl".to_string(), serde_json::json!(h));
                }
            }
            if let Some(feed_type) = node.attribute("type") {
                let ft = feed_type.trim();
                if !ft.is_empty() {
                    entry.insert("type".to_string(), serde_json::json!(ft));
                }
            }
            urls.push(serde_json::Value::Object(entry));
        }
    }
    for child in node.children().filter(|n| n.tag_name().name() == "outline") {
        budget.step()?;
        collect_feed_urls(child, urls, budget)?;
    }
    Ok(())
}

/// Extract OPML outline attributes (xmlUrl, htmlUrl, type, description, _note) into a HashMap.
#[cfg(feature = "office")]
fn extract_outline_attributes(node: Node) -> AHashMap<String, String> {
    let mut attrs = AHashMap::new();
    for attr_name in &["xmlUrl", "htmlUrl", "type", "description", "_note"] {
        if let Some(val) = node.attribute(*attr_name) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                attrs.insert(attr_name.to_string(), trimmed.to_string());
            }
        }
    }
    attrs
}

/// Convert inline HTML tags in OPML text attributes to Markdown equivalents.
///
/// Handles:
/// - `<strong>text</strong>` / `<b>text</b>` -> `**text**`
/// - `<em>text</em>` / `<i>text</i>` -> `*text*`
/// - `<a href="url">text</a>` -> `[text](url)`
/// - Unescapes `\<` and `\>`
#[cfg(feature = "office")]
fn convert_inline_html(text: &str) -> String {
    use regex::Regex;

    let mut result = text.to_string();

    let strong_re = Regex::new(r"<(?:strong|b)>(.*?)</(?:strong|b)>").expect("valid regex");
    result = strong_re.replace_all(&result, "**$1**").into_owned();

    let em_re = Regex::new(r"<(?:em|i)>(.*?)</(?:em|i)>").expect("valid regex");
    result = em_re.replace_all(&result, "*$1*").into_owned();

    let link_re = Regex::new(r#"<a\s+href="([^"]*)"[^>]*>(.*?)</a>"#).expect("valid regex");
    result = link_re.replace_all(&result, "[$2]($1)").into_owned();

    result = result.replace(r"\<", "<").replace(r"\>", ">");

    result
}

/// Build an `InternalDocument` from OPML content.
///
/// Maps the outline hierarchy to headings and paragraphs, mirroring
/// `build_document_structure`.
#[cfg(feature = "office")]
pub(crate) fn build_internal_document(
    content: &[u8],
    budget: &mut SecurityBudget,
) -> Result<crate::types::internal::InternalDocument> {
    use crate::types::internal_builder::InternalDocumentBuilder;

    let doc = roxmltree::Document::parse(
        utf8_validation::from_utf8(content)
            .map_err(|e| crate::XbergError::Other(format!("Invalid UTF-8 in OPML: {}", e)))?,
    )
    .map_err(|e| crate::XbergError::Other(format!("Failed to parse OPML: {}", e)))?;

    let mut builder = InternalDocumentBuilder::new("opml");

    // Both lookups failing silently is total loss: the file parsed as XML, so extraction
    // returns `Ok`, but the document handed back is empty and the caller cannot tell that
    // from an outline file that genuinely has no entries (#171).
    let mut missing_structure: Option<&'static str> = None;
    match doc.root().children().find(|n| n.tag_name().name() == "opml") {
        Some(opml) => match opml.children().find(|n| n.tag_name().name() == "body") {
            Some(body) => {
                for outline in body.children().filter(|n| n.tag_name().name() == "outline") {
                    budget.step()?;
                    build_outline_internal(outline, 1, &mut builder, budget)?;
                }
            }
            None => missing_structure = Some("<body>"),
        },
        None => missing_structure = Some("<opml> root"),
    }

    let mut document = builder.build();
    if let Some(missing) = missing_structure {
        crate::core::diagnostics::push_warning(
            &mut document.processing_warnings,
            OPML_WARNING_SOURCE,
            format!(
                "The file parsed as XML but has no {missing} element, so no outline entries \
                 could be read and the extracted document is empty"
            ),
        );
    }

    Ok(document)
}

/// Recursively build internal document from outline nodes.
#[cfg(feature = "office")]
fn build_outline_internal(
    node: Node,
    depth: u8,
    builder: &mut crate::types::internal_builder::InternalDocumentBuilder,
    budget: &mut SecurityBudget,
) -> Result<()> {
    budget.enter()?;
    let text = node.attribute("text").unwrap_or("").trim();
    let note = node.attribute("_note").map(str::trim).filter(|n| !n.is_empty());

    let child_outlines: Vec<Node> = node.children().filter(|n| n.tag_name().name() == "outline").collect();

    // An outline with no `text` still carries meaningful content when it has a
    // `_note` (or other attributes/children); only truly empty nodes are skipped.
    if text.is_empty() {
        if let Some(note) = note {
            budget.check_attr("_note", note)?;
            budget.account_text(note.len())?;
            builder.push_paragraph(&convert_inline_html(note), Vec::new(), None, None);
        }
        for child in child_outlines {
            budget.step()?;
            build_outline_internal(child, depth, builder, budget)?;
        }
        budget.leave();
        return Ok(());
    }

    budget.check_entity(text)?;
    budget.account_text(text.len())?;

    let attrs = extract_outline_attributes(node);

    let label = Some(text.to_string());
    if let Some(xml_url) = node.attribute("xmlUrl") {
        let trimmed = xml_url.trim();
        if !trimmed.is_empty() {
            budget.check_attr("xmlUrl", trimmed)?;
            builder.push_uri(ExtractedUri::hyperlink(trimmed, label.clone()));
        }
    }
    if let Some(html_url) = node.attribute("htmlUrl") {
        let trimmed = html_url.trim();
        if !trimmed.is_empty() {
            budget.check_attr("htmlUrl", trimmed)?;
            builder.push_uri(ExtractedUri::hyperlink(trimmed, label));
        }
    }

    let level = depth.min(6);
    let converted_text = convert_inline_html(text);
    let idx = builder.push_heading(level, &converted_text, None, None);
    if !attrs.is_empty() {
        builder.set_attributes(idx, attrs);
    }
    for child in child_outlines {
        budget.step()?;
        build_outline_internal(child, depth + 1, builder, budget)?;
    }
    budget.leave();
    Ok(())
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;

    #[test]
    fn test_simple_outline_parsing() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Test</title>
  </head>
  <body>
    <outline text="Item 1" />
    <outline text="Item 2" />
  </body>
</opml>"#;

        let (content, metadata) =
            extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults()).expect("Should parse simple OPML");

        assert!(content.contains("Item 1"), "Should extract first item");
        assert!(content.contains("Item 2"), "Should extract second item");
        assert_eq!(
            metadata.get("title").and_then(|v| v.as_str()),
            Some("Test"),
            "Should extract title"
        );
    }

    #[test]
    fn test_nested_hierarchy() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Hierarchy Test</title>
  </head>
  <body>
    <outline text="Category">
      <outline text="Subcategory">
        <outline text="Item" />
      </outline>
    </outline>
  </body>
</opml>"#;

        let (content, _) =
            extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults()).expect("Should parse nested OPML");

        assert!(content.contains("Category"), "Should contain top level");
        assert!(content.contains("Subcategory"), "Should contain nested level");
        assert!(content.contains("Item"), "Should contain deep item");
        assert!(content.contains("  "), "Should have indentation for nested items");
    }

    #[test]
    fn test_rss_feeds() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Feeds</title>
  </head>
  <body>
    <outline text="Tech">
      <outline text="Hacker News" type="rss" xmlUrl="https://news.ycombinator.com/rss" htmlUrl="https://news.ycombinator.com/" />
      <outline text="TechCrunch" type="rss" xmlUrl="https://techcrunch.com/feed/" />
    </outline>
  </body>
</opml>"#;

        let (content, _) =
            extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults()).expect("Should parse RSS OPML");

        assert!(content.contains("Hacker News"), "Should extract feed title");
        assert!(
            !content.contains("https://"),
            "Should NOT extract feed URLs (text-only extraction)"
        );
        assert!(content.contains("TechCrunch"), "Should extract multiple feeds");
    }

    #[test]
    fn test_metadata_extraction() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>My Feeds</title>
    <dateCreated>Mon, 06 Nov 2023 00:00:00 GMT</dateCreated>
    <dateModified>Fri, 01 Dec 2023 12:00:00 GMT</dateModified>
    <ownerName>John Doe</ownerName>
    <ownerEmail>john@example.com</ownerEmail>
  </head>
  <body>
    <outline text="Item" />
  </body>
</opml>"#;

        let (_content, metadata) =
            extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults()).expect("Should extract metadata");

        assert_eq!(metadata.get("title").and_then(|v| v.as_str()), Some("My Feeds"));
        assert_eq!(metadata.get("ownerName").and_then(|v| v.as_str()), Some("John Doe"));
        assert_eq!(
            metadata.get("ownerEmail").and_then(|v| v.as_str()),
            Some("john@example.com")
        );
        assert!(metadata.contains_key("dateCreated"));
        assert!(metadata.contains_key("dateModified"));
    }

    #[test]
    fn test_with_special_characters() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Test &amp; Special</title>
  </head>
  <body>
    <outline text="Business &amp; Startups" />
    <outline text="Science &lt;Advanced&gt;" />
  </body>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle special characters");

        assert!(
            content.contains("Business") && content.contains("Startups"),
            "Should decode HTML entities"
        );
        let title = metadata.get("title").and_then(|v| v.as_str()).unwrap_or("");
        assert!(!title.is_empty(), "Should extract title");
    }

    #[test]
    fn test_empty_body() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Empty</title>
  </head>
  <body>
  </body>
</opml>"#;

        let (_content, metadata) =
            extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults()).expect("Should handle empty body");

        assert_eq!(metadata.get("title").and_then(|v| v.as_str()), Some("Empty"));
    }

    #[test]
    fn test_malformed_missing_closing_tag() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Broken</title>
  </head>
  <body>
    <outline text="Unclosed"
  </body>
</opml>"#;

        let result = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults());
        assert!(result.is_err(), "Should fail to parse OPML with missing closing tags");
    }

    #[test]
    fn test_malformed_invalid_nesting() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Invalid Nesting</title>
  <body>
    <outline text="Item" />
  </body>
</opml>"#;

        let result = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults());
        assert!(result.is_err(), "Should fail to parse OPML with invalid nesting");
    }

    #[test]
    fn test_empty_outline_elements() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Empty Outlines</title>
  </head>
  <body>
    <outline text="" />
    <outline />
    <outline text="Valid Item">
      <outline text="" />
      <outline text="Another Valid" />
    </outline>
  </body>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle empty outline elements");

        assert!(content.contains("Valid Item"), "Should extract valid items");
        assert!(content.contains("Another Valid"), "Should extract nested valid items");
        let empty_count = content.matches("\n\n").count();
        assert!(empty_count < 3, "Should skip empty outline elements");

        assert_eq!(metadata.get("title").and_then(|v| v.as_str()), Some("Empty Outlines"));
    }

    #[test]
    fn test_deeply_nested_empty_nodes() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Deep Nesting</title>
  </head>
  <body>
    <outline text="Level 1">
      <outline text="">
        <outline text="">
          <outline text="Deep Item">
            <outline text="" />
          </outline>
        </outline>
      </outline>
      <outline text="Level 1 Sibling" />
    </outline>
  </body>
</opml>"#;

        let (content, _) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle deeply nested structures");

        assert!(content.contains("Level 1"), "Should extract top-level item");
        assert!(content.contains("Deep Item"), "Should extract deeply nested item");
        assert!(content.contains("Level 1 Sibling"), "Should extract sibling items");
    }

    #[test]
    fn test_outline_with_missing_text_attribute() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Missing Attributes</title>
  </head>
  <body>
    <outline type="folder" />
    <outline text="Valid Item" type="rss" />
    <outline type="rss" xmlUrl="https://example.com/feed" />
  </body>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle outline with missing text attribute");

        assert!(content.contains("Valid Item"), "Should extract item with text");
        assert!(!content.contains("https://"), "Should not extract URLs");

        assert_eq!(
            metadata.get("title").and_then(|v| v.as_str()),
            Some("Missing Attributes")
        );
    }

    #[test]
    fn test_whitespace_only_text_attribute() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Whitespace Test</title>
  </head>
  <body>
    <outline text="   " />
    <outline text="
" />
    <outline text="Real Content" />
  </body>
</opml>"#;

        let (content, _) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle whitespace-only text");

        assert!(
            content.contains("Real Content"),
            "Should extract non-whitespace content"
        );
        let trimmed = content.trim();
        assert!(trimmed.contains("Whitespace Test"), "Should have title");
        assert!(trimmed.contains("Real Content"), "Should have real content");
    }

    #[test]
    fn test_html_entity_in_nested_structure() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Entities &amp; Nesting</title>
  </head>
  <body>
    <outline text="News &amp; Updates">
      <outline text="Tech &lt; Science" />
      <outline text="Health &gt; Wealth" />
    </outline>
  </body>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle HTML entities");

        assert!(
            content.contains("News") && content.contains("Updates"),
            "Should decode &amp; entity"
        );
        assert!(content.contains("Tech"), "Should handle &lt; entity");
        assert!(content.contains("Science"), "Should decode entity properly");

        let title = metadata.get("title").and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            title.contains("&") && title.contains("Nesting"),
            "Title should have decoded entity"
        );
    }

    #[test]
    fn test_single_outline_no_children() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Single</title>
  </head>
  <body>
    <outline text="Only Item" />
  </body>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle single outline");

        assert!(content.contains("Only Item"), "Should extract single item");
        assert_eq!(metadata.get("title").and_then(|v| v.as_str()), Some("Single"));
    }

    #[test]
    fn test_head_without_body() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>No Body</title>
  </head>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle OPML without body");

        assert_eq!(metadata.get("title").and_then(|v| v.as_str()), Some("No Body"));
        assert!(content.is_empty() || content.trim() == "No Body");
    }

    #[test]
    fn test_feed_url_extraction() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <head>
    <title>Feeds</title>
  </head>
  <body>
    <outline text="Tech">
      <outline text="Hacker News" type="rss" xmlUrl="https://news.ycombinator.com/rss" htmlUrl="https://news.ycombinator.com/" />
      <outline text="TechCrunch" type="rss" xmlUrl="https://techcrunch.com/feed/" />
    </outline>
  </body>
</opml>"#;

        let (_content, metadata) =
            extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults()).expect("Should parse RSS OPML");

        let feed_urls = metadata.get("feed_urls");
        assert!(feed_urls.is_some(), "Should have feed_urls metadata");
        let urls_array = feed_urls
            .expect("feed_urls key should be present")
            .as_array()
            .expect("feed_urls should be an array");
        assert_eq!(urls_array.len(), 2, "Should have 2 feed URLs");

        let first_feed = &urls_array[0];
        assert_eq!(
            first_feed
                .get("text")
                .expect("text key")
                .as_str()
                .expect("text should be string"),
            "Hacker News"
        );
        assert_eq!(
            first_feed
                .get("xmlUrl")
                .expect("xmlUrl key")
                .as_str()
                .expect("xmlUrl should be string"),
            "https://news.ycombinator.com/rss"
        );
        assert_eq!(
            first_feed
                .get("htmlUrl")
                .expect("htmlUrl key")
                .as_str()
                .expect("htmlUrl should be string"),
            "https://news.ycombinator.com/"
        );
        assert_eq!(
            first_feed
                .get("type")
                .expect("type key")
                .as_str()
                .expect("type should be string"),
            "rss"
        );
    }

    #[test]
    fn test_body_without_head() {
        let opml = br#"<?xml version="1.0"?>
<opml version="2.0">
  <body>
    <outline text="Item" />
  </body>
</opml>"#;

        let (content, metadata) = extract_content_and_metadata(opml, &mut SecurityBudget::with_defaults())
            .expect("Should handle OPML without head");

        assert!(content.contains("Item"), "Should extract body content");
        assert!(metadata.is_empty(), "Should have no metadata without head");
    }
}
