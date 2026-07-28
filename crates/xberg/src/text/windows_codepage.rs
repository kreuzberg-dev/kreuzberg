//! Windows codepage number to `encoding_rs` encoding mapping.
//!
//! Shared by the RTF extractor (`\ansicpgNNNN` hex escape decoding) and the
//! MSG/email extractor (PT_STRING8 property decoding).

use encoding_rs::Encoding;

/// Map a Windows codepage number to an `encoding_rs` encoding.
///
/// Backed by the `codepage` crate. The Mac CJK codepages approximate to their
/// Windows counterparts first because the crate has no mapping for them.
/// Unknown values fall back to Windows-1252, the most common legacy ANSI
/// codepage and the RTF default. `to_encoding_no_replacement` is required:
/// plain `to_encoding` maps 50225, 50227, and 52936 to the replacement
/// encoding, which decodes every byte to U+FFFD.
#[inline]
pub(crate) fn encoding_for_windows_codepage(codepage: u32) -> &'static Encoding {
    let codepage = match codepage {
        10001 => 932,
        10002 => 950,
        10003 => 949,
        10008 => 936,
        other => other,
    };
    u16::try_from(codepage)
        .ok()
        .and_then(codepage::to_encoding_no_replacement)
        .unwrap_or(encoding_rs::WINDOWS_1252)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_codepage_mapping() {
        assert_eq!(encoding_for_windows_codepage(1251).name(), "windows-1251");
        assert_eq!(encoding_for_windows_codepage(932).name(), "Shift_JIS");
        assert_eq!(encoding_for_windows_codepage(874).name(), "windows-874");
        assert_eq!(encoding_for_windows_codepage(866).name(), "IBM866");
        assert_eq!(encoding_for_windows_codepage(20866).name(), "KOI8-R");
        // Mac CJK codepages approximate to their Windows counterparts.
        assert_eq!(encoding_for_windows_codepage(10001).name(), "Shift_JIS");
        assert_eq!(encoding_for_windows_codepage(10002).name(), "Big5");
        // Unknown and out-of-range values fall back to Windows-1252.
        assert_eq!(encoding_for_windows_codepage(1717).name(), "windows-1252");
        assert_eq!(encoding_for_windows_codepage(100_000).name(), "windows-1252");
        // ISO-2022 variants map to the replacement encoding in `to_encoding`;
        // `to_encoding_no_replacement` must fall back to Windows-1252 instead.
        assert_eq!(encoding_for_windows_codepage(50225).name(), "windows-1252");
    }
}
