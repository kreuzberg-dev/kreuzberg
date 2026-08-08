//! Apple iWork format extractors (.pages, .numbers, .key)
//!
//! Supports the modern iWork format (2013+):
//! - `.pages`   — Apple Pages word processor
//! - `.numbers` — Apple Numbers spreadsheet
//! - `.key`     — Apple Keynote presentation
//!
//! ## IWA Container Format
//!
//! Modern iWork files are ZIP archives containing `.iwa` (iWork Archive) files.
//! Each `.iwa` file is:
//! 1. Snappy-compressed using Apple's non-standard framing
//!    (no stream identifier chunk, no CRC-32C — raw Snappy blocks).
//! 2. The decompressed payload is a sequence of protobuf `TSP.ArchiveInfo`-framed
//!    messages from which text strings are extracted using raw wire parsing.

pub mod keynote;
pub mod numbers;
pub mod pages;

use crate::Result;
use crate::error::XbergError;
use crate::extractors::security::{
    SecurityBudget, SecurityError, SecurityLimits, StringGrowthValidator, ZipBombValidator,
};
use crate::text::utf8_validation;
use std::io::Cursor;
use std::io::Read;

/// Maximum size for an individual IWA file to guard against decompression bombs.
const MAX_IWA_DECOMPRESSED_SIZE: usize = 64 * 1024 * 1024;

/// `ProcessingWarning::source` used for every degradation reported by the iWork extractors.
pub(crate) const IWORK_WARNING_SOURCE: &str = "iwork";

/// Record that an IWA archive member could not be parsed and its content was
/// dropped from the output (#106). Callers hit this whenever `read_iwa_file`,
/// `parse_iwa_segments`, or a structured-schema decode fails for a non-Security
/// reason; a `Security` error is never routed here because it aborts the whole
/// extraction instead of skipping one member.
pub(crate) fn push_member_parse_warning(
    warnings: &mut Vec<crate::types::ProcessingWarning>,
    member: &str,
    cause: &dyn std::fmt::Display,
) {
    crate::core::diagnostics::push_warning(
        warnings,
        IWORK_WARNING_SOURCE,
        format!("Failed to parse iWork archive member '{member}'; its content was not extracted (cause: {cause})"),
    );
}

/// Document-wide budget for bytes expanded from nested IWA Snappy streams.
pub(crate) struct IwaExpansionBudget {
    growth: StringGrowthValidator,
    max_size: usize,
}

impl IwaExpansionBudget {
    pub(crate) fn from_limits(limits: &SecurityLimits) -> Self {
        Self {
            growth: StringGrowthValidator::new(limits.max_content_size),
            max_size: limits.max_content_size,
        }
    }

    fn account(&mut self, length: usize) -> Result<()> {
        self.growth.check_append(length)?;
        Ok(())
    }

    fn validate_member_size(&self, size: u64) -> Result<()> {
        let max = self.max_size.min(MAX_IWA_DECOMPRESSED_SIZE);
        if size > max as u64 {
            return Err(SecurityError::ContentTooLarge {
                size: usize::try_from(size).unwrap_or(usize::MAX),
                max,
            }
            .into());
        }
        Ok(())
    }
}

/// Validate the outer iWork ZIP before reading any package members.
pub(crate) fn validate_iwork_zip(content: &[u8], limits: &SecurityLimits) -> Result<()> {
    let cursor = Cursor::new(content);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to open iWork ZIP: {e}")))?;
    ZipBombValidator::new(limits.clone()).validate(&mut archive)?;
    Ok(())
}

/// Collects all .iwa file paths from a ZIP archive.
///
/// Opens the ZIP from `content`, iterates every entry, and returns the names of
/// all entries whose path ends with `.iwa`. Entries that cannot be read are
/// silently skipped (consistent with the per-extractor `filter_map` pattern).
pub(crate) fn collect_iwa_paths(content: &[u8]) -> Result<Vec<String>> {
    let cursor = Cursor::new(content);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to open iWork ZIP: {e}")))?;

    let iwa_paths: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            archive.by_index(i).ok().and_then(|f| {
                let name = f.name().to_string();
                if name.ends_with(".iwa") { Some(name) } else { None }
            })
        })
        .collect();

    Ok(iwa_paths)
}

