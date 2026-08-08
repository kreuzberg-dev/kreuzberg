//! Embedded object extraction from OOXML (DOCX/PPTX) archives.
//!
//! OOXML files are ZIP archives that may contain embedded objects in:
//! - DOCX: `word/embeddings/` directory
//! - PPTX: `ppt/embeddings/` directory
//!
//! This module extracts those embedded files, detects their MIME type,
//! and recursively processes them through the extraction pipeline.

use crate::core::config::ExtractionConfig;
use crate::types::{ArchiveEntry, ProcessingWarning};
use std::borrow::Cow;
use std::io::{Cursor, Read};

/// Extract embedded objects from an OOXML ZIP archive and recursively process them.
///
/// Scans the given `embeddings_prefix` directory (e.g. `word/embeddings/` or
/// `ppt/embeddings/`) inside the ZIP archive for embedded files. Known formats
/// (.xlsx, .pdf, .docx, .pptx, etc.) are recursively extracted. OLE compound
/// files (oleObject*.bin) are skipped with a warning unless their format can be
/// identified.
///
/// Returns `(children, warnings)` suitable for attaching to `InternalDocument`.
pub(crate) async fn extract_ooxml_embedded_objects(
    zip_bytes: &[u8],
    embeddings_prefix: &str,
    source_label: &str,
    config: &ExtractionConfig,
) -> (Vec<ArchiveEntry>, Vec<ProcessingWarning>) {
    let mut children = Vec::new();
    let mut warnings = Vec::new();

    let cursor = Cursor::new(zip_bytes);
    let mut archive = match zip::ZipArchive::new(cursor) {
        Ok(a) => a,
        Err(_) => return (children, warnings),
    };

    let embedding_names: Vec<String> = (0..archive.len())
        .filter_map(|i| {
            let file = archive.by_index(i).ok()?;
            let name = file.name().to_string();
            if name.starts_with(embeddings_prefix) && name.len() > embeddings_prefix.len() {
                Some(name)
            } else {
                None
            }
        })
        .collect();

    if embedding_names.is_empty() {
        return (children, warnings);
    }

    if config.max_archive_depth == 0 {
        warnings.push(ProcessingWarning {
            source: Cow::Owned(format!("{}_embedded_objects", source_label)),
            message: Cow::Owned(format!(
                "Skipped {} embedded object(s) under '{}': max_archive_depth reached",
                embedding_names.len(),
                embeddings_prefix
            )),
        });
        return (children, warnings);
    }

    let mut child_config = config.clone();
    child_config.max_archive_depth = config.max_archive_depth.saturating_sub(1);

    for entry_name in &embedding_names {
        let filename = entry_name
            .strip_prefix(embeddings_prefix)
            .unwrap_or(entry_name)
            .to_string();

        let data = match archive.by_name(entry_name) {
            Ok(mut file) => {
                let mut buf = Vec::with_capacity(file.size() as usize);
                if file.read_to_end(&mut buf).is_err() {
                    warnings.push(ProcessingWarning {
                        source: Cow::Owned(format!("{}_embedded_objects", source_label)),
                        message: Cow::Owned(format!("Failed to read embedded file '{}'", filename)),
                    });
                    continue;
                }
                buf
            }
            Err(_) => continue,
        };

        if data.is_empty() {
            continue;
        }

        if config
            .max_embedded_file_bytes
            .is_some_and(|cap| data.len() as u64 > cap)
        {
            let cap = config.max_embedded_file_bytes.unwrap_or(0);
            warnings.push(ProcessingWarning {
                source: Cow::Owned(format!("{}_embedded_objects", source_label)),
                message: Cow::Owned(format!(
                    "Skipped embedded file '{}': size {} bytes exceeds cap {} bytes",
                    filename,
                    data.len(),
                    cap
                )),
            });
            continue;
        }

        let is_ole_binary = data.len() >= 4 && data[0..4] == [0xD0, 0xCF, 0x11, 0xE0];
        if is_ole_binary {
            match extract_ole_embedded_object(&data) {
                Some((inner_bytes, inner_mime)) => {
                    match crate::core::extractor::extract_bytes(&inner_bytes, &inner_mime, &child_config).await {
                        Ok(result) => {
                            children.push(ArchiveEntry {
                                path: filename,
                                mime_type: inner_mime,
                                result: Box::new(result),
                            });
                        }
                        Err(e) => {
                            warnings.push(ProcessingWarning {
                                source: Cow::Owned(format!("{}_embedded_objects", source_label)),
                                message: Cow::Owned(format!(
                                    "Failed to extract embedded OLE object '{}': {}",
                                    filename, e
                                )),
                            });
                        }
                    }
                }
                None => {
                    warnings.push(ProcessingWarning {
                        source: Cow::Owned(format!("{}_embedded_objects", source_label)),
                        message: Cow::Owned(format!(
                            "Skipped OLE compound file '{}': format identification not supported",
                            filename
                        )),
                    });
                }
            }
            continue;
        }

        let detected_mime = crate::core::mime::detect_mime_type_from_bytes(&data).ok().or_else(|| {
            std::path::Path::new(&filename)
                .extension()
                .and_then(|ext| ext.to_str())
                .and_then(|ext| mime_guess::from_ext(ext).first())
                .map(|m| m.to_string())
        });

        let file_mime = match detected_mime {
            Some(m) if m != "application/octet-stream" => m,
            _ => {
                warnings.push(ProcessingWarning {
                    source: Cow::Owned(format!("{}_embedded_objects", source_label)),
                    message: Cow::Owned(format!(
                        "Skipped embedded file '{}': MIME type could not be determined",
                        filename
                    )),
                });
                continue;
            }
        };

        match crate::core::extractor::extract_bytes(&data, &file_mime, &child_config).await {
            Ok(result) => {
                children.push(ArchiveEntry {
                    path: filename,
                    mime_type: file_mime,
                    result: Box::new(result),
                });
            }
            Err(e) => {
                warnings.push(ProcessingWarning {
                    source: Cow::Owned(format!("{}_embedded_objects", source_label)),
                    message: Cow::Owned(format!("Failed to extract embedded '{}': {}", filename, e)),
                });
            }
        }
    }

    (children, warnings)
}

