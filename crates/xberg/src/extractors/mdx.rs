//! MDX extractor with JSX stripping and frontmatter support.
//!
//! MDX is a superset of Markdown that adds JSX support (imports, exports,
//! JSX components, and inline expressions). This extractor strips MDX-specific
//! syntax and then processes the remaining content as standard Markdown.
//!
//! Requires the `mdx` feature (which includes `pulldown-cmark`).

use super::frontmatter_utils::{
    extract_frontmatter_with_warning, extract_metadata_from_yaml, extract_title_from_content,
};
use super::markdown::{MarkdownExtractor, markdown_options};
use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::Metadata;
use crate::types::internal::InternalDocument;
use async_trait::async_trait;
use pulldown_cmark::{Event, Parser};
use regex::Regex;
use std::sync::LazyLock;

/// Regex matching JSX component tags (capitalized tag names).
/// Matches opening tags like `<Component prop="value">`, closing tags like `</Component>`,
/// and self-closing tags like `<Component />`.
static JSX_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"</?[A-Z][a-zA-Z0-9_.]*(?:\s[^>]*)?>|<[A-Z][a-zA-Z0-9_.]*(?:\s[^>]*)?\s*/>").unwrap());

/// Regex matching standalone JSX expression lines like `{expression}` or `{/* comment */}`.
static JSX_EXPR_LINE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\{.*\}\s*$").unwrap());

/// Regex matching inline JSX comments like `{/* ... */}`.
static JSX_INLINE_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s*\{/\*.*?\*/\}").unwrap());

/// Regex matching inline JSX expressions embedded in prose, e.g. `The count is {count}
/// today.` or a self-closing component's props left dangling after tag stripping. Must
/// start with a JS-identifier character so it does not swallow Pandoc/Quarto heading-
/// attribute or fenced-div syntax such as `{#id}` or `{.class}`. See issue #142.
static INLINE_JSX_EXPR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[A-Za-z_$][^{}]*\}").unwrap());

/// MDX extractor with JSX stripping and Markdown processing.
///
/// Strips MDX-specific syntax (imports, exports, JSX component tags,
/// inline expressions) and processes the remaining content as Markdown,
/// extracting metadata from YAML frontmatter and tables.
#[cfg_attr(alef, alef(skip))]
pub struct MdxExtractor;

impl MdxExtractor {
    /// Create a new MDX extractor.
    pub(crate) fn new() -> Self {
        Self
    }

    /// Strip MDX-specific syntax from content, preserving standard Markdown.
    ///
    /// Removes:
    /// - `import` statements (single and multi-line)
    /// - `export` statements (single and multi-line)
    /// - JSX component tags (capitalized: `<Component>`, `</Component>`, `<Component />`)
    /// - Standalone JSX expression lines (`{expression}`, `{/* comment */}`)
    ///
    /// Preserves:
    /// - Content inside code fences (``` blocks)
    /// - Standard HTML tags (lowercase: `<div>`, `<p>`, etc.)
    /// - Text content between JSX component tags
    #[cfg(test)]
    pub(crate) fn strip_mdx_syntax(content: &str) -> String {
        Self::strip_mdx_syntax_collecting(content, None)
    }

