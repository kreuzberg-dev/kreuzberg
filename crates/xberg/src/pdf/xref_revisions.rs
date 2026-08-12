//! PDF incremental-update history via xref chain walk.
//!
//! PDFs support incremental updates: each save appends a new `xref` section and
//! `trailer` to the end of the file, with the trailer's `/Prev` key pointing to
//! the previous xref byte offset. This module surfaces those historical saves as
//! [`DocumentRevision`] values.
//!
//! # What is extracted
//!
//! For each historical xref section (all but the current/latest one) we emit one
//! [`DocumentRevision`]:
//!
//! - `revision_id` — `"xref-offset-{N}"` where N is the byte offset of the xref.
//! - `author` — `/Author` from the `/Info` dictionary when present.
//! - `timestamp` — `/ModDate` (preferred) or `/CreationDate` from `/Info`, parsed
//!   from the PDF date format into ISO-8601 when feasible.
//! - `kind` — [`RevisionKind::Insertion`] is used as a placeholder; PDF incremental
//!   updates carry no typed change classification analogous to DOCX w:ins/w:del.
//!   ([`RevisionKind`] is not `#[non_exhaustive]`, so adding a new `Snapshot` variant
//!   would be a breaking change — deferred until the enum is marked non_exhaustive.)
//! - `anchor` — `None`; whole-file revisions have no natural paragraph-level anchor.
//! - `delta` — [`RevisionDelta::default()`]; per-revision content extraction requires
//!   re-parsing the document at each historical xref state and is deferred.
//!
//! # Algorithm
//!
//! The raw PDF bytes are scanned backwards for every `%%EOF` marker. Just before
//! each `%%EOF` we search for the preceding `startxref\n<offset>` line. This gives
//! us the byte offset of every xref section in the file. We then walk the `/Prev`
//! chain from the final xref backwards to build a sorted list of historical offsets
//! (excluding the current/latest xref that represents the live state).
//!
//! For `/Info` metadata we use the already-loaded `lopdf::Document`, which merges
//! all objects across revisions, so author and date are resolved from the current
//! object graph.
//!
//! # No-revision case
//!
//! Single-save PDFs (no `/Prev` in the trailer) return `None`. Only PDFs with at
//! least one prior xref section return `Some(Vec<DocumentRevision>)`.

use crate::types::revisions::{DocumentRevision, RevisionDelta, RevisionKind};

/// Maximum number of revisions to emit. Guards against pathological inputs with
/// thousands of incremental updates — each revision is a cheap struct allocation
/// but we cap to keep output size bounded.
const MAX_REVISIONS: usize = 128;

/// Number of leading bytes of a PDF date string's digit portion that
/// [`parse_pdf_date_string`] requires to be pure ASCII before it will slice
/// out a date-only (`YYYYMMDD`) timestamp.
const DATE_ONLY_ASCII_BYTES: usize = 8;

/// Number of leading bytes of a PDF date string's digit portion that
/// [`parse_pdf_date_string`] requires to be pure ASCII before it will slice
/// out a full (`YYYYMMDDHHmmSS`) timestamp.
const FULL_TIMESTAMP_ASCII_BYTES: usize = 14;

/// Maximum search window (bytes from end of file) when scanning for `startxref`.
/// Covers even large PDF trailers.
#[cfg(test)]
const EOF_SCAN_WINDOW: usize = 1024;

/// Scan PDF `content` for all `startxref` byte offsets present in the file.
///
/// Returns offsets in the order they appear in the file (first save → last save),
/// with the final entry being the current/latest xref. Returns an empty `Vec` on
/// parse failure.
///
/// Used in tests to verify the file-level xref count independently of the `/Prev`
/// chain walk done in production by [`collect_prev_chain`].
#[cfg(test)]
fn collect_startxref_offsets(content: &[u8]) -> Vec<usize> {
    let mut offsets: Vec<usize> = Vec::new();
    let len = content.len();

    let mut search_start = 0usize;
    while search_start < len {
        let Some(eof_pos) = find_subsequence(&content[search_start..], b"%%EOF").map(|p| p + search_start) else {
            break;
        };

        let window_start = eof_pos.saturating_sub(EOF_SCAN_WINDOW);
        let window = &content[window_start..eof_pos];
        if let Some(sx_rel) = find_last_subsequence(window, b"startxref") {
            let sx_abs = window_start + sx_rel;
            let after = sx_abs + b"startxref".len();
            if let Some(offset) = parse_decimal_after(content, after)
                && offset < len
                && !offsets.contains(&offset)
            {
                offsets.push(offset);
            }
        }

        search_start = eof_pos + b"%%EOF".len();
    }

    offsets.sort_unstable();
    offsets
}