/// Attempt to identify and unwrap an OLE (CFB) compound-file embedded object.
///
/// Two shapes are recognized:
/// - A "Package" stream: the OLE wrapper carries a modern Office document (e.g. an
///   embedded `.xlsx` chart source) verbatim as an OPC/ZIP package in a stream named
///   `Package`. The stream bytes are returned as-is with their detected MIME type.
/// - A legacy binary root stream (`WordDocument`, `PowerPoint Document`, `Workbook`, or
///   `Book`): the OLE container itself *is* the legacy `.doc`/`.ppt`/`.xls` document, so
///   the original bytes are handed back with the matching legacy MIME type for the
///   existing OLE-aware extractors to parse.
///
/// Returns `None` when the container can't be opened or none of the above streams are
/// present, so the caller can fall back to a "format identification not supported"
/// warning instead of silently dropping the object.
///
/// Only compiled when the `cfb` dependency is guaranteed active (via `office`, `hwp`, or
/// `email`); other feature combinations (e.g. `excel` alone, which also calls this
/// module) keep the pre-existing warn-and-skip behavior.
#[cfg(any(feature = "office", feature = "hwp", feature = "email"))]
fn extract_ole_embedded_object(data: &[u8]) -> Option<(Vec<u8>, String)> {
    let mut compound_file = cfb::CompoundFile::open(Cursor::new(data)).ok()?;

    if compound_file.exists("Package") {
        let mut stream = compound_file.open_stream("Package").ok()?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).ok()?;
        if buf.is_empty() {
            return None;
        }
        let mime = crate::core::mime::detect_mime_type_from_bytes(&buf).ok()?;
        return Some((buf, mime));
    }

    let legacy_mime = if compound_file.exists("WordDocument") {
        "application/msword"
    } else if compound_file.exists("PowerPoint Document") {
        "application/vnd.ms-powerpoint"
    } else if compound_file.exists("Workbook") || compound_file.exists("Book") {
        "application/vnd.ms-excel"
    } else {
        return None;
    };

    Some((data.to_vec(), legacy_mime.to_string()))
}

/// Fallback used when the `cfb` dependency isn't active for the enabled feature set
/// (e.g. `excel` without `office`/`hwp`/`email`): OLE objects are always reported as
/// unidentifiable rather than attempting extraction.
#[cfg(not(any(feature = "office", feature = "hwp", feature = "email")))]
fn extract_ole_embedded_object(_data: &[u8]) -> Option<(Vec<u8>, String)> {
    None
}

