//! Source code extractor using tree-sitter language pack.
//!
//! Extracts content and structural analysis from source code files using
//! tree-sitter parsers. Language detection is performed via file extension
//! or shebang line.

use std::borrow::Cow;
use std::path::Path;

use async_trait::async_trait;
use tree_sitter_language_pack as tslp;

use crate::Result;
use crate::core::config::{CodeContentMode, ExtractionConfig};
use crate::core::mime::SOURCE_CODE_MIME_TYPE;
use crate::extractors::SyncExtractor;
use crate::internal_builder::InternalDocumentBuilder;
use crate::plugins::InternalDocumentExtractor;
use crate::plugins::Plugin;
use crate::types::internal::InternalDocument;
use crate::types::metadata::{
    CodeChunkInfo, CodeDataAttribute, CodeDataNode, CodeDataNodeKind, CodeMetadata, FormatMetadata, Metadata,
};

/// `metadata.additional` scratch key carrying the full serialized
/// `tree_sitter_language_pack::ProcessResult` — language, metrics, structure,
/// imports, exports, comments, docstrings, symbols and diagnostics — from
/// extraction through to `extraction::derive::derive_extraction_result`.
///
/// `CodeMetadata` (the typed, FFI-facing struct on `Metadata::format`)
/// deliberately carries only `chunks`/`data`, so the rest of `ProcessResult`
/// has nowhere else to travel without widening that type or `ExtractedDocument`
/// itself. This key is removed from `metadata.additional` by the derivation
/// step (see `extraction/derive.rs`), so it never leaks into the final
/// `ExtractedDocument.metadata.additional` map.
pub(crate) const CODE_INTELLIGENCE_SCRATCH_KEY: &str = "__xberg_code_intelligence_process_result";

/// `ProcessingWarning::source` for every warning this extractor emits (#171).
const CODE_WARNING_SOURCE: &str = "code";

#[cfg_attr(alef, alef(skip))]
/// Source code extractor using tree-sitter language pack.
///
/// Detects the programming language from the file extension or shebang line,
/// then uses tree-sitter to parse and extract structural information.
pub struct CodeExtractor;

impl Default for CodeExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeExtractor {
    pub(crate) fn new() -> Self {
        Self
    }

    /// Build a `tslp::ProcessConfig` from the xberg `TreeSitterProcessConfig`.
    fn build_process_config(language: &str, config: &ExtractionConfig) -> tslp::ProcessConfig {
        if let Some(ref ts_config) = config.tree_sitter {
            let pc: tslp::ProcessConfig = (&ts_config.process).into();
            return tslp::ProcessConfig {
                language: Cow::Owned(language.to_string()),
                ..pc
            };
        }
        tslp::ProcessConfig::new(language)
    }

    /// Build a document that emits the raw source verbatim, with no tree-sitter
    /// processing. Used when tree-sitter is disabled via config.
    fn build_raw_document(source: &str, language: &str) -> InternalDocument {
        let mut builder = InternalDocumentBuilder::new("code");
        builder.push_code(source, Some(language), None, None);

        let mut doc = builder.build();
        doc.metadata = Metadata {
            format: Some(FormatMetadata::Code(CodeMetadata::default())),
            ..Default::default()
        };
        doc.mime_type = SOURCE_CODE_MIME_TYPE.to_string();
        doc
    }

    /// Heading level for a chunk's context marker: 2 for class/module-shaped
    /// containers, 3 for everything else (functions, methods, etc.).
    fn chunk_heading_level(chunk: &tslp::CodeChunk) -> u8 {
        if chunk.metadata.node_types.iter().any(|t| {
            matches!(
                t.as_str(),
                "class_definition" | "module_definition" | "class_declaration" | "module"
            )
        }) {
            2
        } else {
            3
        }
    }

    /// Build the `PackConfig` to apply to TSLP's grammar cache before parsing,
    /// or `None` when `TreeSitterConfig::cache_dir` is unset.
    ///
    /// `languages`/`groups` are deliberately left out of this `PackConfig`:
    /// they are pre-download hints for the CLI's `tree-sitter download`/`cache
    /// warm` commands (see `xberg-cli/src/commands/tree_sitter.rs`), not
    /// per-file gates. Extraction always operates on a single, already
    /// auto-detected `language` (from the file extension, shebang, or
    /// content), so there is nothing for a language/group allowlist to filter
    /// at this call site — `tslp::process` downloads that one language
    /// on demand regardless.
    ///
    /// Unavailable on wasm32 (see `configure_grammar_cache_dir` below) so it
    /// is gated the same way to avoid a dead-code warning on that target.
    #[cfg(not(target_arch = "wasm32"))]
    fn grammar_cache_pack_config(
        ts_config: Option<&crate::core::config::TreeSitterConfig>,
    ) -> Option<tslp::PackConfig> {
        let cache_dir = ts_config.and_then(|c| c.cache_dir.clone())?;
        Some(tslp::PackConfig {
            cache_dir: Some(cache_dir),
            languages: None,
            groups: None,
        })
    }

