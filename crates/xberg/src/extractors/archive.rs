//! Archive extractors for ZIP, TAR, 7z, and GZIP formats.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::archive::{
    ArchiveMetadata as ExtractedMetadata, extract_7z_file_bytes, extract_7z_metadata, extract_7z_text_content,
    extract_gzip, extract_gzip_with_bytes, extract_tar_file_bytes, extract_tar_metadata, extract_tar_text_content,
    extract_zip_file_bytes, extract_zip_metadata, extract_zip_text_content,
};
use crate::extractors::SyncExtractor;
use crate::extractors::security::ZipBombValidator;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::internal::{ElementKind, InternalDocument, InternalElement};
use crate::types::{ArchiveMetadata, Metadata, ProcessingWarning};
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;
use std::io::Cursor;

/// `ProcessingWarning::source` used for every degradation reported by the archive extractors.
const ARCHIVE_WARNING_SOURCE: &str = "archive";

/// Build an `InternalDocument` from archive metadata and text contents.
///
/// Shared inner function — takes pre-computed children and warnings.
fn build_archive_doc_inner(
    extraction_metadata: ExtractedMetadata,
    text_contents: AHashMap<String, String>,
    format_name: &'static str,
    mime_type: &str,
    children: Vec<crate::types::ArchiveEntry>,
    processing_warnings: Vec<ProcessingWarning>,
) -> InternalDocument {
    let file_names: Vec<String> = extraction_metadata
        .file_list
        .iter()
        .map(|entry| entry.path.clone())
        .collect();

    let archive_metadata = ArchiveMetadata {
        format: Cow::Borrowed(format_name),
        file_count: extraction_metadata.file_count as u32,
        file_list: file_names,
        total_size: extraction_metadata.total_size,
        compressed_size: None,
    };

    let mut additional = AHashMap::new();
    let file_details: Vec<serde_json::Value> = extraction_metadata
        .file_list
        .iter()
        .map(|entry| {
            serde_json::json!({
                "path": entry.path,
                "size": entry.size,
                "is_dir": entry.is_dir,
            })
        })
        .collect();
    additional.insert(Cow::Borrowed("files"), serde_json::json!(file_details));

    let metadata = Metadata {
        format: Some(crate::types::FormatMetadata::Archive(archive_metadata)),
        additional,
        ..Default::default()
    };

    let mut doc = InternalDocument::new(format_name.to_lowercase());
    doc.mime_type = mime_type.to_string();
    doc.metadata = metadata;

    let mut idx = 0u32;
    let summary = format!(
        "{} Archive ({} files, {} bytes)",
        format_name, extraction_metadata.file_count, extraction_metadata.total_size
    );
    doc.push_element(InternalElement::text(ElementKind::Paragraph, &summary, 0).with_index(idx));
    idx += 1;

    let mut file_list = String::from("Files:\n");
    for entry in &extraction_metadata.file_list {
        file_list.push_str(&format!("- {} ({} bytes)\n", entry.path, entry.size));
    }
    doc.push_element(InternalElement::text(ElementKind::Paragraph, &file_list, 0).with_index(idx));
    idx += 1;

    // `text_contents` is an `AHashMap`, and aHash randomizes iteration order per process, so
    // extracting the same archive twice produced the members in a different order each run.
    // Emit them in the archive's own order instead, which also makes the bodies agree with the
    // "Files:" listing printed just above. Members the listing does not cover sort after it by
    // path, so the ordering stays total even if the two disagree (#121).
    let listing_order: AHashMap<&str, usize> = extraction_metadata
        .file_list
        .iter()
        .enumerate()
        .map(|(position, entry)| (entry.path.as_str(), position))
        .collect();
    let mut members: Vec<(&String, &String)> = text_contents.iter().collect();
    members.sort_by(|(left, _), (right, _)| {
        let rank = |path: &String| listing_order.get(path.as_str()).copied().unwrap_or(usize::MAX);
        rank(left).cmp(&rank(right)).then_with(|| left.cmp(right))
    });

    for (path, content) in members {
        let text = format!("=== {} ===\n{}", path, content);
        doc.push_element(InternalElement::text(ElementKind::Paragraph, &text, 0).with_index(idx));
        idx += 1;
    }

    doc.children = if children.is_empty() { None } else { Some(children) };
    doc.processing_warnings = processing_warnings;

    doc
}