#[cfg(all(test, feature = "office"))]
mod tests {
    use super::*;
    use std::io::Write;

    /// Build a minimal ZIP in memory with one file at the given path and contents.
    fn make_zip_with_file(entry_path: &str, entry_data: &[u8]) -> Vec<u8> {
        let buf = Cursor::new(Vec::new());
        let mut zip = zip::ZipWriter::new(buf);
        let options = zip::write::FileOptions::<()>::default().compression_method(zip::CompressionMethod::Stored);
        zip.start_file(entry_path, options).unwrap();
        zip.write_all(entry_data).unwrap();
        zip.finish().unwrap().into_inner()
    }

    #[tokio::test]
    async fn test_embedded_file_over_cap_skipped_with_warning() {
        let data = b"Hello world! This is a test document.";
        let zip_bytes = make_zip_with_file("word/embeddings/doc.txt", data);

        let config = ExtractionConfig {
            max_embedded_file_bytes: Some(10),
            ..Default::default()
        };

        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert!(
            children.is_empty(),
            "oversized embedded file must not produce a child entry"
        );
        assert_eq!(warnings.len(), 1, "exactly one warning expected");
        assert!(
            warnings[0].message.contains("exceeds cap"),
            "warning must mention cap: {}",
            warnings[0].message
        );
        assert!(
            warnings[0].message.contains("doc.txt"),
            "warning must name the file: {}",
            warnings[0].message
        );
    }