    /// Point TSLP's on-demand grammar downloader at `TreeSitterConfig::cache_dir`
    /// before parsing, so a configured cache directory is honoured at
    /// extraction time too — not just by the CLI `tree-sitter download`/`cache
    /// warm` commands.
    ///
    /// A `None` `cache_dir` is a no-op: TSLP keeps using its own default
    /// location. Unavailable on wasm32, where TSLP's `download`/`configure`
    /// API does not exist (grammars are compiled in rather than fetched at
    /// runtime).
    #[cfg(not(target_arch = "wasm32"))]
    fn configure_grammar_cache_dir(ts_config: Option<&crate::core::config::TreeSitterConfig>) -> Result<()> {
        let Some(pack_config) = Self::grammar_cache_pack_config(ts_config) else {
            return Ok(());
        };
        tslp::configure(&pack_config).map_err(|e| crate::XbergError::Cache {
            message: format!("failed to configure tree-sitter grammar cache directory: {e}"),
            source: None,
        })
    }

    #[cfg(target_arch = "wasm32")]
    fn configure_grammar_cache_dir(_ts_config: Option<&crate::core::config::TreeSitterConfig>) -> Result<()> {
        Ok(())
    }

    /// Extract from source text with a known language.
    fn extract_with_language(source: &str, language: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let ts_config = config.tree_sitter.as_ref();

        if !ts_config.map(|c| c.enabled).unwrap_or(true) {
            return Ok(Self::build_raw_document(source, language));
        }

        Self::configure_grammar_cache_dir(ts_config)?;

        let process_config = Self::build_process_config(language, config);
        let content_mode = ts_config.map(|c| c.process.content_mode).unwrap_or_default();

        let result = tslp::process(source, &process_config).map_err(|e| crate::XbergError::Parsing {
            message: format!("tree-sitter processing failed for language '{language}': {e}"),
            source: None,
        })?;

        // #259: `chunks`/`data` get lifted into the typed `CodeMetadata` below, but the
        // rest of `ProcessResult` (metrics, structure, imports, exports, comments,
        // docstrings, symbols, diagnostics) has no typed home. Serialize the whole
        // result now, while it is still in scope, and stash it in the scratch slot so
        // the derivation step can surface it as `code_intelligence` instead of losing it.
        let process_result_json = serde_json::to_value(&result).ok();

        let mut builder = InternalDocumentBuilder::new("code");
        let mut code_chunks: Vec<CodeChunkInfo> = Vec::with_capacity(result.chunks.len());

        if result.chunks.is_empty() {
            builder.push_code(source, Some(language), None, None);
        } else {
            for chunk in &result.chunks {
                match content_mode {
                    CodeContentMode::Raw => {}
                    CodeContentMode::Structure => {
                        if let Some(last_context) = chunk.metadata.context_path.last() {
                            let level = Self::chunk_heading_level(chunk);
                            builder.push_heading(level, last_context, None, None);
                        }
                    }
                    _ => {
                        if let Some(last_context) = chunk.metadata.context_path.last() {
                            let level = Self::chunk_heading_level(chunk);
                            builder.push_heading(level, last_context, None, None);
                        }
                        builder.push_code(&chunk.content, Some(language), None, None);
                    }
                }

                code_chunks.push(CodeChunkInfo {
                    text: chunk.content.clone(),
                    context_path: chunk.metadata.context_path.clone(),
                    node_types: chunk.metadata.node_types.clone(),
                    byte_start: chunk.start_byte,
                    byte_end: chunk.end_byte,
                });
            }

            if matches!(content_mode, CodeContentMode::Raw) {
                builder.push_code(source, Some(language), None, None);
            }
        }

        let mut additional = ahash::AHashMap::default();
        if let Some(json) = process_result_json {
            additional.insert(Cow::Borrowed(CODE_INTELLIGENCE_SCRATCH_KEY), json);
        }

        let mut doc = builder.build();
        doc.metadata = Metadata {
            format: Some(FormatMetadata::Code(CodeMetadata {
                chunks: code_chunks,
                data: result.data.as_ref().map(convert_data_node),
            })),
            additional,
            ..Default::default()
        };
        doc.mime_type = SOURCE_CODE_MIME_TYPE.to_string();

        Ok(doc)
    }