/// Walk the `/Prev` chain stored in the raw PDF bytes starting at `xref_offset`.
///
/// Returns a list of historical xref byte offsets that are reachable via `/Prev`
/// links, in oldest-to-newest order (earliest save first). The offset of the
/// starting (current) xref is **not** included in the returned list.
///
/// If the trailer at `xref_offset` has no `/Prev`, returns an empty `Vec`.
fn collect_prev_chain(content: &[u8], xref_offset: usize) -> Vec<usize> {
    let mut chain: Vec<usize> = Vec::new();
    let mut current = xref_offset;
    let mut seen: Vec<usize> = Vec::new();

    loop {
        if seen.contains(&current) {
            break;
        }
        seen.push(current);

        let slice = &content[current..];
        let prev = extract_prev_from_trailer(slice);
        match prev {
            Some(p) if p < content.len() && p != current => {
                chain.push(p);
                current = p;
            }
            _ => break,
        }

        if chain.len() >= MAX_REVISIONS {
            break;
        }
    }

    chain.reverse();
    chain
}

/// Attempt to extract the `/Prev` integer from a trailer dictionary found in
/// the given byte slice (which should start at an xref section).
///
/// Scans forward for the word `trailer` then looks for `/Prev` in the
/// subsequent dictionary text.
fn extract_prev_from_trailer(slice: &[u8]) -> Option<usize> {
    let trailer_pos = find_subsequence(slice, b"trailer")?;
    let after_trailer = &slice[trailer_pos + b"trailer".len()..];

    let dict_start = find_subsequence(after_trailer, b"<<")?;
    let dict_slice = &after_trailer[dict_start..];

    let dict_end = find_subsequence(dict_slice, b">>")?;
    let dict_content = &dict_slice[..dict_end + 2];

    let prev_key = b"/Prev";
    let prev_pos = find_subsequence(dict_content, prev_key)?;
    let after_prev = &dict_content[prev_pos + prev_key.len()..];

    let trimmed = trim_leading_whitespace(after_prev);
    parse_decimal_value(trimmed)
}

/// Extract `/Info` metadata from a `lopdf::Document`.
///
/// Returns `(author, timestamp)` where `timestamp` is the `/ModDate` field
/// (preferred) or `/CreationDate` if `/ModDate` is absent.
///
/// Uses `lopdf` directly (already a dependency for bookmarks extraction) so we
/// share the loaded document rather than re-parsing.
fn extract_lopdf_info_metadata(document: &lopdf::Document) -> (Option<String>, Option<String>) {
    use lopdf::Object;

    let info_id = match document
        .trailer
        .get(b"Info")
        .ok()
        .and_then(|obj| obj.as_reference().ok())
    {
        Some(id) => id,
        None => return (None, None),
    };

    let info_dict = match document.get_object(info_id) {
        Ok(Object::Dictionary(dict)) => dict,
        _ => return (None, None),
    };

    let author = info_dict.get(b"Author").ok().and_then(extract_lopdf_string);

    // `parse_pdf_date_string` refuses to reformat a date string that isn't pure
    // ASCII where it needs to slice (GH#1422). On that error we fall back to the
    // original (untouched) string rather than surfacing a hard failure — a
    // malformed date is still useful revision metadata.
    let timestamp = info_dict
        .get(b"ModDate")
        .ok()
        .and_then(extract_lopdf_string)
        .or_else(|| info_dict.get(b"CreationDate").ok().and_then(extract_lopdf_string))
        .map(|raw| parse_pdf_date_string(&raw).unwrap_or(raw));

    (author, timestamp)
}

