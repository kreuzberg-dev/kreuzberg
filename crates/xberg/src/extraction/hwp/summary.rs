//! Parses the `\x05HwpSummaryInformation` stream.
//!
//! This stream uses the exact same binary layout as the OLE `SummaryInformation`
//! property-set stream Microsoft Office documents carry (MS-OLEPS) — HWP just gives
//! the stream its own name. Verified against a real fixture
//! (`test_documents/hwp/converted_output.hwp`): its title/author/dates decode
//! correctly under this layout, including the `VT_LPWSTR`/`VT_FILETIME` value types
//! implemented here (#105).

use super::model::SummaryInfo;

/// `VT_LPSTR`: a codepage-encoded (non-Unicode) length-prefixed string.
const VT_LPSTR: u32 = 30;
/// `VT_LPWSTR`: a UTF-16LE length-prefixed string.
const VT_LPWSTR: u32 = 31;
/// `VT_FILETIME`: a 64-bit Windows `FILETIME` (100ns intervals since 1601-01-01 UTC).
const VT_FILETIME: u32 = 64;

const PIDSI_TITLE: u32 = 2;
const PIDSI_SUBJECT: u32 = 3;
const PIDSI_AUTHOR: u32 = 4;
const PIDSI_KEYWORDS: u32 = 5;
const PIDSI_COMMENTS: u32 = 6;
const PIDSI_LASTAUTHOR: u32 = 8;
const PIDSI_CREATE_DTM: u32 = 12;
const PIDSI_LASTSAVE_DTM: u32 = 13;

/// Number of 100-nanosecond intervals between the `FILETIME` epoch (1601-01-01) and
/// the Unix epoch (1970-01-01).
const FILETIME_TO_UNIX_EPOCH_100NS: i64 = 116_444_736_000_000_000;