/// Read and Snappy-decompress a single `.iwa` file from the ZIP archive.
///
/// Apple IWA files use a custom framing format:
/// Each block in the file is: `[type: u8][length: u24 LE][payload: length bytes]`
/// - type `0x00`: Snappy-compressed block → decompress payload with raw Snappy
/// - type `0x01`: Uncompressed block → use payload as-is
///
/// Multiple blocks are concatenated to form the decompressed IWA stream.
pub(crate) fn read_iwa_file(content: &[u8], path: &str, expansion: &mut IwaExpansionBudget) -> Result<Vec<u8>> {
    use std::io::Read;

    let cursor = Cursor::new(content);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| XbergError::parsing(format!("Failed to open iWork ZIP: {e}")))?;

    let mut file = archive
        .by_name(path)
        .map_err(|_| XbergError::parsing(format!("IWA file not found in archive: {path}")))?;

    expansion.validate_member_size(file.size())?;
    let compressed_size = usize::try_from(file.size()).map_err(|_| SecurityError::ContentTooLarge {
        size: usize::MAX,
        max: expansion.max_size.min(MAX_IWA_DECOMPRESSED_SIZE),
    })?;
    let mut raw = Vec::with_capacity(compressed_size.min(MAX_IWA_DECOMPRESSED_SIZE));
    file.read_to_end(&mut raw)
        .map_err(|e| XbergError::parsing(format!("Failed to read IWA file {path}: {e}")))?;

    decode_iwa_stream(&raw, expansion)
}

/// Decode an Apple IWA byte stream into the raw protobuf payload.
///
/// IWA framing: each block = 1 byte type + 3 bytes LE length + N bytes payload
/// - type 0x00 → Snappy-compressed, decompress with `snap::raw::Decoder`
/// - type 0x01 → Uncompressed, use as-is
pub(crate) fn decode_iwa_stream(data: &[u8], expansion: &mut IwaExpansionBudget) -> Result<Vec<u8>> {
    let mut decoder = snap::raw::Decoder::new();
    let mut output = Vec::new();
    let mut i = 0usize;

    while data.len().saturating_sub(i) >= 4 {
        let (chunk_type, payload, next) = read_iwa_chunk(data, i)?;
        i = next;
        append_iwa_chunk(chunk_type, payload, &mut decoder, &mut output, expansion)?;
    }

    if i != data.len() {
        return Err(XbergError::parsing(format!(
            "IWA stream has {} trailing framing bytes",
            data.len() - i
        )));
    }

    Ok(output)
}

fn read_iwa_chunk(data: &[u8], offset: usize) -> Result<(u8, &[u8], usize)> {
    let chunk_type = data[offset];
    let chunk_len =
        (data[offset + 1] as usize) | ((data[offset + 2] as usize) << 8) | ((data[offset + 3] as usize) << 16);
    let payload_offset = offset + 4;
    let end = payload_offset
        .checked_add(chunk_len)
        .ok_or_else(|| XbergError::parsing("IWA chunk offset overflow"))?;
    if end > data.len() {
        return Err(XbergError::parsing(format!(
            "IWA chunk out of bounds: offset={payload_offset}, chunk_len={chunk_len}, data_len={}",
            data.len()
        )));
    }
    Ok((chunk_type, &data[payload_offset..end], end))
}

fn append_iwa_chunk(
    chunk_type: u8,
    payload: &[u8],
    decoder: &mut snap::raw::Decoder,
    output: &mut Vec<u8>,
    expansion: &mut IwaExpansionBudget,
) -> Result<()> {
    match chunk_type {
        0x00 => {
            let length = snap::raw::decompress_len(payload)
                .map_err(|error| XbergError::parsing(format!("Snappy length preflight failed: {error}")))?;
            account_iwa_expansion(output.len(), length, expansion)?;
            let decompressed = decoder
                .decompress_vec(payload)
                .map_err(|error| XbergError::parsing(format!("Snappy decompression failed: {error}")))?;
            output.extend_from_slice(&decompressed);
        }
        0x01 => {
            account_iwa_expansion(output.len(), payload.len(), expansion)?;
            output.extend_from_slice(payload);
        }
        _ => {
            return Err(XbergError::parsing(format!(
                "Unknown IWA chunk type: 0x{chunk_type:02x}"
            )));
        }
    }
    Ok(())
}