/// Decode a `lopdf` string or name object to a Rust `String`.
fn extract_lopdf_string(obj: &lopdf::Object) -> Option<String> {
    use lopdf::Object;
    match obj {
        Object::String(bytes, _) => {
            if bytes.len() >= 2 && bytes[0] == 0xFE && bytes[1] == 0xFF {
                let u16s: Vec<u16> = bytes[2..]
                    .chunks_exact(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                let s = String::from_utf16_lossy(&u16s);
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() { None } else { Some(trimmed) }
            } else {
                let s = String::from_utf8_lossy(bytes);
                let trimmed = s.trim().to_string();
                if trimmed.is_empty() { None } else { Some(trimmed) }
            }
        }
        Object::Name(bytes) => {
            let s = String::from_utf8_lossy(bytes);
            let trimmed = s.trim().to_string();
            if trimmed.is_empty() { None } else { Some(trimmed) }
        }
        _ => None,
    }
}

/// A PDF `/ModDate`/`/CreationDate` string was not pure ASCII in the byte range
/// [`parse_pdf_date_string`] needs to slice to extract date components.
///
/// PDF date strings are ASCII by spec (ISO 32000-1 §7.9.4: `D:YYYYMMDDHHmmSS…`).
/// [`extract_lopdf_string`] decodes the raw `/Info` bytes with
/// `String::from_utf8_lossy`, which silently substitutes any invalid byte
/// sequence with the 3-byte replacement character `U+FFFD`. If that
/// substitution lands inside the fixed byte offsets this function slices at
/// (e.g. `&digits[..4]` for the year), the offset no longer falls on a `char`
/// boundary and slicing would panic — this was GH#1422, which aborted the
/// whole process (SIGABRT) when the panic crossed the Go FFI/cgo boundary,
/// since a panic cannot unwind through a non-Rust-created OS thread. We
/// detect the non-ASCII byte up front and return an error instead of ever
/// taking the slice.
#[derive(Debug, thiserror::Error)]
#[error(
    "parsing PDF date string failed: the first {required_ascii_bytes} byte(s) of {raw:?} are \
     not pure ASCII, so slicing them into date components would not land on a char boundary; \
     this usually means the /Info string was decoded from non-UTF-8 bytes via a lossy \
     conversion — treat the date as unparseable and fall back to the raw string"
)]
struct NonAsciiPdfDateError {
    /// The original (untruncated) `/ModDate` or `/CreationDate` string.
    raw: String,
    /// How many leading bytes of the digit portion were required to be ASCII.
    required_ascii_bytes: usize,
}

/// Parse a PDF date string of the form `D:YYYYMMDDHHmmSS…` into ISO-8601.
///
/// On malformed (too-short) input the raw string is returned unchanged. Returns
/// [`NonAsciiPdfDateError`] rather than panicking when the region that must be
/// sliced contains non-ASCII bytes — see that type's documentation for why this
/// can happen even though PDF dates are nominally ASCII-only.
fn parse_pdf_date_string(raw: &str) -> Result<String, NonAsciiPdfDateError> {
    let cleaned = raw.trim();
    let digits = if let Some(stripped) = cleaned.strip_prefix("D:") {
        stripped
    } else {
        cleaned
    };

    if digits.len() < DATE_ONLY_ASCII_BYTES {
        return Ok(raw.to_string());
    }

    let required_ascii_bytes = if digits.len() >= FULL_TIMESTAMP_ASCII_BYTES {
        FULL_TIMESTAMP_ASCII_BYTES
    } else {
        DATE_ONLY_ASCII_BYTES
    };

    // Slicing `digits` at the fixed byte offsets below is only safe once we know
    // the region being sliced is pure ASCII: ASCII bytes are always exactly one
    // byte per `char`, so any offset inside an all-ASCII prefix is guaranteed to
    // land on a char boundary. `bytes::is_ascii` scans the whole sub-slice, so
    // this never itself risks a non-boundary panic (byte slicing has no char
    // boundary concept). ~keep
    let prefix_is_ascii = digits
        .as_bytes()
        .get(..required_ascii_bytes)
        .is_some_and(<[u8]>::is_ascii);
    if !prefix_is_ascii {
        return Err(NonAsciiPdfDateError {
            raw: raw.to_string(),
            required_ascii_bytes,
        });
    }

    let year = &digits[..4];
    let month = &digits[4..6];
    let day = &digits[6..8];
    if required_ascii_bytes >= FULL_TIMESTAMP_ASCII_BYTES {
        let hour = &digits[8..10];
        let min = &digits[10..12];
        let sec = &digits[12..14];
        Ok(format!("{year}-{month}-{day}T{hour}:{min}:{sec}Z"))
    } else {
        Ok(format!("{year}-{month}-{day}T00:00:00Z"))
    }
}

