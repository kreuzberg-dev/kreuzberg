//! Jupyter Notebook extractor for .ipynb files.
//!
//! This extractor provides native Rust parsing of Jupyter notebooks,
//! extracting:
//! - Notebook metadata (kernelspec, language_info, nbformat)
//! - Cell content (markdown and code cells in order)
//! - Cell outputs (text, HTML, images)
//! - Cell metadata (execution_count, tags)
//!
//! Requires the `notebook` feature.

#[cfg(feature = "notebook")]
use crate::Result;
#[cfg(feature = "notebook")]
use crate::core::config::{ExtractionConfig, JupyterCellRendering};
#[cfg(feature = "notebook")]
use crate::extractors::security::SecurityBudget;
#[cfg(feature = "notebook")]
use crate::plugins::{InternalDocumentExtractor, Plugin};
#[cfg(feature = "notebook")]
use crate::types::ProcessingWarning;
#[cfg(feature = "notebook")]
use crate::types::internal::InternalDocument;
#[cfg(feature = "notebook")]
use crate::types::internal_builder::InternalDocumentBuilder;
#[cfg(feature = "notebook")]
use crate::types::{ExtractedImage, Metadata};
#[cfg(feature = "notebook")]
use ahash::AHashMap;
#[cfg(feature = "notebook")]
use async_trait::async_trait;
#[cfg(feature = "notebook")]
use base64::Engine;
#[cfg(feature = "notebook")]
use bytes::Bytes;
#[cfg(feature = "notebook")]
use serde_json::{Value, json};
#[cfg(feature = "notebook")]
use std::borrow::Cow;

#[cfg(feature = "notebook")]
type NotebookContent = (
    String,
    AHashMap<Cow<'static, str>, Value>,
    Vec<ExtractedImage>,
    Value,
    Vec<ProcessingWarning>,
    Vec<ExtractedImage>,
);

/// Image MIME types recognized in output `data` bundles and cell
/// `attachments`, in the order they are checked. Raster formats are listed
/// first (matching the pre-#160 behavior for outputs), with `image/svg+xml`
/// appended last since it requires different handling (raw markup rather
/// than base64-decoded raster bytes) for outputs, though attachments always
/// decode base64 regardless of mimetype.
#[cfg(feature = "notebook")]
const SUPPORTED_IMAGE_MIME_TYPES: &[&str] = &["image/png", "image/jpeg", "image/gif", "image/webp", "image/svg+xml"];

/// Jupyter Notebook extractor.
///
/// Extracts content from Jupyter notebook JSON files, including:
/// - Notebook metadata (kernel, language, nbformat version)
/// - Cell content (code and markdown)
/// - Cell outputs (text, HTML, etc.)
/// - Cell-level metadata (tags, execution counts)
#[cfg_attr(alef, alef(skip))]
#[cfg(feature = "notebook")]
pub struct JupyterExtractor;

/// One output's richest text-bearing representation, tagged with how it
/// must be rendered.
#[cfg(feature = "notebook")]
enum OutputRepresentation {
    /// Raw HTML markup. Must be emitted verbatim (not as a normal
    /// paragraph) because backslash-escaping markdown special characters
    /// (as paragraph rendering does) would corrupt the tags.
    RawHtml(String),
    /// Plain prose, from `text/markdown` or `text/plain`. Rendered as an
    /// ordinary paragraph, subject to the document's usual escaping.
    Text(String),
}

#[cfg(feature = "notebook")]
impl JupyterExtractor {
    /// Create a new Jupyter extractor.
    pub(crate) fn new() -> Self {
        Self
    }

