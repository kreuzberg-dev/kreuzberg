//! Image metadata and data extraction from RTF documents.

use crate::extractors::rtf::encoding::parse_rtf_control_word;

/// Parsed image data from a `\pict` group.
pub struct RtfImage {
    /// Image format string (e.g., "jpeg", "png", "wmf", "bmp").
    pub format: &'static str,
    /// Decoded binary image data.
    pub data: Vec<u8>,
}

/// Extract image metadata and binary data from within a `\pict` group.
///
/// Parses the image type (`\jpegblip`, `\pngblip`, etc.), dimensions, and
/// collects the hex-encoded image data that follows the control words.
/// Returns the parsed image and a metadata string for text representation.
pub(crate) fn extract_pict_image(chars: &mut std::iter::Peekable<std::str::Chars>) -> (String, Option<RtfImage>) {
    let mut metadata = String::new();
    let mut image_type: Option<&str> = None;
    let mut format: &str = "jpeg";
    let mut depth = 0;
    let mut hex_chars = String::new();
    let mut _has_bin = false;

    while let Some(&ch) = chars.peek() {
        match ch {
            '{' => {
                depth += 1;
                chars.next();
            }
            '}' => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                chars.next();
            }
            '\\' => {
                chars.next();
                let (control_word, value) = parse_rtf_control_word(chars);

                match control_word.as_str() {
                    "jpegblip" => {
                        image_type = Some("jpg");
                        format = "jpeg";
                    }
                    "pngblip" => {
                        image_type = Some("png");
                        format = "png";
                    }
                    "wmetafile" => {
                        image_type = Some("wmf");
                        format = "wmf";
                    }
                    "dibitmap" => {
                        image_type = Some("bmp");
                        format = "bmp";
                    }
                    "picwgoal" | "pichgoal" => {}
                    "bin" => {
                        if let Some(count) = value {
                            let count = count.max(0) as usize;
                            // `count` is a number of BYTES of the `\bin` payload, not a number of
                            // `char`s. Consuming `count` `chars.next()` calls OVER-consumes as
                            // soon as the payload holds any multi-byte character, running past
                            // the payload and swallowing the document text that follows it.
                            //
                            // Charging each character its *source* byte width instead requires
                            // one correction: this stream comes from `String::from_utf8_lossy`
                            // (rtf/mod.rs), which rewrites each undecodable byte as U+FFFD — 3
                            // bytes encoded, but 1 byte in the file. A raw `\bin` payload is
                            // arbitrary binary, so it decodes almost entirely to U+FFFD;
                            // charging those 3 bytes each would stop after roughly a third of
                            // the payload and spill the remainder into the parser as RTF text.
                            // Charging them 1 byte is exact for the overwhelmingly common
                            // single-invalid-byte case.
                            //
                            // Residual limitation: `from_utf8_lossy` emits one U+FFFD per
                            // maximal ill-formed subsequence, so a multi-byte truncated sequence
                            // also collapses to a single U+FFFD and is under-counted here. Only
                            // parsing `\bin` against the original bytes can be exact, and this
                            // parser is char-based throughout.
                            let mut consumed_bytes = 0usize;
                            while consumed_bytes < count {
                                match chars.next() {
                                    Some(consumed_char) => {
                                        consumed_bytes += if consumed_char == char::REPLACEMENT_CHARACTER {
                                            1
                                        } else {
                                            consumed_char.len_utf8()
                                        };
                                    }
                                    None => break,
                                }
                            }
                            _has_bin = true;
                        }
                    }
                    _ => {}
                }
            }
            ' ' | '\r' | '\n' => {
                chars.next();
            }
            _ => {
                if ch.is_ascii_hexdigit() {
                    hex_chars.push(ch);
                }
                chars.next();
            }
        }
    }

    if let Some(itype) = image_type {
        metadata.push_str("image.");
        metadata.push_str(itype);
    }

    if metadata.is_empty() {
        metadata.push_str("image.jpg");
    }

    let image = if !hex_chars.is_empty() {
        match hex::decode(&hex_chars) {
            Ok(data) if !data.is_empty() => Some(RtfImage { format, data }),
            _ => None,
        }
    } else {
        None
    };

    (metadata, image)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `\bin` payload that contains a multi-byte UTF-8 character (once the RTF source has
    /// already gone through `String::from_utf8_lossy`) must be skipped by BYTE count, not by
    /// `char` count. `\bin4` declares a 4-byte payload; here it is represented by the 2-byte
    /// char `é` followed by the 1-byte chars `A` and `B` (2 + 1 + 1 = 4 bytes, but only 3
    /// `char`s). Consuming 4 `chars.next()` calls (the old, buggy behavior) eats one char too
    /// many — the group's own closing `}` — which desynchronizes group tracking: the caller
    /// never sees that `}`, the parser never terminates the `\pict` group, and it keeps
    /// scanning into "AFTER" until the input is exhausted, losing it entirely. The fixed,
    /// byte-counting version leaves the closing `}` (and everything after it, "AFTER")
    /// untouched for the caller.
    #[test]
    fn should_skip_bin_payload_by_byte_count_not_char_count() {
        let input = "\\jpegblip\\bin4\u{00e9}AB}AFTER";
        let mut chars = input.chars().peekable();

        let (metadata, image) = extract_pict_image(&mut chars);

        assert_eq!(metadata, "image.jpg");
        assert!(image.is_none());

        let remaining: String = chars.collect();
        assert_eq!(
            remaining, "}AFTER",
            "the group's closing brace and the trailing document text must survive `\\bin` skipping"
        );
    }

    /// The dominant real-world shape: a `\bin` payload of raw binary. `rtf/mod.rs` decodes the
    /// file with `String::from_utf8_lossy`, so each undecodable byte arrives as U+FFFD — encoded
    /// as 3 bytes but only 1 byte in the source file. Charging U+FFFD its *encoded* width
    /// satisfies the declared count after a third of the payload, leaving the remainder to be
    /// read as RTF markup.
    ///
    /// The payload here deliberately contains a `}` byte. Under correct accounting it is opaque
    /// payload and is consumed; under-consume and the parser meets it as a group terminator,
    /// ends the `\pict` group early, and hands the caller the tail of the binary blob as if it
    /// were document text. Without that embedded `}` the surrounding parser happens to absorb
    /// the leftover bytes anyway and the bug hides.
    #[test]
    fn should_charge_replacement_characters_one_source_byte_each() {
        let replacement = char::REPLACEMENT_CHARACTER;
        // Six source bytes: two undecodable, a literal `}`, then three more undecodable.
        let payload = format!("{replacement}{replacement}}}{replacement}{replacement}{replacement}");
        let input = format!("\\jpegblip\\bin6{payload}}}AFTER");
        let mut chars = input.chars().peekable();

        let (metadata, image) = extract_pict_image(&mut chars);

        assert_eq!(metadata, "image.jpg");
        assert!(image.is_none());

        let remaining: String = chars.collect();
        assert_eq!(
            remaining, "}AFTER",
            "the six-byte payload — including its embedded `}}` byte — must be consumed whole, \
             leaving only the group's real closing brace and the document text after it"
        );
    }
}