fn read_u32(data: &[u8], at: usize) -> Option<u32> {
    data.get(at..at + 4)
        .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// Parses the property-set stream, returning `None` only when the stream is too
/// malformed to identify a single property set — an individual unreadable property
/// is simply skipped, matching "preserve partial results on failure" (never let one
/// bad property blank out an otherwise-good title/author).
pub(crate) fn parse_summary_information(data: &[u8]) -> Option<SummaryInfo> {
    // Fixed header: byte order (2) + version (2) + OS (4) + CLSID (16) + count (4) = 28.
    let num_property_sets = read_u32(data, 24)? as usize;
    if num_property_sets == 0 {
        return None;
    }
    // Only the first property set is used — HWP's SummaryInformation, like Office's,
    // carries exactly one (a second, "DocumentSummaryInformation"-style set would
    // live in a different stream entirely).
    let first_set_offset = read_u32(data, 28 + 16)? as usize;

    let mut info = SummaryInfo::default();
    // `size` (the property set's declared byte length) is intentionally not
    // validated against the stream length here: an inflated/wrong value should not
    // stop the loop below from reading whatever properties actually fit — every
    // subsequent read is itself bounds-checked via `data.get(..)`.
    let _size = read_u32(data, first_set_offset)?;
    let num_properties = read_u32(data, first_set_offset + 4)? as usize;

    for i in 0..num_properties {
        let entry_offset = first_set_offset + 8 + i * 8;
        let Some(property_id) = read_u32(data, entry_offset) else {
            break;
        };
        let Some(relative_offset) = read_u32(data, entry_offset + 4) else {
            break;
        };
        let absolute_offset = first_set_offset + relative_offset as usize;

        match property_id {
            PIDSI_TITLE => info.title = read_string_property(data, absolute_offset),
            PIDSI_SUBJECT => info.subject = read_string_property(data, absolute_offset),
            PIDSI_AUTHOR => info.author = read_string_property(data, absolute_offset),
            PIDSI_KEYWORDS => info.keywords = read_string_property(data, absolute_offset),
            PIDSI_COMMENTS => info.comments = read_string_property(data, absolute_offset),
            PIDSI_LASTAUTHOR => info.last_author = read_string_property(data, absolute_offset),
            PIDSI_CREATE_DTM => info.created = read_filetime_property(data, absolute_offset),
            PIDSI_LASTSAVE_DTM => info.modified = read_filetime_property(data, absolute_offset),
            _ => {}
        }
    }

    Some(info)
}

/// Reads a `VT_LPSTR`/`VT_LPWSTR` value at `offset`, returning `None` for an empty
/// string (an empty title/author is equivalent to "not present" for our purposes) or
/// on any bounds/type mismatch.
fn read_string_property(data: &[u8], offset: usize) -> Option<String> {
    let value_type = read_u32(data, offset)?;
    match value_type {
        VT_LPWSTR => {
            let char_count = read_u32(data, offset + 4)? as usize;
            let bytes = data.get(offset + 8..offset + 8 + char_count.checked_mul(2)?)?;
            let units: Vec<u16> = bytes
                .chunks_exact(2)
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect();
            let s = String::from_utf16_lossy(&units);
            let trimmed = s.trim_end_matches('\u{0}');
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        VT_LPSTR => {
            let byte_count = read_u32(data, offset + 4)? as usize;
            let bytes = data.get(offset + 8..offset + 8 + byte_count)?;
            // No codepage table is available for the non-Unicode variant, so this is
            // decoded as UTF-8 with lossy replacement rather than left unsupported —
            // correct for the common case (ASCII/UTF-8 producers) and at least
            // readable, rather than silently empty, for others.
            let s = String::from_utf8_lossy(bytes);
            let trimmed = s.trim_end_matches('\u{0}');
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        _ => None,
    }
}

/// Reads a `VT_FILETIME` value at `offset`, converting it to an RFC 3339 UTC
/// timestamp. Returns `None` for a zero `FILETIME` (HWP uses this to mean "unset")
/// or a value predating the Unix epoch.
fn read_filetime_property(data: &[u8], offset: usize) -> Option<String> {
    let value_type = read_u32(data, offset)?;
    if value_type != VT_FILETIME {
        return None;
    }
    let low = read_u32(data, offset + 4)?;
    let high = read_u32(data, offset + 8)?;
    let filetime = ((high as u64) << 32) | (low as u64);
    filetime_to_rfc3339(filetime)
}

fn filetime_to_rfc3339(filetime: u64) -> Option<String> {
    if filetime == 0 {
        return None;
    }
    let unix_100ns = (filetime as i64).checked_sub(FILETIME_TO_UNIX_EPOCH_100NS)?;
    if unix_100ns < 0 {
        return None;
    }
    let unix_seconds = unix_100ns / 10_000_000;
    let days = unix_seconds.div_euclid(86_400);
    let seconds_of_day = unix_seconds.rem_euclid(86_400);

    let (year, month, day) = civil_from_days(days);
    let hour = seconds_of_day / 3600;
    let minute = (seconds_of_day % 3600) / 60;
    let second = seconds_of_day % 60;

    Some(format!(
        "{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z"
    ))
}

/// Converts a day count since the Unix epoch (1970-01-01) to a `(year, month, day)`
/// civil (Gregorian) date. Public-domain algorithm by Howard Hinnant
/// ("chrono-Compatible Low-Level Date Algorithms", <https://howardhinnant.github.io/date_algorithms.html>),
/// used here instead of pulling in a date/time crate (`hwp` does not depend on
/// `chrono`).
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32; // [1, 31]
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32; // [1, 12]
    let year = if month <= 2 { y + 1 } else { y };
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn utf16le(s: &str) -> Vec<u8> {
        s.encode_utf16().flat_map(|u| u.to_le_bytes()).collect()
    }

    /// Builds a minimal property-set stream with one property set holding the given
    /// `(property_id, value_bytes)` entries, where `value_bytes` already includes the
    /// leading `VT_*` type u32.
    fn build_stream(properties: &[(u32, Vec<u8>)]) -> Vec<u8> {
        let mut data = vec![0u8; 28]; // byte order + version + OS + CLSID + count
        data[24..28].copy_from_slice(&1u32.to_le_bytes());
        data.extend_from_slice(&[0u8; 16]); // FMTID (unused by the parser)
        let first_set_offset = data.len() as u32 + 4;
        data.extend_from_slice(&first_set_offset.to_le_bytes());

        let header_len = 8 + properties.len() * 8;
        let mut values = Vec::new();
        let mut entries = Vec::new();
        for (id, value) in properties {
            entries.push((*id, (header_len + values.len()) as u32));
            values.extend_from_slice(value);
        }

        let property_set_len = header_len + values.len();
        data.extend_from_slice(&(property_set_len as u32).to_le_bytes());
        data.extend_from_slice(&(properties.len() as u32).to_le_bytes());
        for (id, offset) in entries {
            data.extend_from_slice(&id.to_le_bytes());
            data.extend_from_slice(&offset.to_le_bytes());
        }
        data.extend_from_slice(&values);
        data
    }

    fn lpwstr_value(s: &str) -> Vec<u8> {
        let units = utf16le(s);
        let mut v = VT_LPWSTR.to_le_bytes().to_vec();
        v.extend_from_slice(&((s.encode_utf16().count()) as u32).to_le_bytes());
        v.extend_from_slice(&units);
        v
    }

    fn filetime_value(filetime: u64) -> Vec<u8> {
        let mut v = VT_FILETIME.to_le_bytes().to_vec();
        v.extend_from_slice(&(filetime as u32).to_le_bytes());
        v.extend_from_slice(&((filetime >> 32) as u32).to_le_bytes());
        v
    }

    #[test]
    fn should_decode_title_and_author_from_lpwstr() {
        let stream = build_stream(&[
            (PIDSI_TITLE, lpwstr_value("강사위촉계약서(예시)")),
            (PIDSI_AUTHOR, lpwstr_value("jinsol")),
        ]);
        let info = parse_summary_information(&stream).expect("must parse");
        assert_eq!(info.title.as_deref(), Some("강사위촉계약서(예시)"));
        assert_eq!(info.author.as_deref(), Some("jinsol"));
    }

    #[test]
    fn should_return_none_for_empty_string_property() {
        let stream = build_stream(&[(PIDSI_SUBJECT, lpwstr_value(""))]);
        let info = parse_summary_information(&stream).expect("must parse");
        assert_eq!(info.subject, None);
    }

    #[test]
    fn should_convert_filetime_to_rfc3339_using_verified_real_values() {
        // Ground truth cross-checked against Python: datetime(1601,1,1) +
        // timedelta(microseconds=filetime // 10).
        assert_eq!(
            filetime_to_rfc3339(133_642_875_584_630_000),
            Some("2024-07-01T06:05:58Z".to_string())
        );
        assert_eq!(
            filetime_to_rfc3339(133_643_667_597_550_000),
            Some("2024-07-02T04:05:59Z".to_string())
        );
    }

    #[test]
    fn should_map_zero_filetime_to_none() {
        assert_eq!(filetime_to_rfc3339(0), None);
    }

    #[test]
    fn should_decode_created_and_modified_dates() {
        let stream = build_stream(&[
            (PIDSI_CREATE_DTM, filetime_value(133_642_875_584_630_000)),
            (PIDSI_LASTSAVE_DTM, filetime_value(133_643_667_597_550_000)),
        ]);
        let info = parse_summary_information(&stream).expect("must parse");
        assert_eq!(info.created.as_deref(), Some("2024-07-01T06:05:58Z"));
        assert_eq!(info.modified.as_deref(), Some("2024-07-02T04:05:59Z"));
    }

    #[test]
    fn should_not_panic_on_truncated_stream() {
        let short = vec![0u8; 10];
        assert_eq!(parse_summary_information(&short), None);
    }
}