    /// Extract content from a Jupyter notebook.
    fn extract_notebook(content: &[u8], plain: bool) -> Result<NotebookContent> {
        let notebook: Value = serde_json::from_slice(content)
            .map_err(|e| crate::XbergError::parsing(format!("Failed to parse JSON: {}", e)))?;

        let mut extracted_content = String::new();
        let mut metadata = AHashMap::new();
        let mut images = Vec::new();
        let mut attachment_images = Vec::new();
        let mut warnings = Vec::new();

        if let Some(notebook_metadata) = notebook.get("metadata").and_then(|m| m.as_object()) {
            if let Some(kernelspec) = notebook_metadata.get("kernelspec") {
                metadata.insert(Cow::Borrowed("kernelspec"), kernelspec.clone());
            }

            if let Some(language_info) = notebook_metadata.get("language_info") {
                metadata.insert(Cow::Borrowed("language_info"), language_info.clone());

                if let Some(obj) = language_info.as_object() {
                    if let Some(name) = obj.get("name") {
                        metadata.insert(Cow::Borrowed("language_name"), name.clone());
                    }
                    if let Some(version) = obj.get("version") {
                        metadata.insert(Cow::Borrowed("language_version"), version.clone());
                    }
                    if let Some(mimetype) = obj.get("mimetype") {
                        metadata.insert(Cow::Borrowed("language_mimetype"), mimetype.clone());
                    }
                }
            }
        }

        if let Some(nbformat) = notebook.get("nbformat") {
            metadata.insert(Cow::Borrowed("nbformat"), nbformat.clone());
        }
        if let Some(nbformat_minor) = notebook.get("nbformat_minor") {
            metadata.insert(Cow::Borrowed("nbformat_minor"), nbformat_minor.clone());
        }

        if let Some(cells) = notebook.get("cells").and_then(|c| c.as_array()) {
            metadata.insert(Cow::Borrowed("cell_count"), json!(cells.len()));
        }

        if let Some(cells) = notebook.get("cells").and_then(|c| c.as_array()) {
            let mut cells_meta: Vec<Value> = Vec::with_capacity(cells.len());
            for (cell_idx, cell) in cells.iter().enumerate() {
                cells_meta.push(Self::cell_metadata(cell, cell_idx));

                Self::extract_cell(cell, cell_idx, &mut extracted_content, &mut images, plain)?;
                Self::extract_cell_attachments(cell, cell_idx, &mut attachment_images, &mut warnings);
            }
            metadata.insert(Cow::Borrowed("cells"), json!(cells_meta));
        }

        Ok((
            extracted_content,
            metadata,
            images,
            notebook,
            warnings,
            attachment_images,
        ))
    }

    /// Extract a markdown/raw cell's `attachments` map into `images`.
    ///
    /// Per nbformat, `attachments` is a map of filename -> {mimetype: base64
    /// data}, referenced from cell source via `attachment:<filename>` URIs.
    /// Only markdown and raw cells carry this field. Each attachment is
    /// expected to carry exactly one mimetype; if none of the mimetypes on
    /// an attachment are a supported image type, a `ProcessingWarning`
    /// naming the attachment file is recorded instead of silently dropping
    /// it.
    fn extract_cell_attachments(
        cell: &Value,
        cell_idx: usize,
        images: &mut Vec<ExtractedImage>,
        warnings: &mut Vec<ProcessingWarning>,
    ) {
        let Some(attachments) = cell.get("attachments").and_then(|a| a.as_object()) else {
            return;
        };

        for (filename, mime_map) in attachments {
            let Some(mime_map) = mime_map.as_object() else {
                continue;
            };

            let supported = SUPPORTED_IMAGE_MIME_TYPES
                .iter()
                .find_map(|mime_type| mime_map.get(*mime_type).map(|value| (*mime_type, value)));

            let Some((mime_type, value)) = supported else {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("jupyter"),
                    message: Cow::Owned(format!(
                        "Cell {} attachment '{}' has no supported image MIME type; skipped",
                        cell_idx, filename
                    )),
                });
                continue;
            };