    #[tokio::test]
    async fn test_embedded_file_under_cap_proceeds_to_extraction() {
        let data = b"Hello";
        let zip_bytes = make_zip_with_file("word/embeddings/note.txt", data);

        let config = ExtractionConfig {
            max_embedded_file_bytes: Some(1024 * 1024),
            ..Default::default()
        };

        let (_children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        let cap_warnings: Vec<_> = warnings.iter().filter(|w| w.message.contains("exceeds cap")).collect();
        assert!(cap_warnings.is_empty(), "no size-cap warning expected for small file");
    }

    #[tokio::test]
    async fn test_embedded_file_no_cap_proceeds() {
        let data = b"some content";
        let zip_bytes = make_zip_with_file("word/embeddings/file.txt", data);

        let config = ExtractionConfig {
            max_embedded_file_bytes: None,
            ..Default::default()
        };

        let (_children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        let cap_warnings: Vec<_> = warnings.iter().filter(|w| w.message.contains("exceeds cap")).collect();
        assert!(cap_warnings.is_empty(), "no size-cap warning when cap is None");
    }

    /// Build a CFB (OLE compound file) with a single "Package" stream holding `payload`,
    /// the shape OLE object wrappers use to embed a modern Office (OPC/ZIP) document
    /// verbatim.
    fn make_ole_package(payload: &[u8]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut comp = cfb::CompoundFile::create(cursor).expect("create CFB container");
        {
            let mut stream = comp.create_stream("Package").expect("create Package stream");
            stream.write_all(payload).unwrap();
        }
        comp.into_inner().into_inner()
    }

    /// Build a CFB with a single named stream (e.g. "WordDocument"), simulating a legacy
    /// binary Office document embedded directly as an OLE compound file.
    fn make_ole_with_stream(stream_name: &str, payload: &[u8]) -> Vec<u8> {
        let cursor = Cursor::new(Vec::new());
        let mut comp = cfb::CompoundFile::create(cursor).expect("create CFB container");
        {
            let mut stream = comp.create_stream(stream_name).expect("create stream");
            stream.write_all(payload).unwrap();
        }
        comp.into_inner().into_inner()
    }

    /// Path to the shared `test_documents/` corpus (two levels up from this crate).
    fn test_documents_dir() -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("test_documents")
    }

    /// Gated on `excel` as well as `office`: the payload is an .xlsx, so without the
    /// excel extractor registered the recursive extraction correctly reports
    /// `UnsupportedFormat` and produces no child. The test would then fail for a reason
    /// that has nothing to do with OLE Package unwrapping, which is what it exists to
    /// cover. Observed under `--features "email,office,ocr,transcription"`.
    #[cfg(feature = "excel")]
    #[tokio::test]
    async fn test_ole_package_stream_extracted_as_embedded_xlsx() {
        let fixture = test_documents_dir().join("xlsx/excel_tiny_excel.xlsx");
        if !fixture.exists() {
            eprintln!(
                "Skipping test: test_documents/ fixture not found at {}",
                fixture.display()
            );
            return;
        }
        let xlsx_bytes = std::fs::read(&fixture).expect("read fixture xlsx");

        let ole_bytes = make_ole_package(&xlsx_bytes);
        let zip_bytes = make_zip_with_file("word/embeddings/oleObject1.bin", &ole_bytes);

        let config = ExtractionConfig::default();
        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert_eq!(
            children.len(),
            1,
            "the OLE Package stream must be unwrapped and recursively extracted; warnings: {:?}",
            warnings
        );
        assert!(
            children[0].mime_type.contains("spreadsheet") || children[0].mime_type.contains("excel"),
            "expected an Excel MIME type, got '{}'",
            children[0].mime_type
        );
    }

    #[tokio::test]
    async fn test_legacy_word_document_ole_stream_is_identified_not_skipped() {
        // The WordDocument content doesn't need to be a well-formed FIB for this test: we
        // only assert that the OLE container was recognized and routed to the legacy
        // `.doc` MIME type instead of being reported as unidentifiable outright. Whether
        // the FIB itself parses is covered by `extraction::doc` tests.
        let ole_bytes = make_ole_with_stream("WordDocument", b"not-a-real-fib-but-present");
        let zip_bytes = make_zip_with_file("word/embeddings/oleObject2.bin", &ole_bytes);

        let config = ExtractionConfig::default();
        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert!(children.is_empty());
        assert_eq!(warnings.len(), 1, "expected exactly one warning: {:?}", warnings);
        assert!(
            !warnings[0].message.contains("format identification not supported"),
            "a recognized WordDocument stream must not be reported as unidentifiable: {}",
            warnings[0].message
        );
    }

    #[tokio::test]
    async fn test_unidentifiable_ole_container_still_warns() {
        let ole_bytes = make_ole_with_stream("SomeUnknownStream", b"opaque binary data");
        let zip_bytes = make_zip_with_file("word/embeddings/oleObject3.bin", &ole_bytes);

        let config = ExtractionConfig::default();
        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert!(children.is_empty());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].message.contains("format identification not supported"));
        assert!(warnings[0].message.contains("oleObject3.bin"));
    }

    #[tokio::test]
    async fn test_undetectable_mime_now_warns_instead_of_silent_skip() {
        // Bytes with no recognizable magic, invalid as UTF-8 (so the plain-text fallback
        // doesn't kick in either), and no file extension: MIME detection must fail for
        // both the byte-sniffing and extension-based fallback paths.
        let data = vec![0x80u8, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87];
        let zip_bytes = make_zip_with_file("word/embeddings/mystery_blob", &data);

        let config = ExtractionConfig::default();
        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert!(children.is_empty());
        assert_eq!(warnings.len(), 1, "expected exactly one warning: {:?}", warnings);
        assert!(
            warnings[0].message.contains("mystery_blob"),
            "warning must name the file: {}",
            warnings[0].message
        );
        assert!(
            warnings[0].message.contains("MIME type could not be determined"),
            "warning must explain why the file was skipped: {}",
            warnings[0].message
        );
    }

    #[tokio::test]
    async fn test_depth_exhausted_with_embeddings_present_warns() {
        let data = b"Hello world! This is a test document.";
        let zip_bytes = make_zip_with_file("word/embeddings/doc.txt", data);

        let config = ExtractionConfig {
            max_archive_depth: 0,
            ..Default::default()
        };

        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert!(children.is_empty());
        assert_eq!(warnings.len(), 1, "expected exactly one warning: {:?}", warnings);
        assert!(
            warnings[0].message.contains("max_archive_depth"),
            "warning must explain why embeddings were skipped: {}",
            warnings[0].message
        );
    }

    #[tokio::test]
    async fn test_depth_exhausted_with_no_embeddings_does_not_warn() {
        let zip_bytes = make_zip_with_file("word/document.xml", b"<w:document/>");

        let config = ExtractionConfig {
            max_archive_depth: 0,
            ..Default::default()
        };

        let (children, warnings) =
            extract_ooxml_embedded_objects(&zip_bytes, "word/embeddings/", "test", &config).await;

        assert!(children.is_empty());
        assert!(
            warnings.is_empty(),
            "no embeddings exist, so no depth warning should be emitted: {:?}",
            warnings
        );
    }
}