    /// Detect language and read source from a file path.
    ///
    /// Returns `(language, source)`. Reads the file at most once.
    fn read_and_detect(path: &Path) -> Result<(String, String)> {
        let path_str = path.to_string_lossy();

        if let Some(lang) = tslp::detect_language_from_path(&path_str) {
            let source = std::fs::read_to_string(path)?;
            return Ok((lang.to_string(), source));
        }

        let source = std::fs::read_to_string(path)?;
        if let Some(lang) = tslp::detect_language_from_content(&source) {
            return Ok((lang.to_string(), source));
        }

        Err(crate::XbergError::UnsupportedFormat(format!(
            "Cannot detect programming language for: {}",
            path.display()
        )))
    }
}

impl Plugin for CodeExtractor {
    fn name(&self) -> &str {
        "code-extractor"
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
        "Extracts content and structure from source code files using tree-sitter"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for CodeExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        _mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        tracing::debug!(format = "code", size_bytes = content.len(), "extraction starting");
        let source = String::from_utf8_lossy(content);
        // `Cow::Owned` here means `from_utf8_lossy` had to allocate a replacement copy,
        // which it only does when it found at least one undecodable byte sequence to
        // substitute U+FFFD for; valid UTF-8 input is returned unchanged as
        // `Cow::Borrowed` (#171).
        let decoded_lossily = matches!(source, Cow::Owned(_));

        let language = tslp::detect_language_from_content(&source)
            .or_else(|| config.source_name.as_deref().and_then(tslp::detect_language_from_path))
            .ok_or_else(|| {
                crate::XbergError::UnsupportedFormat(
                    "Cannot detect programming language from content (no shebang line). \
                     Use extract_file with a file path for extension-based detection."
                        .to_string(),
                )
            })?;

        let mut doc = Self::extract_with_language(&source, language, config)?;
        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut doc.processing_warnings,
                CODE_WARNING_SOURCE,
                "source file",
            );
        }
        tracing::debug!(
            element_count = doc.elements.len(),
            format = "code",
            "extraction complete"
        );
        Ok(doc)
    }

    async fn extract_path(&self, path: &Path, _mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let (language, source) = Self::read_and_detect(path)?;
        Self::extract_with_language(&source, &language, config)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[SOURCE_CODE_MIME_TYPE]
    }

    fn priority(&self) -> i32 {
        50
    }
}

impl SyncExtractor for CodeExtractor {
    fn extract_sync(&self, content: &[u8], _mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let source = String::from_utf8_lossy(content);
        let decoded_lossily = matches!(source, Cow::Owned(_));

        let language = tslp::detect_language_from_content(&source)
            .or_else(|| config.source_name.as_deref().and_then(tslp::detect_language_from_path))
            .ok_or_else(|| {
                crate::XbergError::UnsupportedFormat("Cannot detect programming language from content".to_string())
            })?;

        let mut doc = Self::extract_with_language(&source, language, config)?;
        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut doc.processing_warnings,
                CODE_WARNING_SOURCE,
                "source file",
            );
        }
        Ok(doc)
    }
}