    /// Strip MDX syntax, optionally collecting stripped JSX blocks into `jsx_blocks`.
    fn strip_mdx_syntax_collecting(content: &str, mut jsx_blocks: Option<&mut Vec<String>>) -> String {
        let mut result = String::with_capacity(content.len());
        let mut in_code_fence = false;
        let mut skip_block_depth: i32 = 0;

        for line in content.lines() {
            let trimmed = line.trim();

            if trimmed.starts_with("```") {
                in_code_fence = !in_code_fence;
                result.push_str(line);
                result.push('\n');
                continue;
            }

            if in_code_fence {
                result.push_str(line);
                result.push('\n');
                continue;
            }

            if skip_block_depth > 0 {
                skip_block_depth += count_braces(trimmed);
                if skip_block_depth <= 0 {
                    skip_block_depth = 0;
                }
                continue;
            }

            if trimmed.starts_with("import ") || trimmed == "import" {
                let depth = count_braces(trimmed);
                if depth > 0 {
                    skip_block_depth = depth;
                }
                continue;
            }

            if trimmed.starts_with("export ") || trimmed == "export" {
                let depth = count_braces(trimmed);
                if depth > 0 {
                    skip_block_depth = depth;
                }
                continue;
            }

            // A standalone `{expression}` line is recorded as a raw JSX block before being
            // dropped, rather than silently discarded. See #142.
            //
            // A JSX *comment* is excluded: it carries no document content, and recording it
            // puts `{/* ... */}` straight back into the rendered output through the raw-block
            // element — which is precisely what stripping exists to prevent.
            if JSX_EXPR_LINE_RE.is_match(trimmed) {
                if let Some(ref mut blocks) = jsx_blocks
                    && !JSX_INLINE_COMMENT_RE.replace_all(trimmed, "").trim().is_empty()
                {
                    blocks.push(trimmed.to_string());
                }
                continue;
            }

            let without_comments = JSX_INLINE_COMMENT_RE.replace_all(line, "");

            // Component tags (with their props) are recorded as raw JSX blocks at match
            // time, not only when stripping empties the whole line — an inline component
            // such as `See <Chart data={data} type="line" /> below.` used to lose its props
            // entirely because the surrounding prose kept the line non-empty. See #142.
            if let Some(ref mut blocks) = jsx_blocks {
                for m in JSX_TAG_RE.find_iter(&without_comments) {
                    blocks.push(m.as_str().to_string());
                }
            }

            let processed = JSX_TAG_RE.replace_all(&without_comments, "");
            let processed_trimmed = processed.trim();

            if processed_trimmed.is_empty() && !trimmed.is_empty() {
                continue;
            }

            // Inline JSX expressions left over in prose after tag/comment stripping (e.g.
            // `The count is {count} today.`) are likewise recorded rather than dropped.
            let processed = if let Some(ref mut blocks) = jsx_blocks {
                for m in INLINE_JSX_EXPR_RE.find_iter(&processed) {
                    blocks.push(m.as_str().to_string());
                }
                INLINE_JSX_EXPR_RE.replace_all(&processed, "")
            } else {
                processed
            };

            result.push_str(&processed);
            result.push('\n');
        }

        result
    }
}

/// Count net brace depth change in a line (opening `{` minus closing `}`).
fn count_braces(line: &str) -> i32 {
    let mut depth: i32 = 0;
    for ch in line.chars() {
        match ch {
            '{' => depth += 1,
            '}' => depth -= 1,
            _ => {}
        }
    }
    depth
}