/// Sync version — no recursive child extraction.
fn build_archive_doc_sync(
    extraction_metadata: ExtractedMetadata,
    text_contents: AHashMap<String, String>,
    format_name: &'static str,
    mime_type: &str,
) -> InternalDocument {
    build_archive_doc_inner(
        extraction_metadata,
        text_contents,
        format_name,
        mime_type,
        Vec::new(),
        Vec::new(),
    )
}

/// Returns true if `path` names an archive/tooling bookkeeping file (macOS `.DS_Store`,
/// `__MACOSX/` AppleDouble resource forks, `._`-prefixed AppleDouble sidecars, Python
/// `__pycache__/`/`.pyc`/`.pyo` bytecode, or Windows `Thumbs.db`/`desktop.ini`) rather than
/// a real document, so it can be filtered out of archive `children` before extraction.
fn is_archive_metadata_path(path: &str) -> bool {
    let components: Vec<&str> = path.split(['/', '\\']).collect();
    let basename = components.last().copied().unwrap_or(path);

    let has_bookkeeping_dir = components
        .iter()
        .any(|component| *component == "__MACOSX" || *component == "__pycache__");
    if has_bookkeeping_dir {
        return true;
    }

    if basename == ".DS_Store" || basename == "Thumbs.db" || basename == "desktop.ini" {
        return true;
    }

    if basename.starts_with("._") {
        return true;
    }

    let lower = basename.to_ascii_lowercase();
    lower.ends_with(".pyc") || lower.ends_with(".pyo")
}

/// Async version with recursive extraction of archive children.
///
/// When `config.max_archive_depth > current_depth`, extracts each file in `file_bytes`
/// by detecting its MIME type and dispatching to the appropriate extractor.
async fn build_archive_doc(
    extraction_metadata: ExtractedMetadata,
    text_contents: AHashMap<String, String>,
    file_bytes: AHashMap<String, Vec<u8>>,
    format_name: &'static str,
    mime_type: &str,
    config: &ExtractionConfig,
    current_depth: usize,
) -> InternalDocument {
    let mut children = Vec::new();
    let mut processing_warnings = Vec::new();
    let mut filtered_paths: Vec<String> = Vec::new();

    // A non-directory entry that the archive index lists but whose bytes never made it
    // into `file_bytes` failed to decompress (bad CRC, truncated deflate stream, ...).
    // It is absent from the text contents *and* from `children`, so name it instead of
    // letting the document look complete (#114, #115).
    let unreadable_entries: Vec<String> = extraction_metadata
        .file_list
        .iter()
        .filter(|entry| !entry.is_dir && !file_bytes.contains_key(&entry.path))
        .map(|entry| entry.path.clone())
        .collect();
    if !unreadable_entries.is_empty() {
        let message = format!(
            "Skipped {} archive entr{} that could not be read: {}",
            unreadable_entries.len(),
            if unreadable_entries.len() == 1 { "y" } else { "ies" },
            crate::core::diagnostics::format_entry_list(&unreadable_entries)
        );
        crate::core::diagnostics::push_warning(&mut processing_warnings, ARCHIVE_WARNING_SOURCE, message);
    }

    if config.max_archive_depth > current_depth && !file_bytes.is_empty() {
        for (path, bytes) in &file_bytes {
            if is_archive_metadata_path(path) {
                filtered_paths.push(path.clone());
                continue;
            }

            let sniffed_mime = crate::core::mime::detect_mime_type_from_bytes(bytes).ok();

            // Sniffing sees markdown/CSV/YAML as plain UTF-8 and returns `text/plain`,
            // so fall back to the extension (as the top-level path does) to reach their
            // real extractors; a concrete sniff (PDF, DOCX, ...) still wins. Only default
            // to plain text when the extension itself maps to a textual type — an
            // unsniffable, extensionless (or unknown-extension) file is treated as
            // `application/octet-stream` so the skip below fires instead of misreporting
            // binary garbage as `text/plain`. ~keep
            let file_mime = match sniffed_mime {
                Some(m) if m != crate::core::mime::PLAIN_TEXT_MIME_TYPE => m,
                sniffed => crate::core::mime::detect_mime_type(path, false)
                    .ok()
                    .or(sniffed)
                    .unwrap_or_else(|| "application/octet-stream".to_string()),
            };

            if file_mime == "application/octet-stream" {
                filtered_paths.push(path.clone());
                continue;
            }

            let mut child_config = config.clone();
            child_config.max_archive_depth = config.max_archive_depth.saturating_sub(current_depth + 1);

            match crate::core::extractor::extract_bytes(bytes, &file_mime, &child_config).await {
                Ok(result) => {
                    children.push(crate::types::ArchiveEntry {
                        path: path.clone(),
                        mime_type: file_mime,
                        result: Box::new(result),
                    });
                }
                Err(e) => {
                    processing_warnings.push(ProcessingWarning {
                        source: Cow::Borrowed("archive_recursive_extraction"),
                        message: Cow::Owned(format!("Failed to extract '{}': {}", path, e)),
                    });
                }
            }
        }
    }

    if !filtered_paths.is_empty() {
        // `file_bytes` is a hash map, so its iteration order is not stable; sort so the
        // warning text is deterministic for a given archive.
        filtered_paths.sort();
        let message = format!(
            "Filtered {} bookkeeping/binary entr{} (e.g. .DS_Store, __MACOSX, __pycache__, .pyc) \
             from archive children: {}",
            filtered_paths.len(),
            if filtered_paths.len() == 1 { "y" } else { "ies" },
            crate::core::diagnostics::format_entry_list(&filtered_paths)
        );
        crate::core::diagnostics::push_warning(&mut processing_warnings, ARCHIVE_WARNING_SOURCE, message);
    }

    build_archive_doc_inner(
        extraction_metadata,
        text_contents,
        format_name,
        mime_type,
        children,
        processing_warnings,
    )
}
#[cfg_attr(alef, alef(skip))]
/// ZIP archive extractor.
///
/// Extracts file lists and text content from ZIP archives.
pub struct ZipExtractor;