/// Find the first occurrence of `needle` in `haystack`. Returns the byte position.
fn find_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

/// Find the *last* occurrence of `needle` in `haystack`.
#[cfg(test)]
fn find_last_subsequence(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).rposition(|window| window == needle)
}

/// Skip leading ASCII whitespace bytes (space, tab, CR, LF) and return the rest.
fn trim_leading_whitespace(bytes: &[u8]) -> &[u8] {
    let skip = bytes
        .iter()
        .position(|&b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap_or(bytes.len());
    &bytes[skip..]
}

/// Parse the first ASCII decimal integer from `bytes`, ignoring leading whitespace.
fn parse_decimal_value(bytes: &[u8]) -> Option<usize> {
    let bytes = trim_leading_whitespace(bytes);
    let end = bytes.iter().position(|b| !b.is_ascii_digit()).unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    std::str::from_utf8(&bytes[..end])
        .ok()
        .and_then(|s| s.parse::<usize>().ok())
}

/// Parse the first decimal integer from `content` starting at byte `after`,
/// ignoring leading whitespace and newlines.
#[cfg(test)]
fn parse_decimal_after(content: &[u8], after: usize) -> Option<usize> {
    parse_decimal_value(&content[after.min(content.len())..])
}

/// Extract `DocumentRevision` entries from the incremental-update xref chain in `content`.
///
/// Returns `None` for single-save PDFs (no `/Prev` in the trailer). Returns
/// `Some(revisions)` where each entry corresponds to one historical xref section,
/// ordered oldest-first. The current/latest xref is not included — it represents
/// the live state of the document.
///
/// # Fields populated
///
/// - `revision_id`: `"xref-offset-{N}"` (byte offset of the historical xref).
/// - `author`: from `/Info/Author` via the already-loaded `lopdf::Document`.
/// - `timestamp`: from `/Info/ModDate` or `/Info/CreationDate`, ISO-8601 when parseable.
/// - `kind`: [`RevisionKind::Insertion`] as placeholder (PDF revisions carry no typed
///   change classification; [`RevisionKind`] is not `#[non_exhaustive]` so we cannot
///   add `Snapshot` without a breaking change).
/// - `anchor`: `None` — whole-file revisions have no paragraph-level anchor.
/// - `delta`: [`RevisionDelta::default()`] — per-revision content extraction deferred.
pub(crate) fn extract_pdf_xref_revisions(content: &[u8], document: &lopdf::Document) -> Option<Vec<DocumentRevision>> {
    let final_offset = document.xref_start;
    let historical_offsets = collect_prev_chain(content, final_offset);

    if historical_offsets.is_empty() {
        return None;
    }

    let (author, timestamp) = extract_lopdf_info_metadata(document);

    let revisions: Vec<DocumentRevision> = historical_offsets
        .into_iter()
        .take(MAX_REVISIONS)
        .map(|offset| DocumentRevision {
            revision_id: format!("xref-offset-{offset}"),
            author: author.clone(),
            timestamp: timestamp.clone(),
            kind: RevisionKind::Insertion,
            anchor: None,
            delta: RevisionDelta::default(),
        })
        .collect();

    if revisions.is_empty() { None } else { Some(revisions) }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a valid minimal single-page PDF as bytes.
    ///
    /// Structure:
    /// ```text
    /// %PDF-1.4
    /// 1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
    /// 2 0 obj<</Type/Pages/Kids[3 0 R]/Count 1>>endobj
    /// 3 0 obj<</Type/Page/MediaBox[0 0 612 792]/Parent 2 0 R>>endobj
    /// xref
    /// 0 4
    /// <free entry>
    /// <obj 1 offset>
    /// <obj 2 offset>
    /// <obj 3 offset>
    /// trailer<</Size 4/Root 1 0 R>>
    /// startxref
    /// <xref_offset>
    /// %%EOF
    /// ```
    fn build_minimal_pdf() -> Vec<u8> {
        let mut buf = Vec::<u8>::new();

        let header = b"%PDF-1.4\n";
        buf.extend_from_slice(header);

        let obj1_offset = buf.len();
        buf.extend_from_slice(b"1 0 obj\n<</Type /Catalog /Pages 2 0 R>>\nendobj\n");

        let obj2_offset = buf.len();
        buf.extend_from_slice(b"2 0 obj\n<</Type /Pages /Kids [3 0 R] /Count 1>>\nendobj\n");

        let obj3_offset = buf.len();
        buf.extend_from_slice(b"3 0 obj\n<</Type /Page /MediaBox [0 0 612 792] /Parent 2 0 R>>\nendobj\n");

        let xref_offset = buf.len();

        buf.extend_from_slice(b"xref\n");
        buf.extend_from_slice(b"0 4\n");
        buf.extend_from_slice(b"0000000000 65535 f \n");
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj1_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj2_offset).as_bytes());
        buf.extend_from_slice(format!("{:010} 00000 n \n", obj3_offset).as_bytes());

        buf.extend_from_slice(b"trailer\n<</Size 4 /Root 1 0 R>>\n");

        buf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", xref_offset).as_bytes());

        buf
    }

    /// Build a two-revision PDF: append an incremental update to `base` that adds
    /// a trivial new object and sets `/Prev` in its trailer.
    ///
    /// The incremental update structure:
    /// ```text
    /// 4 0 obj<</Update true>>endobj
    /// xref
    /// 4 1
    /// <new obj offset>
    /// trailer<</Size 5/Root 1 0 R/Prev <base_xref_offset>>>
    /// startxref
    /// <new xref offset>
    /// %%EOF
    /// ```
    fn build_incremental_pdf(base: &[u8], base_xref_offset: usize) -> Vec<u8> {
        let mut buf = base.to_vec();

        let new_obj_offset = buf.len();
        buf.extend_from_slice(b"4 0 obj\n<</Update true>>\nendobj\n");

        let new_xref_offset = buf.len();

        buf.extend_from_slice(b"xref\n");
        buf.extend_from_slice(b"4 1\n");
        buf.extend_from_slice(format!("{:010} 00000 n \n", new_obj_offset).as_bytes());

        buf.extend_from_slice(format!("trailer\n<</Size 5 /Root 1 0 R /Prev {}>>\n", base_xref_offset).as_bytes());

        buf.extend_from_slice(format!("startxref\n{}\n%%EOF\n", new_xref_offset).as_bytes());

        buf
    }

    /// Parse the xref offset from `startxref\n<N>\n%%EOF` at end of `bytes`.
    fn parse_last_startxref(bytes: &[u8]) -> usize {
        let len = bytes.len();
        let window = &bytes[len.saturating_sub(256)..];
        let sx = find_last_subsequence(window, b"startxref").expect("no startxref");
        let after = sx + b"startxref".len();
        parse_decimal_value(trim_leading_whitespace(&window[after..])).expect("no offset")
    }

    #[test]
    fn should_find_subsequence_at_start() {
        assert_eq!(find_subsequence(b"hello world", b"hello"), Some(0));
    }

    #[test]
    fn should_find_subsequence_in_middle() {
        assert_eq!(find_subsequence(b"hello world", b"world"), Some(6));
    }

    #[test]
    fn should_return_none_when_subsequence_absent() {
        assert_eq!(find_subsequence(b"hello", b"xyz"), None);
    }

    #[test]
    fn should_find_last_subsequence() {
        assert_eq!(find_last_subsequence(b"abcabc", b"abc"), Some(3));
    }

    #[test]
    fn should_parse_decimal_value_with_leading_whitespace() {
        assert_eq!(parse_decimal_value(b"  42 rest"), Some(42));
    }

    #[test]
    fn should_parse_decimal_value_returns_none_for_empty() {
        assert_eq!(parse_decimal_value(b""), None);
    }

    #[test]
    fn should_parse_decimal_value_returns_none_for_non_digit() {
        assert_eq!(parse_decimal_value(b"abc"), None);
    }

    #[test]
    fn should_parse_pdf_date_with_d_prefix_and_full_timestamp() {
        assert_eq!(
            parse_pdf_date_string("D:20240315103045").expect("pure-ASCII date must parse"),
            "2024-03-15T10:30:45Z"
        );
    }

    #[test]
    fn should_parse_pdf_date_with_d_prefix_date_only() {
        assert_eq!(
            parse_pdf_date_string("D:20240315").expect("pure-ASCII date must parse"),
            "2024-03-15T00:00:00Z"
        );
    }

    #[test]
    fn should_parse_pdf_date_without_d_prefix() {
        assert_eq!(
            parse_pdf_date_string("20240315").expect("pure-ASCII date must parse"),
            "2024-03-15T00:00:00Z"
        );
    }

    #[test]
    fn should_return_raw_string_for_malformed_date() {
        assert_eq!(
            parse_pdf_date_string("bad").expect("short input is not an error"),
            "bad"
        );
    }

    /// Regression test for GH#1422: a `/ModDate`/`/CreationDate` string whose
    /// digit portion contains a non-ASCII byte inside the range
    /// `parse_pdf_date_string` needs to slice must return an `Err` carrying
    /// the offending string and the required-ASCII length, never panic.
    ///
    /// The literal below reproduces the exact byte layout from the issue's
    /// panic message ("end byte index 4 is not a char boundary; it is inside
    /// ... (bytes 3..6)"): three ASCII digits, then the Unicode replacement
    /// character `U+FFFD` (a 3-byte UTF-8 sequence, exactly what
    /// `extract_lopdf_string`'s `String::from_utf8_lossy` substitutes for an
    /// invalid byte in the raw `/Info` string) starting at byte offset 3, so
    /// the unfixed code's `&digits[..4]` slice would land inside it.
    #[test]
    fn should_return_error_when_pdf_date_contains_non_ascii_byte_in_required_range() {
        let raw = "D:202\u{FFFD}0315103045";
        let digits = raw.strip_prefix("D:").expect("literal has D: prefix");
        assert_eq!(
            &digits[3..6],
            "\u{FFFD}",
            "test fixture must place the non-ASCII char at bytes [3..6) of `digits`, matching \
             the issue's reported byte range"
        );

        let err = parse_pdf_date_string(raw).expect_err("non-ASCII byte in required range must error");

        assert_eq!(
            err.raw, raw,
            "error must carry the original date string for diagnostics"
        );
        assert_eq!(
            err.required_ascii_bytes, FULL_TIMESTAMP_ASCII_BYTES,
            "digits is long enough for a full timestamp, so the full 14-byte ASCII \
             requirement applies"
        );
        assert!(
            err.to_string().contains("not pure ASCII"),
            "error message must explain the root cause, got: {}",
            err
        );
        assert!(
            err.to_string().contains(raw),
            "error message must include the offending input for diagnostics, got: {}",
            err
        );
    }

    /// A date string too short to be a recognizable date at all (below
    /// [`DATE_ONLY_ASCII_BYTES`]) is not an ASCII-boundary problem — it must
    /// keep returning `Ok` with the original text unchanged, exactly like the
    /// pre-existing malformed-input fallback.
    #[test]
    fn should_return_raw_string_ok_when_non_ascii_date_is_too_short_to_parse() {
        let raw = "ø1";
        assert_eq!(
            parse_pdf_date_string(raw).expect("too-short input is not an ASCII error"),
            raw
        );
    }

    #[test]
    fn should_extract_prev_from_trailer_with_prev_key() {
        let trailer = b"trailer\n<</Size 5 /Root 1 0 R /Prev 100>>\nstartxref\n";
        assert_eq!(extract_prev_from_trailer(trailer), Some(100));
    }

    #[test]
    fn should_return_none_when_no_prev_in_trailer() {
        let trailer = b"trailer\n<</Size 4 /Root 1 0 R>>\nstartxref\n";
        assert_eq!(extract_prev_from_trailer(trailer), None);
    }

    #[test]
    fn should_return_none_when_no_trailer_keyword() {
        let slice = b"not a trailer at all";
        assert_eq!(extract_prev_from_trailer(slice), None);
    }

    #[test]
    fn should_collect_one_startxref_offset_from_single_save_pdf() {
        let pdf = build_minimal_pdf();
        let offsets = collect_startxref_offsets(&pdf);
        assert_eq!(
            offsets.len(),
            1,
            "single-save PDF must yield exactly one startxref offset"
        );
    }

    #[test]
    fn should_collect_two_startxref_offsets_from_incremental_pdf() {
        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf = build_incremental_pdf(&base, base_xref);
        let offsets = collect_startxref_offsets(&pdf);
        assert_eq!(
            offsets.len(),
            2,
            "incremental PDF must yield two startxref offsets; got {:?}",
            offsets
        );
    }

    /// A single-save PDF has no /Prev chain → revisions must be None.
    #[test]
    #[cfg(feature = "pdf")]
    fn should_return_none_for_single_save_pdf() {
        let pdf = build_minimal_pdf();
        let doc = lopdf::Document::load_mem(&pdf).expect("lopdf must parse minimal PDF");
        let result = extract_pdf_xref_revisions(&pdf, &doc);
        assert!(
            result.is_none(),
            "single-save PDF must yield revisions = None, got {:?}",
            result
        );
    }

    /// An incrementally-updated PDF has one prior save → revisions must be Some with length 1.
    #[test]
    #[cfg(feature = "pdf")]
    fn should_return_one_revision_for_incremental_pdf() {
        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf = build_incremental_pdf(&base, base_xref);

        let doc = lopdf::Document::load_mem(&pdf).expect("lopdf must parse incremental PDF");
        let revisions = extract_pdf_xref_revisions(&pdf, &doc).expect("incremental PDF must yield Some(revisions)");

        assert_eq!(revisions.len(), 1, "one prior save must yield one revision");
    }

    /// Each revision's ID must follow the `xref-offset-<N>` pattern.
    #[test]
    #[cfg(feature = "pdf")]
    fn should_produce_revision_ids_in_xref_offset_format() {
        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf = build_incremental_pdf(&base, base_xref);

        let doc = lopdf::Document::load_mem(&pdf).expect("lopdf must parse incremental PDF");
        let revisions = extract_pdf_xref_revisions(&pdf, &doc).expect("incremental PDF must yield Some(revisions)");

        for rev in &revisions {
            assert!(
                rev.revision_id.starts_with("xref-offset-"),
                "revision_id must start with 'xref-offset-', got '{}'",
                rev.revision_id
            );
            let suffix = &rev.revision_id["xref-offset-".len()..];
            suffix
                .parse::<usize>()
                .expect("revision_id suffix must be a valid usize");
        }
    }

    /// Revision kind must be Insertion (placeholder).
    #[test]
    #[cfg(feature = "pdf")]
    fn should_use_insertion_as_revision_kind_placeholder() {
        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf = build_incremental_pdf(&base, base_xref);

        let doc = lopdf::Document::load_mem(&pdf).expect("lopdf must parse incremental PDF");
        let revisions = extract_pdf_xref_revisions(&pdf, &doc).expect("incremental PDF must yield Some(revisions)");

        for rev in &revisions {
            assert!(
                matches!(rev.kind, RevisionKind::Insertion),
                "kind must be Insertion placeholder"
            );
        }
    }

    /// Anchor must be None for all revisions.
    #[test]
    #[cfg(feature = "pdf")]
    fn should_produce_no_anchor_for_pdf_revisions() {
        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf = build_incremental_pdf(&base, base_xref);

        let doc = lopdf::Document::load_mem(&pdf).expect("lopdf must parse incremental PDF");
        let revisions = extract_pdf_xref_revisions(&pdf, &doc).expect("incremental PDF must yield Some(revisions)");

        for rev in &revisions {
            assert!(rev.anchor.is_none(), "anchor must be None for PDF revisions");
        }
    }

    /// Delta must be empty (content extraction deferred).
    #[test]
    #[cfg(feature = "pdf")]
    fn should_produce_empty_delta_for_pdf_revisions() {
        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf = build_incremental_pdf(&base, base_xref);

        let doc = lopdf::Document::load_mem(&pdf).expect("lopdf must parse incremental PDF");
        let revisions = extract_pdf_xref_revisions(&pdf, &doc).expect("incremental PDF must yield Some(revisions)");

        for rev in &revisions {
            assert!(rev.delta.content.is_empty(), "delta.content must be empty (deferred)");
            assert!(
                rev.delta.table_changes.is_empty(),
                "delta.table_changes must be empty (deferred)"
            );
        }
    }

    /// PDF with an /Info dictionary carrying /Author and /ModDate surfaces
    /// those values in every revision.
    #[test]
    #[cfg(feature = "pdf")]
    fn should_surface_author_and_timestamp_from_info_dict() {
        use lopdf::{Dictionary, Document, Object, ObjectId};

        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf_bytes = build_incremental_pdf(&base, base_xref);

        let mut doc = Document::load_mem(&pdf_bytes).expect("lopdf must parse incremental PDF");

        let mut info = Dictionary::new();
        info.set(
            "Author",
            Object::String(b"Test Author".to_vec(), lopdf::StringFormat::Literal),
        );
        info.set(
            "ModDate",
            Object::String(b"D:20240101120000".to_vec(), lopdf::StringFormat::Literal),
        );
        let info_id: ObjectId = (99, 0);
        doc.objects.insert(info_id, Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let revisions =
            extract_pdf_xref_revisions(&pdf_bytes, &doc).expect("incremental PDF must yield Some(revisions)");

        let rev = &revisions[0];
        assert_eq!(
            rev.author.as_deref(),
            Some("Test Author"),
            "author must be extracted from /Info"
        );
        assert_eq!(
            rev.timestamp.as_deref(),
            Some("2024-01-01T12:00:00Z"),
            "timestamp must be extracted and formatted from /Info/ModDate"
        );
    }

    /// End-to-end regression test for GH#1422, reproducing the reported crash
    /// through the actual production code path rather than calling
    /// `parse_pdf_date_string` directly.
    ///
    /// The `/Info/ModDate` bytes below contain a single invalid UTF-8 byte
    /// (`0xFF`, which is never a valid leading byte) at the position that
    /// puts the resulting `U+FFFD` replacement character produced by
    /// `extract_lopdf_string`'s `String::from_utf8_lossy` exactly where the
    /// original issue's panic occurred: spanning bytes `[3..6)` of the
    /// "D:"-stripped digit string, so the unfixed `&digits[..4]` slice lands
    /// inside it and panics with "end byte index 4 is not a char boundary".
    ///
    /// Before the fix this test would panic (and abort the test binary,
    /// exactly mirroring the SIGABRT reported over the Go/cgo FFI boundary,
    /// since a panic here cannot be caught after the fact — it must never
    /// happen). After the fix, `extract_pdf_xref_revisions` must return
    /// normally with the revision's timestamp falling back to the raw,
    /// unparsed (lossily-decoded) string instead of a formatted ISO-8601 date.
    #[test]
    #[cfg(feature = "pdf")]
    fn should_not_panic_when_info_moddate_contains_non_utf8_bytes() {
        use lopdf::{Dictionary, Document, Object, ObjectId};

        let base = build_minimal_pdf();
        let base_xref = parse_last_startxref(&base);
        let pdf_bytes = build_incremental_pdf(&base, base_xref);

        let mut doc = Document::load_mem(&pdf_bytes).expect("lopdf must parse incremental PDF");

        let mut info = Dictionary::new();
        // "D:202" (5 ASCII bytes) + invalid byte 0xFF + "0315103045" (10 ASCII
        // bytes). `String::from_utf8_lossy` replaces the lone 0xFF with one
        // U+FFFD (3 bytes), so in the decoded string the digit portion
        // ("202\u{FFFD}0315103045") has that replacement character spanning
        // bytes [3..6) — squarely inside the year slice `&digits[..4]`. ~keep
        let corrupt_mod_date: &[u8] = b"D:202\xFF0315103045";
        info.set(
            "ModDate",
            Object::String(corrupt_mod_date.to_vec(), lopdf::StringFormat::Literal),
        );
        let info_id: ObjectId = (99, 0);
        doc.objects.insert(info_id, Object::Dictionary(info));
        doc.trailer.set("Info", Object::Reference(info_id));

        let revisions = extract_pdf_xref_revisions(&pdf_bytes, &doc)
            .expect("incremental PDF must still yield Some(revisions) despite the corrupt date");

        let rev = &revisions[0];
        assert_eq!(
            rev.timestamp.as_deref(),
            Some("D:202\u{FFFD}0315103045"),
            "an unparseable (non-ASCII) date must fall back to the raw lossily-decoded string, \
             not panic and not silently disappear"
        );
    }
}