fn account_iwa_expansion(current: usize, added: usize, expansion: &mut IwaExpansionBudget) -> Result<()> {
    let expanded_size = current.checked_add(added).ok_or_else(|| {
        XbergError::from(SecurityError::ContentTooLarge {
            size: usize::MAX,
            max: MAX_IWA_DECOMPRESSED_SIZE,
        })
    })?;
    if expanded_size > MAX_IWA_DECOMPRESSED_SIZE {
        return Err(SecurityError::ContentTooLarge {
            size: expanded_size,
            max: MAX_IWA_DECOMPRESSED_SIZE,
        }
        .into());
    }
    expansion.account(added)
}

/// Extract all UTF-8 text strings from a raw protobuf byte slice.
///
/// This uses a simple wire-format scanner without a full schema:
/// - Field type 2 (length-delimited) with a valid UTF-8 payload of ≥3 bytes is
///   treated as a text string candidate.
/// - We skip binary blobs (non-UTF-8) and very short noise strings.
///
/// This approach avoids the need for `prost-build` and generated proto code while
/// still extracting human-readable text reliably from iWork documents.
pub(crate) fn extract_text_from_proto(data: &[u8], budget: &mut SecurityBudget) -> Result<Vec<String>> {
    budget.enter()?;
    let result = extract_proto_fields(data, budget);
    budget.leave();
    result
}

fn extract_proto_fields(data: &[u8], budget: &mut SecurityBudget) -> Result<Vec<String>> {
    let mut texts = Vec::new();
    let mut position = 0usize;
    while position < data.len() {
        budget.step()?;
        let Some((tag, tag_length)) = read_varint(data, position) else {
            break;
        };
        position += tag_length;
        if !extract_proto_field(data, &mut position, tag & 0x7, budget, &mut texts)? {
            break;
        }
    }
    Ok(texts)
}

fn extract_proto_field(
    data: &[u8],
    position: &mut usize,
    wire_type: u64,
    budget: &mut SecurityBudget,
    texts: &mut Vec<String>,
) -> Result<bool> {
    match wire_type {
        0 => Ok(skip_proto_varint(data, position)),
        1 => {
            *position = position.saturating_add(8);
            Ok(true)
        }
        2 => extract_length_delimited_field(data, position, budget, texts),
        5 => {
            *position = position.saturating_add(4);
            Ok(true)
        }
        _ => Ok(false),
    }
}

fn skip_proto_varint(data: &[u8], position: &mut usize) -> bool {
    let Some((_, length)) = read_varint(data, *position) else {
        return false;
    };
    *position += length;
    true
}

fn extract_length_delimited_field(
    data: &[u8],
    position: &mut usize,
    budget: &mut SecurityBudget,
    texts: &mut Vec<String>,
) -> Result<bool> {
    let Some((length, prefix_length)) = read_varint(data, *position) else {
        return Ok(false);
    };
    *position += prefix_length;
    let length =
        usize::try_from(length).map_err(|_| XbergError::parsing("protobuf length does not fit this platform"))?;
    let Some(end) = position.checked_add(length) else {
        return Err(XbergError::parsing("protobuf field offset overflow"));
    };
    if end > data.len() {
        return Ok(false);
    }
    let payload = &data[*position..end];
    *position = end;
    append_proto_text(payload, budget, texts)?;
    texts.extend(extract_text_from_proto(payload, budget)?);
    Ok(true)
}