            let base64_str = Self::extract_source(value);
            let cleaned = base64_str.replace(['\n', '\r'], "");
            let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&cleaned) else {
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("jupyter"),
                    message: Cow::Owned(format!(
                        "Cell {} attachment '{}' could not be base64-decoded; skipped",
                        cell_idx, filename
                    )),
                });
                continue;
            };

            let format = mime_type.trim_start_matches("image/").replace("svg+xml", "svg");
            let (image_kind, kind_confidence) =
                crate::extraction::image_kind::classify(&decoded, &format, None, None, None, None, false);

            images.push(ExtractedImage {
                data: Bytes::from(decoded),
                format: Cow::Owned(format),
                image_index: images.len() as u32,
                page_number: Some((cell_idx + 1) as u32),
                width: None,
                height: None,
                colorspace: None,
                bits_per_component: None,
                is_mask: false,
                description: Some(format!("Notebook cell {} attachment: {}", cell_idx, filename)),
                ocr_result: None,
                bounding_box: None,
                source_path: None,
                image_kind: Some(image_kind),
                kind_confidence: Some(kind_confidence),
                cluster_id: None,
                caption: None,
                qr_codes: None,
                data_base64: None,
            });
        }
    }

    fn cell_metadata(cell: &Value, cell_idx: usize) -> Value {
        let cell_type = cell
            .get("cell_type")
            .and_then(|value| value.as_str())
            .unwrap_or("unknown");
        let mut metadata = serde_json::Map::new();
        metadata.insert("index".into(), json!(cell_idx));
        metadata.insert("cell_type".into(), json!(cell_type));

        for key in ["id", "execution_count"] {
            if let Some(value) = cell.get(key) {
                metadata.insert(key.into(), value.clone());
            }
        }
        if let Some(tags) = cell
            .get("metadata")
            .and_then(|value| value.get("tags"))
            .and_then(|value| value.as_array())
            && !tags.is_empty()
        {
            metadata.insert("tags".into(), Value::Array(tags.clone()));
        }
        if let Some(outputs) = cell.get("outputs").and_then(|value| value.as_array())
            && !outputs.is_empty()
        {
            metadata.insert(
                "outputs".into(),
                Value::Array(outputs.iter().enumerate().map(Self::output_metadata).collect()),
            );
        }

        Value::Object(metadata)
    }

    fn output_metadata((output_idx, output): (usize, &Value)) -> Value {
        let mut metadata = serde_json::Map::new();
        metadata.insert("index".into(), json!(output_idx));
        if let Some(output_type) = output.get("output_type") {
            metadata.insert("output_type".into(), output_type.clone());
        }
        for key in ["name", "execution_count", "ename", "evalue"] {
            if let Some(value) = output.get(key) {
                metadata.insert(key.into(), value.clone());
            }
        }
        if let Some(data) = output.get("data").and_then(|value| value.as_object()) {
            let mut mime_types: Vec<&str> = data.keys().map(String::as_str).collect();
            mime_types.sort_unstable();
            metadata.insert("mime_types".into(), json!(mime_types));
        }
        Value::Object(metadata)
    }

    /// Extract content from a single cell.
    fn extract_cell(
        cell: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain: bool,
    ) -> Result<()> {
        let cell_type = cell.get("cell_type").and_then(|t| t.as_str()).unwrap_or("unknown");

        match cell_type {
            "markdown" => Self::extract_markdown_cell(cell, content)?,
            "code" => Self::extract_code_cell(cell, cell_idx, content, images, plain)?,
            "raw" => Self::extract_raw_cell(cell, content)?,
            _ => {}
        }

        if !content.ends_with('\n') {
            content.push('\n');
        }
        content.push('\n');
        Ok(())
    }

    /// Extract markdown cell content.
    fn extract_markdown_cell(cell: &Value, content: &mut String) -> Result<()> {
        if let Some(source) = cell.get("source") {
            let cell_text = Self::extract_source(source);
            content.push_str(&cell_text);
        }
        Ok(())
    }

    /// Extract code cell content and outputs.
    fn extract_code_cell(
        cell: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain: bool,
    ) -> Result<()> {
        if let Some(source) = cell.get("source") {
            let cell_text = Self::extract_source(source);
            content.push_str(&cell_text);
            if !cell_text.ends_with('\n') {
                content.push('\n');
            }
        }

        if let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array()) {
            for output in outputs {
                Self::extract_output(output, cell_idx, content, images, plain)?;
            }
        }

        Ok(())
    }

    /// Extract raw cell content.
    fn extract_raw_cell(cell: &Value, content: &mut String) -> Result<()> {
        if let Some(source) = cell.get("source") {
            let cell_text = Self::extract_source(source);
            content.push_str(&cell_text);
        }
        Ok(())
    }

    /// Extract source content from various formats.
    ///
    /// Source can be either a string or an array of strings.
    fn extract_source(source: &Value) -> String {
        match source {
            Value::String(s) => s.clone(),
            Value::Array(arr) => arr.iter().filter_map(|v| v.as_str()).collect::<String>(),
            _ => String::new(),
        }
    }

    /// Extract output from a cell.
    fn extract_output(
        output: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain: bool,
    ) -> Result<()> {
        let output_type = output.get("output_type").and_then(|t| t.as_str()).unwrap_or("unknown");

        match output_type {
            "stream" => Self::extract_stream_output(output, content)?,
            "execute_result" | "display_data" | "update_display_data" => {
                Self::extract_data_output(output, cell_idx, content, images, plain)?;
            }
            "error" => Self::extract_error_output(output, content)?,
            _ => {}
        }

        Ok(())
    }

    /// Extract stream output (stdout, stderr).
    fn extract_stream_output(output: &Value, content: &mut String) -> Result<()> {
        if let Some(text) = output.get("text") {
            let text_content = Self::extract_source(text);
            content.push_str(&text_content);
        }

        Ok(())
    }

    /// Extract data output (execute_result or display_data).
    ///
    /// Prioritizes text/plain for quality scoring. For raster image types,
    /// decodes base64 data and populates the images collection.
    fn extract_data_output(
        output: &Value,
        cell_idx: usize,
        content: &mut String,
        images: &mut Vec<ExtractedImage>,
        plain_mode: bool,
    ) -> Result<()> {
        if let Some(data) = output.get("data").and_then(|d| d.as_object()) {
            if let Some(plain) = data.get("text/plain") {
                let text = Self::extract_source(plain);
                if !text.is_empty() {
                    content.push_str(&text);
                    if !text.ends_with('\n') {
                        content.push('\n');
                    }
                }
            }

            if !plain_mode {
                for mime_type in &["text/markdown", "text/html"] {
                    if let Some(mime_content) = data.get(*mime_type) {
                        let mime_text = Self::extract_source(mime_content);
                        if !mime_text.is_empty() {
                            content.push_str(&mime_text);
                            if !mime_text.ends_with('\n') {
                                content.push('\n');
                            }
                        }
                    }
                }
            }

            for mime_type in &["image/png", "image/jpeg", "image/gif", "image/webp"] {
                if let Some(image_value) = data.get(*mime_type) {
                    let base64_str = Self::extract_source(image_value);
                    let cleaned = base64_str.replace(['\n', '\r'], "");
                    if let Ok(decoded) = base64::engine::general_purpose::STANDARD.decode(&cleaned) {
                        let format = match *mime_type {
                            "image/png" => "png",
                            "image/jpeg" => "jpeg",
                            "image/gif" => "gif",
                            "image/webp" => "webp",
                            _ => "unknown",
                        };

                        let (image_kind, kind_confidence) =
                            crate::extraction::image_kind::classify(&decoded, format, None, None, None, None, false);

                        images.push(ExtractedImage {
                            data: Bytes::from(decoded),
                            format: Cow::Borrowed(format),
                            image_index: images.len() as u32,
                            page_number: Some((cell_idx + 1) as u32),
                            width: None,
                            height: None,
                            colorspace: None,
                            bits_per_component: None,
                            is_mask: false,
                            description: Some(format!("Notebook cell {} output", cell_idx)),
                            ocr_result: None,
                            bounding_box: None,
                            source_path: None,
                            image_kind: Some(image_kind),
                            kind_confidence: Some(kind_confidence),
                            cluster_id: None,
                            caption: None,
                            qr_codes: None,
                            data_base64: None,
                        });
                        content.push_str(&format!("[Image: {}]\n", mime_type));
                    }
                }
            }

            if let Some(svg_value) = data.get("image/svg+xml") {
                let svg_markup = Self::extract_source(svg_value);
                if !svg_markup.is_empty() {
                    let svg_bytes = svg_markup.into_bytes();
                    let (image_kind, kind_confidence) =
                        crate::extraction::image_kind::classify(&svg_bytes, "svg", None, None, None, None, false);

                    images.push(ExtractedImage {
                        data: Bytes::from(svg_bytes),
                        format: Cow::Borrowed("svg"),
                        image_index: images.len() as u32,
                        page_number: Some((cell_idx + 1) as u32),
                        width: None,
                        height: None,
                        colorspace: None,
                        bits_per_component: None,
                        is_mask: false,
                        description: Some(format!("Notebook cell {} output", cell_idx)),
                        ocr_result: None,
                        bounding_box: None,
                        source_path: None,
                        image_kind: Some(image_kind),
                        kind_confidence: Some(kind_confidence),
                        cluster_id: None,
                        caption: None,
                        qr_codes: None,
                        data_base64: None,
                    });
                    content.push_str("[Image: image/svg+xml]\n");
                }
            }

            if let Some(json_content) = data.get("application/json")
                && let Ok(formatted) = serde_json::to_string_pretty(json_content)
            {
                content.push_str(&formatted);
                content.push('\n');
            }
        }

        Ok(())
    }

    /// Push the richest text-bearing representation of a single output onto
    /// `builder` as one element, or push nothing if the output carries none.
    ///
    /// Precedence for `execute_result`/`display_data`/`update_display_data`,
    /// most to least rich (unless `plain` restricts output to plain text, in
    /// which case only `text/plain` is considered):
    /// 1. `text/html` — carries the most structure/semantics; some outputs
    ///    (e.g. a bare `display(HTML(...))` call) have *only* this
    ///    representation, so it must not be skipped in favor of a
    ///    less-structured one that happens to also be present.
    /// 2. `text/markdown` — less structure than HTML but more than plain text.
    /// 3. `text/plain` — always available as a fallback for reprs that only
    ///    implement `__repr__`.
    ///
    /// `stream` outputs use their `text` field directly. `error` outputs
    /// render the exception name/value plus the *full* traceback (not just
    /// the name/value), since the traceback is often the only actionable
    /// diagnostic for a failed cell.
    fn push_output_element(builder: &mut InternalDocumentBuilder, output: &Value, plain: bool) {
        let output_type = output.get("output_type").and_then(|t| t.as_str()).unwrap_or("");

        match output_type {
            "stream" => {
                if let Some(t) = output.get("text") {
                    let text = Self::extract_source(t);
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        builder.push_paragraph(trimmed, vec![], None, None);
                    }
                }
            }
            "execute_result" | "display_data" | "update_display_data" => {
                let Some(data) = output.get("data").and_then(|d| d.as_object()) else {
                    return;
                };
                match Self::richest_output_representation(data, plain) {
                    Some(OutputRepresentation::RawHtml(html)) => {
                        let trimmed = html.trim();
                        if !trimmed.is_empty() {
                            builder.push_raw_block("html", trimmed, None);
                        }
                    }
                    Some(OutputRepresentation::Text(text)) => {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            builder.push_paragraph(trimmed, vec![], None, None);
                        }
                    }
                    None => {}
                }
            }
            "error" => {
                let text = Self::collect_error_text(output);
                if !text.is_empty() {
                    builder.push_paragraph(&text, vec![], None, None);
                }
            }
            _ => {}
        }
    }

    /// Select the single richest text-bearing representation from an
    /// output's `data` bundle. See `push_output_element` for the precedence.
    fn richest_output_representation(
        data: &serde_json::Map<String, Value>,
        plain: bool,
    ) -> Option<OutputRepresentation> {
        if !plain {
            if let Some(html) = data.get("text/html") {
                let text = Self::extract_source(html);
                if !text.is_empty() {
                    return Some(OutputRepresentation::RawHtml(text));
                }
            }
            if let Some(markdown) = data.get("text/markdown") {
                let text = Self::extract_source(markdown);
                if !text.is_empty() {
                    return Some(OutputRepresentation::Text(text));
                }
            }
        }

        let plain_value = data.get("text/plain")?;
        let text = Self::extract_source(plain_value);
        if text.is_empty() {
            None
        } else {
            Some(OutputRepresentation::Text(text))
        }
    }

    /// Render an `error` output's exception name/value and full traceback as
    /// a single text block.
    fn collect_error_text(output: &Value) -> String {
        let ename = output.get("ename").and_then(|e| e.as_str()).unwrap_or("Unknown");
        let evalue = output.get("evalue").and_then(|e| e.as_str()).unwrap_or("");
        let mut text = format!("Error ({}): {}", ename, evalue);

        if let Some(traceback) = output.get("traceback").and_then(|t| t.as_array()) {
            text.push('\n');
            text.push_str("Traceback:");
            for line in traceback {
                if let Some(line_str) = line.as_str() {
                    text.push('\n');
                    text.push_str(line_str);
                }
            }
        }

        text
    }

    /// Build an `InternalDocument` from the already-parsed notebook JSON.
    ///
    /// Markdown cells are split into headings and paragraphs. Code cells
    /// become code blocks followed by any output paragraphs. `plain`
    /// mirrors `ExtractionConfig::output_format` being `Plain`/`Structured`
    /// and suppresses richer (markdown/html) output representations in
    /// favor of `text/plain` only.
    fn build_internal_document(
        notebook: &Value,
        rendering: JupyterCellRendering,
        plain: bool,
    ) -> Option<InternalDocument> {
        let cells = notebook.get("cells")?.as_array()?;

        let kernel_lang = notebook
            .get("metadata")
            .and_then(|m| m.get("kernelspec"))
            .and_then(|k| k.get("language"))
            .and_then(|l| l.as_str())
            .or_else(|| {
                notebook
                    .get("metadata")
                    .and_then(|m| m.get("language_info"))
                    .and_then(|l| l.get("name"))
                    .and_then(|n| n.as_str())
            });

        let mut builder = InternalDocumentBuilder::new("jupyter");

        for cell in cells {
            let cell_type = cell.get("cell_type").and_then(|t| t.as_str()).unwrap_or("unknown");
            let source_text = Self::extract_source(cell.get("source").unwrap_or(&Value::Null));
            let trimmed = source_text.trim();

            // A cell is only genuinely empty (and therefore droppable) if it
            // also has nothing else to contribute: a code cell's `source`
            // may be legitimately cleared (privacy/size stripping) while its
            // saved `outputs` still carry real content (#159), and a
            // markdown/raw cell's `source` may be empty while it still
            // references image `attachments` (#160).
            let has_code_outputs = cell_type == "code"
                && cell
                    .get("outputs")
                    .and_then(|o| o.as_array())
                    .is_some_and(|arr| !arr.is_empty());
            let has_attachments = cell
                .get("attachments")
                .and_then(|a| a.as_object())
                .is_some_and(|m| !m.is_empty());

            if trimmed.is_empty() && !has_code_outputs && !has_attachments {
                continue;
            }

            match cell_type {
                "markdown" => {
                    let events: Vec<pulldown_cmark::Event> =
                        pulldown_cmark::Parser::new_ext(trimmed, crate::extractors::markdown::markdown_options())
                            .collect();
                    let cell_doc =
                        crate::extractors::markdown::MarkdownExtractor::build_internal_document(&events, &None);
                    builder.append_document(cell_doc);
                }
                "code" => {
                    if rendering.includes_source() && !trimmed.is_empty() {
                        let idx = builder.push_code(trimmed, kernel_lang, None, None);
                        let mut attrs = AHashMap::new();
                        if let Some(exec_count) = cell.get("execution_count") {
                            match exec_count {
                                Value::Number(n) => {
                                    attrs.insert("execution_count".to_string(), n.to_string());
                                }
                                Value::Null => {
                                    attrs.insert("execution_count".to_string(), "null".to_string());
                                }
                                _ => {}
                            }
                        }
                        if let Some(tags) = cell
                            .get("metadata")
                            .and_then(|m| m.get("tags"))
                            .and_then(|t| t.as_array())
                            && !tags.is_empty()
                        {
                            let tag_strs: Vec<&str> = tags.iter().filter_map(|v| v.as_str()).collect();
                            attrs.insert("tags".to_string(), tag_strs.join(","));
                        }
                        if !attrs.is_empty() {
                            builder.set_attributes(idx, attrs);
                        }
                    }

                    if rendering.includes_outputs()
                        && let Some(outputs) = cell.get("outputs").and_then(|o| o.as_array())
                    {
                        for output in outputs {
                            Self::push_output_element(&mut builder, output, plain);
                        }
                    }
                }
                _ => {
                    if !trimmed.is_empty() {
                        builder.push_paragraph(trimmed, vec![], None, None);
                    }
                }
            }
        }

        Some(builder.build())
    }

    /// Extract error output, preserving ename, evalue, and traceback in content.
    fn extract_error_output(output: &Value, content: &mut String) -> Result<()> {
        let ename = output.get("ename").and_then(|e| e.as_str()).unwrap_or("Unknown");
        let evalue = output.get("evalue").and_then(|e| e.as_str()).unwrap_or("");

        content.push_str(&format!("Error ({}): {}\n", ename, evalue));

        if let Some(traceback) = output.get("traceback").and_then(|t| t.as_array()) {
            content.push_str("Traceback:\n");
            for line in traceback {
                if let Some(line_str) = line.as_str() {
                    content.push_str(line_str);
                    content.push('\n');
                }
            }
        }

        Ok(())
    }
}