/// Recursively map a `tree_sitter_language_pack::DataNode` to xberg's
/// FFI/binding-friendly [`CodeDataNode`], flattening `Span` down to byte offsets.
fn convert_data_node(node: &tslp::DataNode) -> CodeDataNode {
    CodeDataNode {
        kind: match node.kind {
            tslp::DataNodeKind::KeyValue => CodeDataNodeKind::KeyValue,
            tslp::DataNodeKind::Element => CodeDataNodeKind::Element,
            tslp::DataNodeKind::Sequence => CodeDataNodeKind::Sequence,
        },
        key: node.key.clone(),
        value: node.value.clone(),
        attributes: node
            .attributes
            .iter()
            .map(|attr| CodeDataAttribute {
                name: attr.name.clone(),
                value: attr.value.clone(),
                byte_start: attr.span.start_byte,
                byte_end: attr.span.end_byte,
            })
            .collect(),
        children: node.children.iter().map(convert_data_node).collect(),
        byte_start: node.span.start_byte,
        byte_end: node.span.end_byte,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use std::path::PathBuf;

    /// `grammar_cache_pack_config` must return `None` when no config is
    /// supplied at all — there is nothing to configure.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_grammar_cache_pack_config_none_when_no_tree_sitter_config() {
        assert!(CodeExtractor::grammar_cache_pack_config(None).is_none());
    }

    /// `grammar_cache_pack_config` must return `None` when `cache_dir` is
    /// unset, even if a `TreeSitterConfig` is present — this is the "no
    /// override configured" case that must be a pure no-op.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_grammar_cache_pack_config_none_when_cache_dir_unset() {
        let config = crate::core::config::TreeSitterConfig::default();
        assert!(CodeExtractor::grammar_cache_pack_config(Some(&config)).is_none());
    }

    /// `grammar_cache_pack_config` must carry `TreeSitterConfig::cache_dir`
    /// into the resulting `PackConfig` unchanged, with `languages`/`groups`
    /// left empty (those are CLI pre-download hints, not extraction-time
    /// gates — see the doc comment on `grammar_cache_pack_config`).
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn test_grammar_cache_pack_config_carries_configured_cache_dir() {
        let config = crate::core::config::TreeSitterConfig {
            cache_dir: Some(PathBuf::from("/tmp/my-grammars")),
            languages: Some(vec!["python".to_string()]),
            groups: Some(vec!["web".to_string()]),
            ..Default::default()
        };

        let pack_config =
            CodeExtractor::grammar_cache_pack_config(Some(&config)).expect("cache_dir set must produce a PackConfig");

        assert_eq!(pack_config.cache_dir, Some(PathBuf::from("/tmp/my-grammars")));
        assert!(pack_config.languages.is_none());
        assert!(pack_config.groups.is_none());
    }

    fn code_warnings(doc: &InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.source == CODE_WARNING_SOURCE)
            .map(|w| w.message.to_string())
            .collect()
    }

    /// Config used by the lossy-decode tests below: tree-sitter disabled (so
    /// `extract_with_language` never needs a grammar download in a test), with
    /// `source_name` set so language detection falls back to the file extension
    /// instead of needing a shebang line in the (deliberately garbled) content.
    fn disabled_tree_sitter_config(source_name: &str) -> ExtractionConfig {
        ExtractionConfig {
            source_name: Some(source_name.to_string()),
            tree_sitter: Some(crate::core::config::TreeSitterConfig {
                enabled: false,
                ..Default::default()
            }),
            ..Default::default()
        }
    }

    /// #171: `String::from_utf8_lossy` silently substitutes U+FFFD for every
    /// undecodable byte and returns `Ok`, so a source file with invalid UTF-8
    /// bytes was indistinguishable from a clean one.
    #[tokio::test]
    async fn should_warn_when_source_file_is_not_valid_utf8() {
        let extractor = CodeExtractor::new();
        let config = disabled_tree_sitter_config("test.py");
        let content: &[u8] = b"print(\xFF\xFE'hi')";

        let doc = extractor
            .extract_content(content, SOURCE_CODE_MIME_TYPE, &config)
            .await
            .expect("extraction of invalid UTF-8 source must still succeed");

        let warnings = code_warnings(&doc);
        assert_eq!(warnings.len(), 1, "expected exactly one code warning, got {warnings:?}");
        assert!(
            warnings[0].contains("not valid UTF-8") && warnings[0].contains("replacement character"),
            "warning must describe the lossy decode, got {warnings:?}"
        );
    }

    /// A valid UTF-8 source file must not warn.
    #[tokio::test]
    async fn valid_utf8_source_file_produces_zero_warnings() {
        let extractor = CodeExtractor::new();
        let config = disabled_tree_sitter_config("test.py");
        let content = b"print('hi')";

        let doc = extractor
            .extract_content(content, SOURCE_CODE_MIME_TYPE, &config)
            .await
            .expect("extraction should succeed");

        assert!(
            code_warnings(&doc).is_empty(),
            "valid UTF-8 source must not warn, got {:?}",
            code_warnings(&doc)
        );
    }

    /// Disabled tree-sitter config must skip TSLP processing entirely and emit the
    /// raw source as a single code element — this path must not call
    /// `tslp::process`, which needs grammar downloads at runtime.
    #[test]
    fn test_disabled_tree_sitter_emits_raw_source() {
        let config = ExtractionConfig {
            tree_sitter: Some(crate::core::config::TreeSitterConfig {
                enabled: false,
                ..Default::default()
            }),
            ..Default::default()
        };

        let source = "fn main() {\n    println!(\"hi\");\n}\n";
        let doc = CodeExtractor::extract_with_language(source, "rust", &config).expect("raw extraction must succeed");

        assert_eq!(doc.elements.len(), 1, "exactly one raw code element expected");
        assert_eq!(doc.mime_type, SOURCE_CODE_MIME_TYPE);

        let Some(FormatMetadata::Code(CodeMetadata { chunks, data })) = doc.metadata.format.as_ref() else {
            panic!("expected Code format metadata");
        };
        assert!(chunks.is_empty(), "raw path must not populate chunks");
        assert!(data.is_none(), "raw path must not populate data");
    }

    /// `TreeSitterProcessConfig::data_extraction` must map through to TSLP's
    /// `ProcessConfig::data_extraction` unchanged.
    #[test]
    fn test_process_config_maps_data_extraction() {
        let xberg_process_config = crate::core::config::TreeSitterProcessConfig {
            data_extraction: true,
            ..Default::default()
        };

        let tslp_process_config: tslp::ProcessConfig = (&xberg_process_config).into();

        assert!(tslp_process_config.data_extraction);
    }

    /// `convert_data_node` must map kind, key, value, attributes, children, and
    /// byte offsets from a hand-built `tslp::DataNode` tree.
    #[test]
    fn test_convert_data_node_maps_tree() {
        let child_span = tslp::Span {
            start_byte: 2,
            end_byte: 10,
            start_line: 0,
            start_column: 2,
            end_line: 0,
            end_column: 10,
        };
        let attr_span = tslp::Span {
            start_byte: 3,
            end_byte: 9,
            start_line: 0,
            start_column: 3,
            end_line: 0,
            end_column: 9,
        };
        let root_span = tslp::Span {
            start_byte: 0,
            end_byte: 12,
            start_line: 0,
            start_column: 0,
            end_line: 0,
            end_column: 12,
        };

        let child = tslp::DataNode {
            kind: tslp::DataNodeKind::Element,
            key: Some("host".to_string()),
            value: Some("localhost".to_string()),
            attributes: vec![tslp::DataAttribute {
                name: "class".to_string(),
                value: "primary".to_string(),
                span: attr_span,
            }],
            children: Vec::new(),
            span: child_span,
        };

        let root = tslp::DataNode {
            kind: tslp::DataNodeKind::KeyValue,
            key: None,
            value: None,
            attributes: Vec::new(),
            children: vec![child],
            span: root_span,
        };

        let converted = convert_data_node(&root);

        assert_eq!(converted.kind, CodeDataNodeKind::KeyValue);
        assert_eq!(converted.key, None);
        assert_eq!(converted.value, None);
        assert!(converted.attributes.is_empty());
        assert_eq!(converted.byte_start, 0);
        assert_eq!(converted.byte_end, 12);

        assert_eq!(converted.children.len(), 1);
        let converted_child = &converted.children[0];
        assert_eq!(converted_child.kind, CodeDataNodeKind::Element);
        assert_eq!(converted_child.key.as_deref(), Some("host"));
        assert_eq!(converted_child.value.as_deref(), Some("localhost"));
        assert_eq!(converted_child.byte_start, 2);
        assert_eq!(converted_child.byte_end, 10);

        assert_eq!(converted_child.attributes.len(), 1);
        let converted_attr = &converted_child.attributes[0];
        assert_eq!(converted_attr.name, "class");
        assert_eq!(converted_attr.value, "primary");
        assert_eq!(converted_attr.byte_start, 3);
        assert_eq!(converted_attr.byte_end, 9);
    }

    /// `CodeDataNodeKind` must serialize under `snake_case` naming, matching the
    /// rest of xberg's public API convention.
    #[test]
    fn test_code_data_node_kind_serde_snake_case() {
        assert_eq!(
            serde_json::to_string(&CodeDataNodeKind::KeyValue).expect("serializes"),
            "\"key_value\""
        );
        assert_eq!(
            serde_json::to_string(&CodeDataNodeKind::Element).expect("serializes"),
            "\"element\""
        );
        assert_eq!(
            serde_json::to_string(&CodeDataNodeKind::Sequence).expect("serializes"),
            "\"sequence\""
        );
    }
}