impl ZipExtractor {
    /// Create a new ZIP extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for ZipExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for ZipExtractor {
    fn name(&self) -> &str {
        "zip-extractor"
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
        "Extracts file lists and text content from ZIP archives"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for ZipExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();

        let cursor = Cursor::new(content);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| crate::error::XbergError::parsing(format!("Failed to read ZIP archive: {}", e)))?;
        let validator = ZipBombValidator::new(limits.clone());
        validator
            .validate(&mut archive)
            .map_err(|e| crate::error::XbergError::validation(e.to_string()))?;

        let extraction_metadata = extract_zip_metadata(content, &limits)?;
        let text_contents = extract_zip_text_content(content, &limits)?;
        let file_bytes = extract_zip_file_bytes(content, &limits)?;
        Ok(build_archive_doc(
            extraction_metadata,
            text_contents,
            file_bytes,
            "ZIP",
            mime_type,
            config,
            0,
        )
        .await)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/zip", "application/x-zip-compressed"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

impl SyncExtractor for ZipExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, config: &ExtractionConfig) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();
        let cursor = Cursor::new(content);
        let mut archive = zip::ZipArchive::new(cursor)
            .map_err(|e| crate::error::XbergError::parsing(format!("Failed to read ZIP archive: {}", e)))?;
        let validator = ZipBombValidator::new(limits.clone());
        validator
            .validate(&mut archive)
            .map_err(|e| crate::error::XbergError::validation(e.to_string()))?;

        let extraction_metadata = extract_zip_metadata(content, &limits)?;
        let text_contents = extract_zip_text_content(content, &limits)?;
        Ok(build_archive_doc_sync(
            extraction_metadata,
            text_contents,
            "ZIP",
            mime_type,
        ))
    }
}
#[cfg_attr(alef, alef(skip))]
/// TAR archive extractor.
///
/// Extracts file lists and text content from TAR archives.
pub struct TarExtractor;

impl TarExtractor {
    /// Create a new TAR extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for TarExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for TarExtractor {
    fn name(&self) -> &str {
        "tar-extractor"
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
        "Extracts file lists and text content from TAR archives"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for TarExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();
        let extraction_metadata = extract_tar_metadata(content, &limits)?;
        let text_contents = extract_tar_text_content(content, &limits)?;
        let file_bytes = extract_tar_file_bytes(content, &limits)?;
        Ok(build_archive_doc(
            extraction_metadata,
            text_contents,
            file_bytes,
            "TAR",
            mime_type,
            config,
            0,
        )
        .await)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &[
            "application/x-tar",
            "application/tar",
            "application/x-gtar",
            "application/x-ustar",
        ]
    }

