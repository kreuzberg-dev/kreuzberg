/// CFB stream reading and decompression utilities.
///
/// Consolidated from hwpers reader/cfb.rs, reader/stream.rs, and utils/compression.rs.
use super::error::{HwpError, Result};
use cfb::CompoundFile;
use flate2::read::{DeflateDecoder, ZlibDecoder};
use std::io::{Cursor, Read, Seek};

// ---------------------------------------------------------------------------

/// Thin wrapper around a CFB `CompoundFile` that reads named streams from HWP documents.
#[cfg_attr(alef, alef(skip))]
pub struct CfbReader<F> {
    cfb: CompoundFile<F>,
}

impl CfbReader<Cursor<Vec<u8>>> {
    /// Open a CFB compound file from raw bytes.
    pub(crate) fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let cursor = Cursor::new(bytes.to_vec());
        let cfb = CompoundFile::open(cursor).map_err(|e| HwpError::Cfb(format!("Failed to open CFB: {e}")))?;
        Ok(Self { cfb })
    }
}

impl<F: Read + Seek> CfbReader<F> {
    /// Read a named stream into a `Vec<u8>`.
    pub(crate) fn read_stream(&mut self, path: &str) -> Result<Vec<u8>> {
        let mut stream = self
            .cfb
            .open_stream(path)
            .map_err(|e| HwpError::NotFound(format!("Stream '{path}' not found: {e}")))?;
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf)?;
        Ok(buf)
    }

    /// Return `true` if the named stream exists in the compound file.
    pub(crate) fn stream_exists(&self, path: &str) -> bool {
        self.cfb.exists(path)
    }

    /// List all streams in the compound file, as root-relative paths.
    ///
    /// `cfb` reports entry paths absolutely (`/BodyText/Section0`), while every caller
    /// here — and [`Self::read_stream`], which resolves relative to the root — speaks in
    /// relative names (`BodyText/Section0`). Returning the absolute form made every
    /// `starts_with("BodyText/Section")` and `starts_with("BinData/")` test fail, so no
    /// section and no image was ever found in a real HWP file.
    pub(crate) fn list_streams(&self) -> Vec<String> {
        self.cfb
            .walk()
            .filter_map(|entry| {
                if entry.is_stream() {
                    let path = entry.path().to_string_lossy().into_owned();
                    Some(path.trim_start_matches('/').to_string())
                } else {
                    None
                }
            })
            .collect()
    }
}

/// Cursor-backed little-endian binary reader for HWP stream payloads.
#[cfg_attr(alef, alef(skip))]
pub struct StreamReader {
    cursor: Cursor<Vec<u8>>,
}

impl StreamReader {
    pub(crate) fn new(data: Vec<u8>) -> Self {
        Self {
            cursor: Cursor::new(data),
        }
    }

    #[cfg_attr(alef, alef(skip))]
    pub(crate) fn read_u16(&mut self) -> Result<u16> {
        let mut buf = [0u8; 2];
        self.cursor.read_exact(&mut buf)?;
        Ok(u16::from_le_bytes(buf))
    }

    #[cfg_attr(alef, alef(skip))]
    pub(crate) fn read_u32(&mut self) -> Result<u32> {
        let mut buf = [0u8; 4];
        self.cursor.read_exact(&mut buf)?;
        Ok(u32::from_le_bytes(buf))
    }

    #[cfg_attr(alef, alef(skip))]
    pub(crate) fn read_bytes(&mut self, len: usize) -> Result<Vec<u8>> {
        let mut buf = vec![0u8; len];
        self.cursor.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Number of bytes remaining from the current position to the end.
    pub(crate) fn remaining(&self) -> usize {
        let pos = self.cursor.position() as usize;
        let len = self.cursor.get_ref().len();
        len.saturating_sub(pos)
    }
}

/// Decompress a raw-deflate stream from an HWP section.
///
/// HWP 5.0 compresses sections with raw deflate (no zlib header). Falls back
/// to zlib if raw deflate fails, and returns the data as-is if both fail.
pub(crate) fn decompress_stream(data: &[u8]) -> Result<Vec<u8>> {
    if data.is_empty() {
        return Ok(Vec::new());
    }

    let mut decoder = DeflateDecoder::new(data);
    let mut decompressed = Vec::new();
    if decoder.read_to_end(&mut decompressed).is_ok() {
        return Ok(decompressed);
    }

    let mut decoder = ZlibDecoder::new(data);
    let mut decompressed = Vec::new();
    if decoder.read_to_end(&mut decompressed).is_ok() {
        return Ok(decompressed);
    }

    Ok(data.to_vec())
}