#[cfg(feature = "notebook")]
impl Default for JupyterExtractor {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(feature = "notebook")]
impl Plugin for JupyterExtractor {
    fn name(&self) -> &str {
        "jupyter-extractor"
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
        "Extracts content from Jupyter notebooks (.ipynb files)"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg(feature = "notebook")]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for JupyterExtractor {
    #[cfg_attr(
        feature = "otel",
        tracing::instrument(
            skip(self, content, config),
            fields(
                extractor.name = self.name(),
                content.size_bytes = content.len(),
            )
        )
    )]
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;
        let plain = matches!(
            config.output_format,
            crate::core::config::OutputFormat::Plain | crate::core::config::OutputFormat::Structured
        );
        let (_extracted_content, additional_metadata, extracted_images, notebook_json, warnings, attachment_images) =
            Self::extract_notebook(content, plain)?;

        let mut metadata_additional = AHashMap::new();
        let meta_language = additional_metadata
            .get(&Cow::Borrowed("language_name"))
            .and_then(|v| v.as_str().map(|s| s.to_string()));
        for (key, value) in additional_metadata {
            metadata_additional.insert(key, json!(value));
        }

        // `JupyterCellRendering` governs a code cell's saved *outputs*; cell
        // `attachments` are part of markdown/raw cell *source* and are kept
        // regardless of that setting (see `JupyterCellRendering`'s docs).
        let mut images = if config.jupyter_cell_rendering.includes_outputs() {
            extracted_images
        } else {
            Vec::new()
        };
        images.extend(attachment_images);
        for (index, image) in images.iter_mut().enumerate() {
            image.image_index = index as u32;
        }

        let mut doc = Self::build_internal_document(&notebook_json, config.jupyter_cell_rendering, plain)
            .unwrap_or_else(|| InternalDocumentBuilder::new("jupyter").build());
        doc.mime_type = mime_type.to_string();

        doc.metadata = Metadata {
            language: meta_language,
            additional: metadata_additional,
            ..Default::default()
        };
        doc.images = images;
        doc.processing_warnings.extend(warnings);

        Ok(doc)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-ipynb+json"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::internal::ElementKind;

    #[test]
    fn test_jupyter_extractor_plugin_interface() {
        let extractor = JupyterExtractor::new();
        assert_eq!(extractor.name(), "jupyter-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(extractor.priority(), 50);
        assert!(extractor.supported_mime_types().contains(&"application/x-ipynb+json"));
    }

    #[test]
    fn test_extract_execution_count_and_tags() {
        let notebook_json = r#"{
            "cells": [
                {
                    "cell_type": "code",
                    "id": "code-cell",
                    "source": ["print('hello')"],
                    "execution_count": 5,
                    "outputs": [{
                        "output_type": "execute_result",
                        "execution_count": 5,
                        "data": {"text/plain": ["hello"]},
                        "metadata": {}
                    }],
                    "metadata": {"tags": ["test-tag", "important"]}
                }
            ],
            "metadata": {
                "kernelspec": {"name": "python3", "language": "python"},
                "language_info": {"name": "python", "version": "3.10.0", "mimetype": "text/x-python"}
            },
            "nbformat": 4,
            "nbformat_minor": 5
        }"#;

        let (_, metadata, _, _, _, _) = JupyterExtractor::extract_notebook(notebook_json.as_bytes(), false).unwrap();

        let cells = metadata.get(&Cow::Borrowed("cells"));
        assert!(cells.is_some(), "Should have cells metadata array");
        let cells_arr = cells.unwrap().as_array().expect("cells should be an array");
        assert_eq!(cells_arr.len(), 1);
        let cell0 = &cells_arr[0];
        assert_eq!(cell0["index"], json!(0));
        assert_eq!(cell0["cell_type"], json!("code"));
        assert_eq!(cell0["id"], json!("code-cell"));
        assert_eq!(cell0["execution_count"], json!(5));
        assert_eq!(cell0["tags"], json!(["test-tag", "important"]));
        assert_eq!(cell0["outputs"][0]["output_type"], json!("execute_result"));
        assert_eq!(cell0["outputs"][0]["execution_count"], json!(5));
        assert_eq!(cell0["outputs"][0]["mime_types"], json!(["text/plain"]));

        assert_eq!(metadata.get(&Cow::Borrowed("cell_count")), Some(&json!(1)));

        assert_eq!(metadata.get(&Cow::Borrowed("language_name")), Some(&json!("python")));
        assert_eq!(metadata.get(&Cow::Borrowed("language_version")), Some(&json!("3.10.0")));
        assert_eq!(
            metadata.get(&Cow::Borrowed("language_mimetype")),
            Some(&json!("text/x-python"))
        );

        assert_eq!(metadata.get(&Cow::Borrowed("nbformat_minor")), Some(&json!(5)));
    }

    #[test]
    fn test_extract_error_output_content() {
        let notebook_json = r#"{
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["1/0"],
                    "execution_count": 1,
                    "outputs": [
                        {
                            "output_type": "error",
                            "ename": "ZeroDivisionError",
                            "evalue": "division by zero",
                            "traceback": ["Traceback line 1", "Traceback line 2"]
                        }
                    ],
                    "metadata": {}
                }
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 0
        }"#;

        let (content, _, _, _, _, _) = JupyterExtractor::extract_notebook(notebook_json.as_bytes(), false).unwrap();

        assert!(
            content.contains("Error (ZeroDivisionError): division by zero"),
            "Should contain error name and value"
        );
        assert!(content.contains("Traceback:"), "Should contain traceback header");
        assert!(content.contains("Traceback line 1"), "Should contain traceback lines");
    }

    fn rendering_sample() -> Value {
        serde_json::from_str(
            r#"{
            "cells": [
                {
                    "cell_type": "code",
                    "source": ["print('hello world')"],
                    "execution_count": 1,
                    "outputs": [
                        {"output_type": "stream", "name": "stdout", "text": ["hello world\n"]}
                    ],
                    "metadata": {}
                }
            ],
            "metadata": {"kernelspec": {"name": "python3", "language": "python"}},
            "nbformat": 4,
            "nbformat_minor": 5
        }"#,
        )
        .unwrap()
    }

    #[test]
    fn test_rendering_source_emits_code_without_outputs() {
        let doc = JupyterExtractor::build_internal_document(&rendering_sample(), JupyterCellRendering::Source, false)
            .unwrap();
        assert!(
            doc.elements
                .iter()
                .any(|e| matches!(e.kind, ElementKind::Code) && e.text.contains("print('hello world')")),
            "source rendering keeps the code cell"
        );
        assert!(
            !doc.elements.iter().any(|e| e.text.contains("[output_type:")),
            "source rendering suppresses saved outputs"
        );
    }

    #[test]
    fn test_rendering_outputs_emits_outputs_without_code() {
        let doc = JupyterExtractor::build_internal_document(&rendering_sample(), JupyterCellRendering::Outputs, false)
            .unwrap();
        assert!(
            !doc.elements.iter().any(|e| matches!(e.kind, ElementKind::Code)),
            "outputs rendering suppresses the code source"
        );
        assert!(
            doc.elements.iter().any(|e| e.text.contains("hello world")),
            "outputs rendering keeps the saved output text"
        );
        assert!(
            !doc.elements.iter().any(|e| e.text.contains("[output_type:")),
            "outputs rendering does not expose diagnostic markers"
        );
    }

    #[test]
    fn test_rendering_both_emits_code_and_outputs() {
        let doc =
            JupyterExtractor::build_internal_document(&rendering_sample(), JupyterCellRendering::Both, false).unwrap();
        assert!(
            doc.elements.iter().any(|e| matches!(e.kind, ElementKind::Code)),
            "both rendering keeps the code source"
        );
        assert!(doc.elements.iter().any(|e| e.text.contains("hello world")));
        assert!(!doc.elements.iter().any(|e| e.text.contains("[output_type:")));
    }

    #[test]
    fn test_markdown_cell_reuses_shared_parser() {
        let notebook: Value = serde_json::from_str(
            r##"{
            "cells": [
                {"cell_type": "markdown", "source": ["# Heading\n\nSome **bold** prose."], "metadata": {}}
            ],
            "metadata": {},
            "nbformat": 4,
            "nbformat_minor": 5
        }"##,
        )
        .unwrap();
        let doc = JupyterExtractor::build_internal_document(&notebook, JupyterCellRendering::Both, false).unwrap();
        assert!(
            doc.elements
                .iter()
                .any(|e| matches!(e.kind, ElementKind::Heading { .. }) && e.text.contains("Heading")),
            "markdown cells render through the shared MarkdownExtractor (heading element present)"
        );
    }
}