    fn priority(&self) -> i32 {
        50
    }
}

impl SyncExtractor for TarExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, _config: &ExtractionConfig) -> Result<InternalDocument> {
        let limits = _config.security_limits.clone().unwrap_or_default();
        let extraction_metadata = extract_tar_metadata(content, &limits)?;
        let text_contents = extract_tar_text_content(content, &limits)?;
        Ok(build_archive_doc_sync(
            extraction_metadata,
            text_contents,
            "TAR",
            mime_type,
        ))
    }
}
#[cfg_attr(alef, alef(skip))]
/// 7z archive extractor.
///
/// Extracts file lists and text content from 7z archives.
pub struct SevenZExtractor;

impl SevenZExtractor {
    /// Create a new 7z extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for SevenZExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for SevenZExtractor {
    fn name(&self) -> &str {
        "7z-extractor"
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
        "Extracts file lists and text content from 7z archives"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for SevenZExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();
        let extraction_metadata = extract_7z_metadata(content, &limits)?;
        let text_contents = extract_7z_text_content(content, &limits)?;
        let file_bytes = extract_7z_file_bytes(content, &limits)?;
        Ok(build_archive_doc(
            extraction_metadata,
            text_contents,
            file_bytes,
            "7Z",
            mime_type,
            config,
            0,
        )
        .await)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/x-7z-compressed"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

impl SyncExtractor for SevenZExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, _config: &ExtractionConfig) -> Result<InternalDocument> {
        let limits = _config.security_limits.clone().unwrap_or_default();
        let extraction_metadata = extract_7z_metadata(content, &limits)?;
        let text_contents = extract_7z_text_content(content, &limits)?;
        Ok(build_archive_doc_sync(
            extraction_metadata,
            text_contents,
            "7Z",
            mime_type,
        ))
    }
}
#[cfg_attr(alef, alef(skip))]
/// Gzip archive extractor.
///
/// Decompresses gzip files and extracts text content from the compressed data.
pub struct GzipExtractor;