fn append_proto_text(payload: &[u8], budget: &mut SecurityBudget, texts: &mut Vec<String>) -> Result<()> {
    let Ok(text) = utf8_validation::from_utf8(payload) else {
        return Ok(());
    };
    let trimmed = text.trim();
    // A field is accepted as text once it has *any* alphanumeric character; that
    // condition already implies non-empty, so no separate minimum-length floor
    // is applied. A byte-length floor here (previously 3) silently dropped real
    // short content — single-letter headings, numeric answers, unit labels like
    // "OK", "5", "Q1" (#107).
    if trimmed.chars().any(|character| character.is_alphanumeric()) {
        budget.check_entity(trimmed)?;
        budget.account_text(trimmed.len())?;
        texts.push(trimmed.to_string());
    }
    Ok(())
}

/// Read a protobuf varint from `data` starting at byte `pos`.
///
/// Returns `(value, bytes_consumed)` or `None` if there aren't enough bytes.
fn read_varint(data: &[u8], pos: usize) -> Option<(u64, usize)> {
    let mut result: u64 = 0;
    let mut shift = 0u32;
    let mut i = pos;

    loop {
        if i >= data.len() {
            return None;
        }
        let byte = data[i] as u64;
        i += 1;
        result |= (byte & 0x7F) << shift;
        if byte & 0x80 == 0 {
            return Some((result, i - pos));
        }
        shift += 7;
        if shift >= 64 {
            return None;
        }
    }
}

/// Extract metadata from an iWork ZIP archive.
///
/// Attempts to read `Metadata/Properties.plist` and
/// `Metadata/BuildVersionHistory.plist` from the ZIP. These files are XML plists
/// containing authorship and creation information. If the files cannot be read
/// or parsed, an empty `Metadata` is returned.
pub(crate) fn extract_metadata_from_zip(content: &[u8]) -> crate::types::metadata::Metadata {
    let cursor = Cursor::new(content);
    let Ok(mut archive) = zip::ZipArchive::new(cursor) else {
        return crate::types::metadata::Metadata::default();
    };

    let mut metadata = crate::types::metadata::Metadata::default();

    if let Ok(mut file) = archive.by_name("Metadata/Properties.plist") {
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_ok()
            && let Ok(text) = std::str::from_utf8(&buf)
        {
            parse_plist_metadata(text, &mut metadata);
        }
    }

    if let Ok(mut file) = archive.by_name("Metadata/DocumentIdentifier") {
        let mut buf = Vec::new();
        if file.read_to_end(&mut buf).is_ok()
            && let Ok(text) = std::str::from_utf8(&buf)
        {
            let trimmed = text.trim();
            if !trimmed.is_empty() && metadata.title.is_none() {
                metadata.title = Some(trimmed.to_string());
            }
        }
    }

    metadata
}