impl Default for MdxExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for MdxExtractor {
    fn name(&self) -> &str {
        "mdx-extractor"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    fn description(&self) -> &str {
        "Extracts content from MDX files by stripping JSX syntax and processing as Markdown"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for MdxExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let _ = config;
        let text = String::from_utf8_lossy(content).into_owned();

        let (yaml, remaining_content, frontmatter_warning) = extract_frontmatter_with_warning(&text);

        let mut metadata = if let Some(ref yaml_value) = yaml {
            extract_metadata_from_yaml(yaml_value)
        } else {
            Metadata::default()
        };

        let mut jsx_blocks_buf = Some(Vec::new());
        let clean_markdown = Self::strip_mdx_syntax_collecting(&remaining_content, jsx_blocks_buf.as_mut());

        if metadata.title.is_none()
            && let Some(title) = extract_title_from_content(&clean_markdown)
        {
            metadata.title = Some(title);
        }

        // Use the same full pulldown-cmark option set as the Markdown extractor (issue
        // #123) — previously MDX enabled only tables/strikethrough/footnotes, so math, GFM
        // alerts, definition lists, task lists, super/subscript, heading attributes, smart
        // punctuation, and wikilinks were all silently unsupported in `.mdx` files.
        let parser = Parser::new_ext(&clean_markdown, markdown_options());
        let events: Vec<Event> = parser.collect();

        let raw_jsx = jsx_blocks_buf.unwrap_or_default();

        // Images (including data-URI decoding) and every other event kind are handled by the
        // single shared builder also used by the Markdown extractor (issue #273) — this used
        // to be a ~470-line copy-paste fork that had drifted and silently dropped math,
        // inline HTML, superscript/subscript, and definition lists in `.mdx` files.
        let mut doc = MarkdownExtractor::build_internal_document_with_jsx(&events, &yaml, &raw_jsx);
        doc.mime_type = mime_type.to_string();
        doc.metadata = metadata;
        doc.processing_warnings.extend(frontmatter_warning);

        Ok(doc)
    }

    async fn extract_path(
        &self,
        path: &std::path::Path,
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        crate::core::path_resolver::extract_file_with_image_resolution(self, path, mime_type, config).await
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["text/mdx", "text/x-mdx"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_import_statements() {
        let input = "import { Chart } from './Chart'\nimport Alert from './Alert'\n\n# Hello\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("import"));
        assert!(result.contains("# Hello"));
    }

    #[test]
    fn test_strip_multiline_import() {
        let input = "import {\n  Chart,\n  Table,\n} from './components'\n\n# Hello\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("import"));
        assert!(!result.contains("Chart"));
        assert!(result.contains("# Hello"));
    }

    #[test]
    fn test_strip_export_statements() {
        let input = "export const meta = { title: 'Hello' }\n\n# Hello\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("export"));
        assert!(result.contains("# Hello"));
    }

    #[test]
    fn test_strip_multiline_export() {
        let input = "export const meta = {\n  title: 'Hello',\n  date: '2024-01-01',\n}\n\n# Hello\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("export"));
        assert!(!result.contains("title"));
        assert!(result.contains("# Hello"));
    }

    #[test]
    fn test_strip_export_default() {
        let input = "export default function Layout({ children }) { return children }\n\n# Hello\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("export"));
        assert!(result.contains("# Hello"));
    }

    #[test]
    fn test_strip_jsx_component_tags() {
        let input = "# Hello\n\n<Alert type=\"warning\">\nBe careful!\n</Alert>\n\nMore text.\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("<Alert"));
        assert!(!result.contains("</Alert>"));
        assert!(result.contains("Be careful!"));
        assert!(result.contains("More text."));
    }

    #[test]
    fn test_strip_self_closing_jsx() {
        let input = "# Hello\n\n<Chart data={data} />\n\nSome text.\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("<Chart"));
        assert!(result.contains("Some text."));
    }

    #[test]
    fn test_strip_jsx_expression_lines() {
        let input = "# Hello\n\n{/* This is a comment */}\n\n{someExpression}\n\nText.\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("comment"));
        assert!(!result.contains("someExpression"));
        assert!(result.contains("Text."));
    }

    #[test]
    fn test_preserve_code_fences() {
        let input =
            "# Hello\n\n```jsx\nimport React from 'react'\nconst x = <Component />\nexport default App\n```\n\nText.\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(result.contains("import React from 'react'"));
        assert!(result.contains("<Component />"));
        assert!(result.contains("export default App"));
        assert!(result.contains("Text."));
    }

    #[test]
    fn test_preserve_standard_html_tags() {
        let input = "# Hello\n\n<div>Some content</div>\n\n<p>Paragraph</p>\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(result.contains("<div>"));
        assert!(result.contains("</div>"));
        assert!(result.contains("<p>"));
    }

    #[test]
    fn test_preserve_markdown_content() {
        let input = "# Title\n\nThis is **bold** and *italic* text.\n\n- Item 1\n- Item 2\n\n> Blockquote\n";
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(result.contains("# Title"));
        assert!(result.contains("**bold**"));
        assert!(result.contains("*italic*"));
        assert!(result.contains("- Item 1"));
        assert!(result.contains("> Blockquote"));
    }

    #[test]
    fn test_strip_complex_mdx() {
        let input = r#"import { Chart } from './Chart'
import Alert from './Alert'

export const meta = {
  title: 'My Post',
  date: '2024-01-01',
}

# My Post

This is a paragraph with **bold** text.

<Alert type="warning">
  Be careful with this!
</Alert>

<Chart data={data} />

{/* A comment */}

Some more text.

```javascript
const x = <div>Not JSX</div>
```

Final paragraph.
"#;
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("import"));
        assert!(!result.contains("export"));
        assert!(!result.contains("<Alert"));
        assert!(!result.contains("<Chart"));
        assert!(!result.contains("comment"));
        assert!(result.contains("# My Post"));
        assert!(result.contains("**bold**"));
        assert!(result.contains("Be careful with this!"));
        assert!(result.contains("Some more text."));
        assert!(result.contains("const x = <div>Not JSX</div>"));
        assert!(result.contains("Final paragraph."));
    }

    #[test]
    fn test_empty_content() {
        let result = MdxExtractor::strip_mdx_syntax("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_plugin_interface() {
        let extractor = MdxExtractor::new();
        assert_eq!(extractor.name(), "mdx-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert_eq!(extractor.supported_mime_types(), &["text/mdx", "text/x-mdx"]);
    }

    #[tokio::test]
    async fn test_extract_mdx_basic() {
        let content = b"import Chart from './Chart'\n\n# Hello World\n\nThis is content.\n";
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract MDX content");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.mime_type, "text/mdx");
        assert!(result.content.contains("Hello World"));
        assert!(result.content.contains("This is content"));
        assert!(!result.content.contains("import"));
    }

    #[tokio::test]
    async fn test_extract_mdx_with_frontmatter() {
        let content = b"---\ntitle: My MDX Post\nauthor: Test Author\ndate: 2024-01-15\n---\n\nimport Alert from './Alert'\n\n# Content\n\nBody text.\n";
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract MDX with frontmatter");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.metadata.title.as_deref(), Some("My MDX Post"));
        assert_eq!(result.metadata.created_by.as_deref(), Some("Test Author"));
        assert!(result.content.contains("Body text"));
        assert!(!result.content.contains("import"));
    }

    #[tokio::test]
    async fn test_extract_mdx_with_jsx_components() {
        let content = b"# Title\n\n<Alert type=\"warning\">\nImportant message!\n</Alert>\n\nRegular text.\n";
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract MDX with JSX components");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("Important message"));
        assert!(result.content.contains("Regular text"));
        // NOTE: JSX tags appear in content because they are stored as RawBlock elements
    }

    #[tokio::test]
    async fn test_extract_mdx_with_tables() {
        let content = b"# Tables\n\n| Header 1 | Header 2 |\n|----------|----------|\n| Cell 1   | Cell 2   |\n";
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract MDX with tables");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(!result.tables.is_empty());
        let table = &result.tables[0];
        assert_eq!(table.cells[0].len(), 2);
    }

    /// Regression test: `build_internal_document` used to create an `ElementKind::Image`
    /// placeholder with a sentinel `image_index: u32::MAX` for every image, then a separate pass
    /// in `extract_content` pushed the actual bytes via the raw, non-index-patching
    /// `InternalDocument::push_image`. The placeholder's index was never fixed up, so every
    /// renderer (which looks images up by walking `ElementKind::Image` elements) silently
    /// dropped the image from rendered output even though `doc.images` had the data. Images are
    /// now decoded and pushed in a single step with a correct index.
    #[tokio::test]
    async fn test_mdx_data_uri_image_renders_in_output() {
        let png_b64 =
            "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8/5+hHgAHggJ/PchI7wAAAABJRU5ErkJggg==";
        let content = format!("Intro.\n\n![a photo](data:image/png;base64,{png_b64})\n\nOutro.\n");

        let extractor = MdxExtractor::new();
        let doc = extractor
            .extract_content(content.as_bytes(), "text/mdx", &ExtractionConfig::default())
            .await
            .expect("extraction should succeed");

        assert_eq!(doc.images.len(), 1);
        let image_element_count = doc
            .elements
            .iter()
            .filter(|e| matches!(e.kind, crate::types::internal::ElementKind::Image { .. }))
            .count();
        assert_eq!(
            image_element_count, 1,
            "expected one Image element in {:?}",
            doc.elements
        );

        let markdown = crate::rendering::render_markdown(&doc);
        assert!(
            markdown.contains("a photo"),
            "image description missing from rendered markdown: {markdown}"
        );
    }

    #[tokio::test]
    async fn test_extract_mdx_title_from_heading() {
        let content = b"# My Document Title\n\nContent here.\n";
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract title from heading");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.metadata.title, Some("My Document Title".to_string()));
    }

    #[test]
    fn test_count_braces_balanced() {
        assert_eq!(count_braces("{ a: 1 }"), 0);
    }

    #[test]
    fn test_count_braces_opening() {
        assert_eq!(count_braces("const x = {"), 1);
    }

    #[test]
    fn test_count_braces_closing() {
        assert_eq!(count_braces("}"), -1);
    }

    #[test]
    fn test_count_braces_nested() {
        assert_eq!(count_braces("{ a: { b: 1 }"), 1);
    }

    #[test]
    fn test_count_braces_none() {
        assert_eq!(count_braces("no braces here"), 0);
    }

    /// Helper: load a test document from the test_documents directory.
    fn load_test_doc(relative_path: &str) -> Vec<u8> {
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let path = std::path::Path::new(manifest_dir)
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_documents")
            .join(relative_path);
        std::fs::read(&path).unwrap_or_else(|e| panic!("Failed to read {}: {}", path.display(), e))
    }

    #[tokio::test]
    async fn test_extract_real_world_getting_started() {
        let content = load_test_doc("markdown/mdx_getting_started.mdx");
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(&content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract getting-started.mdx");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("Getting started"), "Missing main heading");

        assert!(
            result.content.contains("how to integrate MDX into your project"),
            "Missing introductory text"
        );

        assert!(
            result.content.contains("Prerequisites"),
            "Missing Prerequisites section"
        );
        assert!(result.content.contains("Quick start"), "Missing Quick start section");
        assert!(result.content.contains("Bundler"), "Missing Bundler section");
        assert!(result.content.contains("Security"), "Missing Security section");
        assert!(result.content.contains("Integrations"), "Missing Integrations section");

        assert!(result.content.contains("React"), "Missing React mention");
        assert!(result.content.contains("webpack"), "Missing webpack mention");
        assert!(result.content.contains("esbuild"), "Missing esbuild mention");

        assert!(
            !result.content.contains("import {Note}"),
            "import statement not stripped"
        );
        assert!(
            !result.content.contains("export const info"),
            "export const info not stripped"
        );
        assert!(
            !result.content.contains("export const navSortSelf"),
            "export const navSortSelf not stripped"
        );

        // NOTE: JSX component tags appear in content as RawBlock element text from the derive pipeline.

        assert!(!result.content.contains("{/* more */}"), "JSX comment not stripped");

        assert!(
            result.content.contains("npm install @types/mdx"),
            "Code block content should be preserved"
        );

        assert!(
            result.content.len() > 2000,
            "Extracted content too short: {} chars",
            result.content.len()
        );
    }

    #[tokio::test]
    async fn test_extract_real_world_using_mdx() {
        let content = load_test_doc("markdown/mdx_using_mdx.mdx");
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(&content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract using-mdx.mdx");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("Using MDX"), "Missing main heading");

        assert!(
            result.content.contains("How MDX works"),
            "Missing 'How MDX works' section"
        );
        assert!(result.content.contains("MDX content"), "Missing 'MDX content' section");
        assert!(result.content.contains("Props"), "Missing Props section");
        assert!(result.content.contains("Components"), "Missing Components section");
        assert!(result.content.contains("Layout"), "Missing Layout section");
        assert!(result.content.contains("MDX provider"), "Missing MDX provider section");

        assert!(!result.content.contains("import {Note}"), "import not stripped");
        assert!(!result.content.contains("export const info"), "export not stripped");

        // NOTE: JSX component tags appear in content as RawBlock element text from the derive pipeline.

        assert!(
            result.content.len() > 2000,
            "Extracted content too short: {} chars",
            result.content.len()
        );
    }

    #[tokio::test]
    async fn test_extract_real_world_troubleshooting() {
        let content = load_test_doc("markdown/mdx_troubleshooting.mdx");
        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(&content, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should extract troubleshooting-mdx.mdx");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("Troubleshooting MDX"), "Missing main heading");

        assert!(
            result.content.contains("Problems integrating MDX"),
            "Missing integrating section"
        );
        assert!(result.content.contains("ESM"), "Missing ESM section");
        assert!(result.content.contains("Problems using MDX"), "Missing using section");
        assert!(
            result.content.contains("Problems writing MDX"),
            "Missing writing section"
        );

        assert!(!result.content.contains("import {Note}"), "import not stripped");
        assert!(!result.content.contains("export const info"), "export not stripped");

        assert!(!result.content.contains("{/* lint disable"), "JSX comment not stripped");

        // NOTE: JSX component tags appear in content as RawBlock element text from the derive pipeline.

        assert!(
            result.content.contains("Had trouble with something"),
            "Content inside <Note> should be preserved"
        );

        assert!(
            result.content.len() > 2000,
            "Extracted content too short: {} chars",
            result.content.len()
        );
    }

    #[tokio::test]
    async fn test_strip_mdx_real_world_multiline_exports() {
        let input = r#"import {Note} from '../_component/note.jsx'

export const info = {
  author: [
    {github: 'wooorm', name: 'Titus Wormer'}
  ],
  modified: new Date('2025-01-27'),
  published: new Date('2021-10-05')
}
export const navSortSelf = 2

# Getting started

Content here.
"#;
        let result = MdxExtractor::strip_mdx_syntax(input);
        assert!(!result.contains("import"), "import not stripped");
        assert!(!result.contains("export"), "export not stripped");
        assert!(!result.contains("wooorm"), "Nested export content not stripped");
        assert!(!result.contains("navSortSelf"), "Single-line export not stripped");
        assert!(result.contains("# Getting started"), "Heading should be preserved");
        assert!(result.contains("Content here"), "Content should be preserved");
    }

    #[tokio::test]
    async fn test_trimmed_paragraph_with_emoji_mdx() {
        let mdx = b"  **bold** \xf0\x9f\x8e\x89 text  ";

        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(mdx, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should handle emoji in trimmed MDX paragraph");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("bold"), "Bold text preserved");
        assert!(result.content.contains("\u{1F389}"), "Emoji preserved after trim");
    }

    #[tokio::test]
    async fn test_cjk_paragraph_with_formatting_mdx() {
        let mdx = "# CJK\n\nこれは**太字**テスト".as_bytes();

        let extractor = MdxExtractor::new();
        let result = extractor
            .extract_content(mdx, "text/mdx", &ExtractionConfig::default())
            .await
            .expect("Should handle CJK with bold formatting");
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert!(result.content.contains("太字"), "Bold CJK content present");
        assert!(result.content.contains("これは"), "Leading CJK preserved");
    }
}