impl GzipExtractor {
    /// Create a new gzip extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for GzipExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl Plugin for GzipExtractor {
    fn name(&self) -> &str {
        "gzip-extractor"
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
        "Decompresses and extracts text content from gzip-compressed files"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for GzipExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let limits = config.security_limits.clone().unwrap_or_default();
        let (extraction_metadata, text_contents, file_bytes) = extract_gzip_with_bytes(content, &limits)?;
        Ok(build_archive_doc(
            extraction_metadata,
            text_contents,
            file_bytes,
            "GZIP",
            mime_type,
            config,
            0,
        )
        .await)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["application/gzip", "application/x-gzip"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

impl SyncExtractor for GzipExtractor {
    fn extract_sync(&self, content: &[u8], mime_type: &str, _config: &ExtractionConfig) -> Result<InternalDocument> {
        let limits = _config.security_limits.clone().unwrap_or_default();
        let (extraction_metadata, text_contents) = extract_gzip(content, &limits)?;
        Ok(build_archive_doc_sync(
            extraction_metadata,
            text_contents,
            "GZIP",
            mime_type,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use tar::Builder as TarBuilder;
    use zip::write::{FileOptions, ZipWriter};

    #[tokio::test]
    async fn test_zip_extractor() {
        let extractor = ZipExtractor::new();

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options = FileOptions::<'_, ()>::default();

            zip.start_file("test.txt", options).unwrap();
            zip.write_all(b"Hello, World!").unwrap();

            zip.finish().unwrap();
        }

        let bytes = cursor.into_inner();
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(&bytes, "application/zip", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.mime_type, "application/zip");
        assert!(result.content.contains("ZIP Archive"));
        assert!(result.content.contains("test.txt"));
        assert!(result.content.contains("Hello, World!"));
        assert!(result.metadata.format.is_some());
        let archive_meta = match result.metadata.format.as_ref().unwrap() {
            crate::types::FormatMetadata::Archive(meta) => meta,
            _ => panic!("Expected Archive metadata"),
        };
        assert_eq!(archive_meta.format, "ZIP");
        assert_eq!(archive_meta.file_count, 1);
    }

    /// Regression test for #121: member bodies were emitted by iterating an `AHashMap`, whose
    /// order aHash randomizes per process, so the same archive rendered differently on every run.
    ///
    /// The member names below are deliberately anti-alphabetical, so this also pins *which*
    /// deterministic order was chosen: archive order, matching the "Files:" listing — a plain
    /// `sort()` would have produced alpha/middle/zebra and failed here.
    #[tokio::test]
    async fn should_emit_archive_members_in_archive_order_not_hash_order() {
        let extractor = ZipExtractor::new();

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options = FileOptions::<'_, ()>::default();

            for (name, body) in [
                ("zebra.txt", "zebra body"),
                ("alpha.txt", "alpha body"),
                ("middle.txt", "middle body"),
            ] {
                zip.start_file(name, options).unwrap();
                zip.write_all(body.as_bytes()).unwrap();
            }

            zip.finish().unwrap();
        }

        let bytes = cursor.into_inner();
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(&bytes, "application/zip", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        let positions: Vec<usize> = ["=== zebra.txt ===", "=== alpha.txt ===", "=== middle.txt ==="]
            .iter()
            .map(|marker| {
                result
                    .content
                    .find(marker)
                    .unwrap_or_else(|| panic!("{marker} missing from {:?}", result.content))
            })
            .collect();

        assert!(
            positions[0] < positions[1] && positions[1] < positions[2],
            "members must appear in archive order (zebra, alpha, middle); got offsets {positions:?} in {:?}",
            result.content
        );
    }

    #[tokio::test]
    async fn test_zip_filters_bookkeeping_and_binary_junk_from_children() {
        let extractor = ZipExtractor::new();

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options = FileOptions::<'_, ()>::default();

            zip.start_file("report.txt", options).unwrap();
            zip.write_all(b"Quarterly report body.").unwrap();

            // macOS Finder bookkeeping file (binary "Bud1..." header).
            zip.start_file(".DS_Store", options).unwrap();
            zip.write_all(&[0x00, 0x00, 0x00, 0x01, b'B', b'u', b'd', b'1'])
                .unwrap();

            // AppleDouble resource fork sidecar under the macOS archive-utility folder.
            zip.start_file("__MACOSX/._report.txt", options).unwrap();
            zip.write_all(&[
                0x00, 0x05, 0x16, 0x07, 0x00, 0x02, b'M', b'a', b'c', b' ', b'O', b'S', b' ', b'X',
            ])
            .unwrap();

            // AppleDouble sidecar at the top level (same file, no __MACOSX wrapper).
            zip.start_file("._report.txt", options).unwrap();
            zip.write_all(&[
                0x00, 0x05, 0x16, 0x07, 0x00, 0x02, b'M', b'a', b'c', b' ', b'O', b'S', b' ', b'X',
            ])
            .unwrap();

            // Python bytecode cache.
            zip.start_file("__pycache__/mod.cpython-311.pyc", options).unwrap();
            zip.write_all(&[0x42, 0x0d, 0x0d, 0x0a, 0x00, 0x00, 0x00, 0x00])
                .unwrap();

            zip.finish().unwrap();
        }

        let bytes = cursor.into_inner();
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(&bytes, "application/zip", &config)
            .await
            .unwrap();

        let children = result.children.expect("archive should extract the real document");
        assert_eq!(
            children.len(),
            1,
            "only report.txt should survive filtering: {children:?}"
        );
        assert_eq!(children[0].path, "report.txt");
        assert_eq!(children[0].mime_type, "text/plain");

        assert!(
            !result.processing_warnings.is_empty(),
            "expected a ProcessingWarning about filtered bookkeeping/binary entries"
        );
        let warning = &result.processing_warnings[0];
        assert_eq!(warning.source, "archive");
        assert!(
            warning.message.contains("Filtered"),
            "warning message should mention filtering: {}",
            warning.message
        );
    }

    #[tokio::test]
    async fn test_zip_markdown_member_routes_to_markdown_extractor() {
        let markdown = "# Title\n\nBody paragraph.\n\n## Section\n\n- a\n- b\n";

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = ZipWriter::new(&mut cursor);
            let options = FileOptions::<'_, ()>::default();
            zip.start_file("doc.md", options).unwrap();
            zip.write_all(markdown.as_bytes()).unwrap();
            zip.finish().unwrap();
        }

        let bytes = cursor.into_inner();
        let config = ExtractionConfig {
            output_format: crate::core::config::OutputFormat::Markdown,
            ..Default::default()
        };

        let result = ZipExtractor::new()
            .extract_content(&bytes, "application/zip", &config)
            .await
            .unwrap();

        let children = result.children.expect("archive should extract its member");
        let member = children.iter().find(|c| c.path == "doc.md").unwrap();
        assert_eq!(member.mime_type, "text/markdown");

        let rendered = member
            .result
            .formatted_content
            .as_ref()
            .unwrap_or(&member.result.content);
        assert!(rendered.contains("# Title"), "heading lost: {rendered:?}");
        assert!(!rendered.contains("\\#"), "heading was escaped as prose: {rendered:?}");
    }

    #[tokio::test]
    async fn test_tar_extractor() {
        let extractor = TarExtractor::new();

        let mut cursor = Cursor::new(Vec::new());
        {
            let mut tar = TarBuilder::new(&mut cursor);

            let data = b"Hello, World!";
            let mut header = tar::Header::new_gnu();
            header.set_path("test.txt").unwrap();
            header.set_size(data.len() as u64);
            header.set_cksum();
            tar.append(&header, &data[..]).unwrap();

            tar.finish().unwrap();
        }

        let bytes = cursor.into_inner();
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(&bytes, "application/x-tar", &config)
            .await
            .unwrap();
        let result =
            crate::extraction::derive::derive_extraction_result(result, true, crate::core::config::OutputFormat::Plain);

        assert_eq!(result.mime_type, "application/x-tar");
        assert!(result.content.contains("TAR Archive"));
        assert!(result.content.contains("test.txt"));
        assert!(result.content.contains("Hello, World!"));
        assert!(result.metadata.format.is_some());
        let archive_meta = match result.metadata.format.as_ref().unwrap() {
            crate::types::FormatMetadata::Archive(meta) => meta,
            _ => panic!("Expected Archive metadata"),
        };
        assert_eq!(archive_meta.format, "TAR");
        assert_eq!(archive_meta.file_count, 1);
    }

    #[tokio::test]
    async fn test_zip_extractor_invalid() {
        let extractor = ZipExtractor::new();
        let invalid_bytes = vec![0, 1, 2, 3, 4, 5];
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(&invalid_bytes, "application/zip", &config)
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_tar_extractor_invalid() {
        let extractor = TarExtractor::new();
        let invalid_bytes = vec![0, 1, 2, 3, 4, 5];
        let config = ExtractionConfig::default();

        let result = extractor
            .extract_content(&invalid_bytes, "application/x-tar", &config)
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_zip_plugin_interface() {
        let extractor = ZipExtractor::new();
        assert_eq!(extractor.name(), "zip-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert!(extractor.supported_mime_types().contains(&"application/zip"));
        assert_eq!(extractor.priority(), 50);
    }

    #[test]
    fn test_tar_plugin_interface() {
        let extractor = TarExtractor::new();
        assert_eq!(extractor.name(), "tar-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert!(extractor.supported_mime_types().contains(&"application/x-tar"));
        assert!(extractor.supported_mime_types().contains(&"application/tar"));
        assert_eq!(extractor.priority(), 50);
    }

    #[test]
    fn test_gzip_plugin_interface() {
        let extractor = GzipExtractor::new();
        assert_eq!(extractor.name(), "gzip-extractor");
        assert_eq!(extractor.version(), env!("CARGO_PKG_VERSION"));
        assert!(extractor.supported_mime_types().contains(&"application/gzip"));
        assert!(extractor.supported_mime_types().contains(&"application/x-gzip"));
        assert_eq!(extractor.priority(), 50);
    }

    #[tokio::test]
    async fn test_gzip_extractor_valid_data() {
        use flate2::Compression;
        use flate2::write::GzEncoder;
        use std::io::Write;

        let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"Hello from gzip extraction!").unwrap();
        let compressed = encoder.finish().unwrap();

        let extractor = GzipExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(&compressed, "application/gzip", &config)
            .await;
        assert!(result.is_ok());
        let extraction = result.unwrap();
        let extraction = crate::extraction::derive::derive_extraction_result(
            extraction,
            true,
            crate::core::config::OutputFormat::Plain,
        );
        assert!(extraction.content.contains("Hello from gzip extraction!"));
    }

    #[tokio::test]
    async fn test_gzip_extractor_invalid_data() {
        let extractor = GzipExtractor::new();
        let config = ExtractionConfig::default();
        let result = extractor
            .extract_content(&[0, 1, 2, 3], "application/gzip", &config)
            .await;
        assert!(result.is_err());
    }
}