/// Parse metadata fields from an XML plist string.
///
/// iWork plist metadata uses `<key>...</key><string>...</string>` pairs.
/// We extract known keys: title, author, keywords, language.
fn parse_plist_metadata(plist: &str, metadata: &mut crate::types::metadata::Metadata) {
    let lines: Vec<&str> = plist.lines().map(|l| l.trim()).collect();
    let mut i = 0;
    while i < lines.len() {
        if let Some(key) = extract_plist_tag(lines[i], "key") {
            let mut j = i + 1;
            while j < lines.len() && lines[j].is_empty() {
                j += 1;
            }
            if j < lines.len()
                && let Some(value) = extract_plist_tag(lines[j], "string")
            {
                match key.as_str() {
                    "title" | "Title" if metadata.title.is_none() => {
                        metadata.title = Some(value);
                    }
                    "author" | "Author" | "creator" | "Creator" => {
                        let authors = metadata.authors.get_or_insert_with(Vec::new);
                        if !authors.contains(&value) {
                            authors.push(value);
                        }
                    }
                    "keywords" | "Keywords" => {
                        let kw = metadata.keywords.get_or_insert_with(Vec::new);
                        for word in value.split(',') {
                            let trimmed = word.trim().to_string();
                            if !trimmed.is_empty() && !kw.contains(&trimmed) {
                                kw.push(trimmed);
                            }
                        }
                    }
                    "language" | "Language" if metadata.language.is_none() => {
                        metadata.language = Some(value);
                    }
                    _ => {}
                }
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }
}

/// Extract the text content of a simple XML tag, e.g. `<string>value</string>`.
fn extract_plist_tag(line: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    if let Some(start) = line.find(&open)
        && let Some(end) = line.find(&close)
    {
        let content = &line[start + open.len()..end];
        return Some(content.to_string());
    }
    None
}

/// Deduplicate a list of text strings while preserving order.
///
/// Only *adjacent* duplicates are collapsed. The iWork wire format sometimes
/// emits the exact same text run twice in a row as a structural artifact of
/// nested protobuf messages (see `extract_length_delimited_field`, which reads
/// a payload as text and then recurses into the same bytes looking for nested
/// fields); removing an immediate repeat like that loses nothing. Removing
/// *non-adjacent* repeats — the previous behavior, which used a `HashSet` over
/// the whole document — deleted legitimately repeated text: a heading reused
/// on two slides, a footer repeated on every page, a word that just happens to
/// appear twice. That was data loss (#101).
pub(crate) fn dedup_text(texts: Vec<String>) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(texts.len());
    for text in texts {
        if result.last() != Some(&text) {
            result.push(text);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_text_from_proto_basic() {
        let text = b"Hello World from iWork";
        let mut proto = vec![0x1A, text.len() as u8];
        proto.extend_from_slice(text);

        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let extracted = extract_text_from_proto(&proto, &mut budget).unwrap();
        assert!(
            extracted.iter().any(|s| s.contains("Hello World")),
            "Should extract the embedded UTF-8 string: {:?}",
            extracted
        );
    }

    #[test]
    fn test_extract_text_from_proto_skips_binary() {
        let binary: Vec<u8> = (0..20).map(|i| i * 7 + 3).collect();
        let mut proto = vec![0x1A, binary.len() as u8];
        proto.extend_from_slice(&binary);

        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let extracted = extract_text_from_proto(&proto, &mut budget).unwrap();
        for s in &extracted {
            assert!(
                !s.chars().all(|c| c.is_alphabetic()),
                "Binary blob should not produce clean alphabetic strings: {s}"
            );
        }
    }

    #[test]
    fn test_extract_text_from_proto_nested() {
        let inner_text = b"Nested Content";
        let mut inner = vec![0x1A, inner_text.len() as u8];
        inner.extend_from_slice(inner_text);

        let mut outer = vec![0x12, inner.len() as u8];
        outer.extend_from_slice(&inner);

        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let extracted = extract_text_from_proto(&outer, &mut budget).unwrap();
        assert!(
            extracted.iter().any(|s| s.contains("Nested Content")),
            "Should extract text from nested protobuf messages: {:?}",
            extracted
        );
    }

    #[test]
    fn should_enforce_protobuf_nesting_limit() {
        let inner_text = b"Nested Content";
        let mut inner = vec![0x1A, inner_text.len() as u8];
        inner.extend_from_slice(inner_text);
        let mut outer = vec![0x12, inner.len() as u8];
        outer.extend_from_slice(&inner);
        let limits = SecurityLimits {
            max_nesting_depth: 1,
            max_xml_depth: 100,
            ..SecurityLimits::default()
        };
        let mut budget = SecurityBudget::for_iwork(&limits);

        assert!(matches!(
            extract_text_from_proto(&outer, &mut budget),
            Err(XbergError::Security { .. })
        ));
    }

    #[test]
    fn should_reject_snappy_expansion_before_decompression() {
        let expanded = vec![b'x'; 1_024];
        let compressed = snap::raw::Encoder::new().compress_vec(&expanded).unwrap();
        let mut framed = vec![0, 0, 0, 0];
        let length = compressed.len();
        framed[1] = (length & 0xff) as u8;
        framed[2] = ((length >> 8) & 0xff) as u8;
        framed[3] = ((length >> 16) & 0xff) as u8;
        framed.extend_from_slice(&compressed);
        let limits = SecurityLimits {
            max_content_size: 128,
            ..SecurityLimits::default()
        };
        let mut expansion = IwaExpansionBudget::from_limits(&limits);

        assert!(matches!(
            decode_iwa_stream(&framed, &mut expansion),
            Err(XbergError::Security { .. })
        ));
    }

    #[test]
    fn should_enforce_document_wide_iwa_expansion_budget() {
        let mut payload = vec![1, 80, 0, 0];
        payload.extend(std::iter::repeat_n(0, 80));
        let limits = SecurityLimits {
            max_content_size: 128,
            ..SecurityLimits::default()
        };
        let mut expansion = IwaExpansionBudget::from_limits(&limits);

        assert_eq!(decode_iwa_stream(&payload, &mut expansion).unwrap().len(), 80);
        assert!(matches!(
            decode_iwa_stream(&payload, &mut expansion),
            Err(XbergError::Security { .. })
        ));
    }

    #[test]
    fn should_reject_unknown_iwa_chunk_type() {
        let limits = SecurityLimits::default();
        let mut expansion = IwaExpansionBudget::from_limits(&limits);

        assert!(decode_iwa_stream(&[2, 0, 0, 0], &mut expansion).is_err());
    }

    #[test]
    fn should_reject_trailing_iwa_framing_bytes() {
        let limits = SecurityLimits::default();
        let mut expansion = IwaExpansionBudget::from_limits(&limits);

        assert!(decode_iwa_stream(&[0, 0, 0], &mut expansion).is_err());
    }

    /// Regression for #101: dedup_text must keep non-adjacent repeats. A global
    /// HashSet-based dedup previously deleted the second "Confidential" even
    /// though it is legitimately repeated content, not a wire-format artifact.
    #[test]
    fn dedup_text_keeps_non_adjacent_repeats_but_collapses_adjacent_ones() {
        let texts = vec![
            "Confidential".to_string(),
            "Confidential".to_string(), // adjacent repeat: wire-format artifact, collapse
            "Body text".to_string(),
            "Confidential".to_string(), // non-adjacent repeat: legitimate content, keep
        ];

        assert_eq!(
            dedup_text(texts),
            vec![
                "Confidential".to_string(),
                "Body text".to_string(),
                "Confidential".to_string(),
            ]
        );
    }

    #[test]
    fn dedup_text_on_empty_input_is_empty() {
        assert_eq!(dedup_text(Vec::new()), Vec::<String>::new());
    }

    /// Regression for #107: a single-character alphanumeric field (a numeric
    /// answer, a unit label) must survive, not be discarded by a byte-length floor.
    #[test]
    fn extract_text_from_proto_keeps_short_alphanumeric_strings() {
        let text = b"5";
        let mut proto = vec![0x1A, text.len() as u8];
        proto.extend_from_slice(text);

        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let extracted = extract_text_from_proto(&proto, &mut budget).unwrap();

        assert_eq!(extracted, vec!["5".to_string()]);
    }

    #[test]
    fn extract_text_from_proto_drops_purely_non_alphanumeric_strings() {
        let text = b"---";
        let mut proto = vec![0x1A, text.len() as u8];
        proto.extend_from_slice(text);

        let mut budget = SecurityBudget::from_limits(&SecurityLimits::default());
        let extracted = extract_text_from_proto(&proto, &mut budget).unwrap();

        assert!(
            extracted.is_empty(),
            "punctuation-only noise should still be dropped: {extracted:?}"
        );
    }

    #[test]
    fn test_collect_iwa_paths_returns_only_iwa() {
        use std::io::Write;

        let mut buf = Vec::new();
        {
            let cursor = std::io::Cursor::new(&mut buf);
            let mut zip = zip::ZipWriter::new(cursor);
            let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
            zip.start_file("Index/Document.iwa", options).unwrap();
            zip.write_all(b"fake iwa content").unwrap();
            zip.start_file("metadata.xml", options).unwrap();
            zip.write_all(b"<xml/>").unwrap();
            zip.finish().unwrap();
        }

        let paths = collect_iwa_paths(&buf).expect("Should list IWA entries");
        assert_eq!(paths.len(), 1, "Should find exactly one .iwa entry");
        assert_eq!(paths[0], "Index/Document.iwa");
    }
}
