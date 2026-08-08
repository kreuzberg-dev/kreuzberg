//! WebVTT (Web Video Text Tracks) extractor.
//!
//! The plain-text extractor used to pass `.vtt` files through unchanged, so cue
//! identifiers and `00:00:01.000 --> 00:00:04.000` timestamp lines landed in the body
//! text as noise and no cue structure survived at all (#228).
//!
//! This extractor parses the WebVTT grammar:
//!
//! - The `WEBVTT` signature line, whose optional trailing text becomes the document title.
//! - `NOTE`, `STYLE` and `REGION` blocks, which are metadata and never body text.
//! - Cues: an optional identifier line, a timing line, and the cue payload.
//!
//! **Only the cue payload becomes content.** Timings never enter the body text — they are
//! attached to the cue element's `attributes` (`start`, `end`, and `cue_id` / `speaker`
//! when present), because the timing of a subtitle belongs to the cue it labels rather
//! than to the document as a whole, and `TextMetadata` has no timing field to put them in.
//! The overall track duration is recorded in `Metadata.additional`.
//!
//! Malformed input degrades: a missing `WEBVTT` signature, an unparsable timing line or a
//! cue with no payload each produce a [`ProcessingWarning`] and the remaining cues are
//! still extracted.

use crate::Result;
use crate::core::config::ExtractionConfig;
use crate::extraction::transform::normalize_line_endings;
use crate::extractors::security::SecurityBudget;
use crate::plugins::{InternalDocumentExtractor, Plugin};
use crate::types::ProcessingWarning;
use crate::types::internal::InternalDocument;
use crate::types::internal_builder::InternalDocumentBuilder;
use crate::types::metadata::Metadata;
use ahash::AHashMap;
use async_trait::async_trait;
use std::borrow::Cow;

/// The mandatory WebVTT file signature.
const SIGNATURE: &str = "WEBVTT";

/// Separator between a cue's start and end timestamps.
const TIMING_ARROW: &str = "-->";

/// Milliseconds per second / minute / hour, used when normalising timestamps.
const MILLIS_PER_SECOND: u64 = 1_000;
const MILLIS_PER_MINUTE: u64 = 60 * MILLIS_PER_SECOND;
const MILLIS_PER_HOUR: u64 = 60 * MILLIS_PER_MINUTE;

/// WebVTT subtitle track extractor.
///
/// Emits one paragraph element per cue, carrying the cue's timings and identifier as
/// element attributes rather than as body text.
#[cfg_attr(alef, alef(skip))]
pub struct WebVttExtractor;

impl WebVttExtractor {
    /// Create a new WebVTT extractor.
    pub(crate) fn new() -> Self {
        Self
    }
}

impl Default for WebVttExtractor {
    fn default() -> Self {
        Self::new()
    }
}

/// A single parsed cue.
struct Cue {
    identifier: Option<String>,
    /// `None` for a block that carried text but no timing line. Such a block is not a cue in
    /// the WebVTT sense, but its text is still document content, so it is kept untimed rather
    /// than discarded. Zero is deliberately not used as a stand-in: it would fabricate a
    /// `00:00:00.000` start/end and skew the reported duration.
    start_millis: Option<u64>,
    end_millis: Option<u64>,
    speaker: Option<String>,
    text: String,
}

/// Result of parsing a whole WebVTT file.
struct ParsedTrack {
    title: Option<String>,
    cues: Vec<Cue>,
    warnings: Vec<ProcessingWarning>,
}

/// Parse a WebVTT source into cues plus warnings.
fn parse_track(source: &str) -> ParsedTrack {
    let mut warnings = Vec::new();
    let mut title = None;
    let lines: Vec<&str> = source.lines().collect();

    let first = lines.first().copied().unwrap_or_default().trim_start();
    let body_start = if let Some(after_signature) = first.strip_prefix(SIGNATURE) {
        let trailing = after_signature.trim().trim_start_matches('-').trim();
        if !trailing.is_empty() {
            title = Some(trailing.to_string());
        }
        1
    } else {
        warnings.push(warning("missing WEBVTT signature line; parsing as WebVTT anyway"));
        0
    };

    let mut cues = Vec::new();
    for block in split_blocks(&lines[body_start..]) {
        match classify_block(&block) {
            BlockKind::Metadata => {}
            BlockKind::Cue => match parse_cue(&block) {
                Ok(cue) => cues.push(cue),
                Err(message) => warnings.push(warning(message)),
            },
            // A block with no timing line is not a cue, but its text is still the document's
            // content — dropping it loses everything in a file that is mislabelled `.vtt` or
            // truncated before its timings. Keep the text as an untimed cue and warn.
            BlockKind::Unknown => {
                warnings.push(warning("block without a timing line kept as untimed text"));
                let text = block.join("\n").trim().to_string();
                if !text.is_empty() {
                    cues.push(Cue {
                        identifier: None,
                        start_millis: None,
                        end_millis: None,
                        speaker: None,
                        text,
                    });
                }
            }
        }
    }

    ParsedTrack { title, cues, warnings }
}

/// Split a line sequence into blank-line-separated blocks.
fn split_blocks<'a>(lines: &[&'a str]) -> Vec<Vec<&'a str>> {
    let mut blocks = Vec::new();
    let mut current: Vec<&'a str> = Vec::new();
    for &line in lines {
        if line.trim().is_empty() {
            if !current.is_empty() {
                blocks.push(std::mem::take(&mut current));
            }
        } else {
            current.push(line);
        }
    }
    if !current.is_empty() {
        blocks.push(current);
    }
    blocks
}

/// What a WebVTT block represents.
enum BlockKind {
    /// `NOTE`, `STYLE` or `REGION` — never body content.
    Metadata,
    /// A block containing a timing line.
    Cue,
    /// Anything else.
    Unknown,
}

fn classify_block(block: &[&str]) -> BlockKind {
    let Some(first) = block.first().map(|line| line.trim()) else {
        return BlockKind::Unknown;
    };
    if first == "NOTE"
        || first.starts_with("NOTE ")
        || first == "STYLE"
        || first == "REGION"
        || first.starts_with("REGION ")
    {
        return BlockKind::Metadata;
    }
    if block.iter().any(|line| line.contains(TIMING_ARROW)) {
        BlockKind::Cue
    } else {
        BlockKind::Unknown
    }
}

/// Parse a cue block: optional identifier, timing line, payload.
fn parse_cue(block: &[&str]) -> std::result::Result<Cue, &'static str> {
    let timing_position = block
        .iter()
        .position(|line| line.contains(TIMING_ARROW))
        .ok_or("cue block without a timing line skipped")?;

    let identifier = if timing_position == 0 {
        None
    } else {
        Some(block[..timing_position].join(" ").trim().to_string())
    };

    let (start_millis, end_millis) =
        parse_timing_line(block[timing_position]).ok_or("cue with an unparsable timing line skipped")?;

    let payload = block[timing_position + 1..].join("\n");
    let (speaker, text) = strip_cue_tags(&payload);
    if text.trim().is_empty() {
        return Err("cue with an empty payload skipped");
    }

    Ok(Cue {
        identifier,
        start_millis: Some(start_millis),
        end_millis: Some(end_millis),
        speaker,
        text,
    })
}

/// Parse `00:00:01.000 --> 00:00:04.000 line:0 position:20%` into start/end milliseconds.
fn parse_timing_line(line: &str) -> Option<(u64, u64)> {
    let (start, rest) = line.split_once(TIMING_ARROW)?;
    let end = rest.split_whitespace().next()?;
    Some((parse_timestamp(start.trim())?, parse_timestamp(end)?))
}

/// Parse `hh:mm:ss.mmm` or `mm:ss.mmm` into milliseconds.
fn parse_timestamp(value: &str) -> Option<u64> {
    let (clock, fraction) = value.split_once('.')?;
    if fraction.len() != 3 || !fraction.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let millis: u64 = fraction.parse().ok()?;
    let parts: Vec<&str> = clock.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [hours, minutes, seconds] => (hours.parse::<u64>().ok()?, minutes.parse::<u64>().ok()?, *seconds),
        [minutes, seconds] => (0, minutes.parse::<u64>().ok()?, *seconds),
        _ => return None,
    };
    let seconds: u64 = seconds.parse().ok()?;
    Some(hours * MILLIS_PER_HOUR + minutes * MILLIS_PER_MINUTE + seconds * MILLIS_PER_SECOND + millis)
}

/// Format milliseconds back to the canonical `hh:mm:ss.mmm` form.
fn format_timestamp(millis: u64) -> String {
    let hours = millis / MILLIS_PER_HOUR;
    let minutes = (millis % MILLIS_PER_HOUR) / MILLIS_PER_MINUTE;
    let seconds = (millis % MILLIS_PER_MINUTE) / MILLIS_PER_SECOND;
    let remainder = millis % MILLIS_PER_SECOND;
    format!("{hours:02}:{minutes:02}:{seconds:02}.{remainder:03}")
}

/// Strip WebVTT cue markup, returning the voice-span speaker (if any) and clean text.
///
/// Removes `<v Speaker>`, `<i>`, `<c.classname>`, `<b>`, `<ruby>` and inline
/// `<00:00:01.000>` karaoke timestamps, then decodes the WebVTT character escapes.
fn strip_cue_tags(payload: &str) -> (Option<String>, String) {
    let mut speaker = None;
    let mut out = String::with_capacity(payload.len());
    let mut rest = payload;

    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            // Unbalanced '<' — keep it verbatim rather than dropping the remainder.
            out.push_str(&rest[open..]);
            return (speaker, decode_escapes(&out));
        };
        let tag = &after[..close];
        if speaker.is_none()
            && let Some(name) = parse_voice_tag(tag)
        {
            speaker = Some(name);
        }
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    (speaker, decode_escapes(&out))
}

/// Extract the speaker name from a voice span tag body such as `v Roger` or `v.loud Roger`.
///
/// Returns `None` for every other tag (`i`, `b`, `c.classname`, karaoke timestamps, and
/// all closing tags).
fn parse_voice_tag(tag: &str) -> Option<String> {
    let mut rest = tag.strip_prefix('v')?;
    if !rest.is_empty() && !rest.starts_with('.') && !rest.starts_with(char::is_whitespace) {
        return None;
    }
    while let Some(after_dot) = rest.strip_prefix('.') {
        let end = after_dot
            .find(|c: char| c == '.' || c.is_whitespace())
            .unwrap_or(after_dot.len());
        rest = &after_dot[end..];
    }
    let name = rest.trim();
    if name.is_empty() { None } else { Some(name.to_string()) }
}

/// Decode the character escapes WebVTT defines for cue payloads.
fn decode_escapes(text: &str) -> String {
    text.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", "\u{a0}")
        .replace("&lrm;", "\u{200e}")
        .replace("&rlm;", "\u{200f}")
        .replace("&amp;", "&")
}

fn warning(message: &'static str) -> ProcessingWarning {
    ProcessingWarning {
        source: Cow::Borrowed("vtt"),
        message: Cow::Borrowed(message),
    }
}

/// Attributes attached to a cue element.
fn cue_attributes(cue: &Cue) -> AHashMap<String, String> {
    let mut attributes = AHashMap::new();
    // An untimed block gets no start/end attributes at all rather than a fabricated
    // `00:00:00.000`, so a consumer can tell "not timed" from "starts at zero".
    if let Some(start_millis) = cue.start_millis {
        attributes.insert("start".to_string(), format_timestamp(start_millis));
    }
    if let Some(end_millis) = cue.end_millis {
        attributes.insert("end".to_string(), format_timestamp(end_millis));
    }
    if let Some(identifier) = &cue.identifier {
        attributes.insert("cue_id".to_string(), identifier.clone());
    }
    if let Some(speaker) = &cue.speaker {
        attributes.insert("speaker".to_string(), speaker.clone());
    }
    attributes
}

impl Plugin for WebVttExtractor {
    fn name(&self) -> &str {
        "webvtt-extractor"
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
        "Extracts cue text and timings from WebVTT subtitle tracks"
    }

    fn author(&self) -> &str {
        "Xberg Team"
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl InternalDocumentExtractor for WebVttExtractor {
    async fn extract_content(
        &self,
        content: &[u8],
        mime_type: &str,
        config: &ExtractionConfig,
    ) -> Result<InternalDocument> {
        let mut budget = SecurityBudget::from_config(config);
        budget.account_text(content.len())?;

        // See the identical rationale on `extractors::text::PlainTextExtractor::extract_content`
        // (#171, #395): `replaced_characters` is recorded by the decoder itself, at the point
        // data is actually lost, so it stays accurate under `quality`'s mojibake cleanup where
        // scanning the decoded text for U+FFFD afterwards would not.
        let outcome = crate::utils::decode_with_provenance(content, None);
        let decoded_lossily = outcome.replaced_characters;
        let source = normalize_line_endings(crate::utils::strip_bom(&outcome.text));

        let parsed = parse_track(&source);

        let mut builder = InternalDocumentBuilder::new("vtt");
        let mut body = String::new();
        for cue in &parsed.cues {
            let text = match &cue.speaker {
                Some(speaker) => format!("{speaker}: {}", cue.text.trim()),
                None => cue.text.trim().to_string(),
            };
            let index = builder.push_paragraph(&text, Vec::new(), None, None);
            builder.set_attributes(index, cue_attributes(cue));
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(&text);
        }

        let mut metadata = Metadata {
            title: parsed.title,
            ..Default::default()
        };
        // `cue_count` counts real (timed) cues only — untimed blocks are recovered text, not
        // cues, and counting them would change what this metadata field has always meant.
        let timed_cue_count = parsed.cues.iter().filter(|cue| cue.start_millis.is_some()).count();
        metadata
            .additional
            .insert(Cow::Borrowed("cue_count"), serde_json::json!(timed_cue_count));
        if let Some(last) = parsed.cues.iter().filter_map(|cue| cue.end_millis).max() {
            metadata
                .additional
                .insert(Cow::Borrowed("duration"), serde_json::json!(format_timestamp(last)));
        }
        metadata.format = Some(crate::types::FormatMetadata::Text(crate::types::TextMetadata {
            line_count: body.lines().count() as u32,
            word_count: body.split_whitespace().count() as u32,
            character_count: body.chars().count() as u32,
            // WebVTT has no headings, hyperlinks or code blocks to report.
            headers: None,
            links: None,
            code_blocks: None,
        }));
        builder.set_metadata(metadata);
        for warning in parsed.warnings {
            builder.add_warning(warning);
        }

        let mut document = builder.build();
        document.mime_type = mime_type.to_string();
        if decoded_lossily {
            crate::core::diagnostics::push_lossy_decode_warning(
                &mut document.processing_warnings,
                "vtt",
                "WebVTT source",
            );
        }
        Ok(document)
    }

    fn supported_mime_types(&self) -> &[&str] {
        &["text/vtt"]
    }

    fn priority(&self) -> i32 {
        50
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_parse_hour_and_minute_timestamps() {
        assert_eq!(parse_timestamp("00:00:01.000"), Some(1_000));
        assert_eq!(parse_timestamp("01:02:03.004"), Some(3_723_004));
        assert_eq!(parse_timestamp("02:03.004"), Some(123_004));
        assert_eq!(parse_timestamp("00:00:01.0"), None);
        assert_eq!(parse_timestamp("nonsense"), None);
    }

    #[test]
    fn should_round_trip_timestamp_formatting() {
        assert_eq!(format_timestamp(3_723_004), "01:02:03.004");
        assert_eq!(format_timestamp(0), "00:00:00.000");
    }

    #[test]
    fn should_strip_voice_and_style_tags() {
        let (speaker, text) = strip_cue_tags("<v Roger Bingham>We are <i>here</i> &amp; now");
        assert_eq!(speaker.as_deref(), Some("Roger Bingham"));
        assert_eq!(text, "We are here & now");
    }

    #[test]
    fn should_keep_unbalanced_angle_bracket_verbatim() {
        let (speaker, text) = strip_cue_tags("5 < 6 and no close");
        assert_eq!(speaker, None);
        assert_eq!(text, "5 < 6 and no close");
    }

    #[test]
    fn should_warn_but_still_parse_when_signature_is_missing() {
        let parsed = parse_track("00:00:01.000 --> 00:00:02.000\nHello\n");
        assert_eq!(parsed.cues.len(), 1);
        assert_eq!(parsed.cues[0].text, "Hello");
        assert_eq!(parsed.warnings.len(), 1);
    }

    /// A file with no timing line anywhere — a plain-text file that happens to be named
    /// `.vtt`, or a track truncated before its timings — used to extract to nothing at all:
    /// every block was classified `Unknown` and discarded, leaving only a warning. The text
    /// is now recovered as untimed cues.
    #[test]
    fn should_keep_text_of_blocks_that_have_no_timing_line() {
        let parsed = parse_track("WEBVTT\n\nFirst stray line.\n\nSecond stray line.\n");

        assert_eq!(parsed.cues.len(), 2);
        assert_eq!(parsed.cues[0].text, "First stray line.");
        assert_eq!(parsed.cues[1].text, "Second stray line.");
        assert_eq!(
            parsed.cues[0].start_millis, None,
            "an untimed block must not fabricate a start"
        );
        assert_eq!(
            parsed.cues[0].end_millis, None,
            "an untimed block must not fabricate an end"
        );
        assert_eq!(parsed.warnings.len(), 2, "each dropped-then-recovered block warns");
    }

    /// The recovered text must not be mistaken for a real cue: `cue_attributes` emits no
    /// `start`/`end` keys at all rather than a fabricated `00:00:00.000`, which a consumer
    /// would be unable to tell from a cue that genuinely starts at zero.
    #[test]
    fn should_omit_timing_attributes_for_an_untimed_block() {
        let parsed = parse_track("WEBVTT\n\nStray line.\n");
        let attributes = cue_attributes(&parsed.cues[0]);

        assert!(!attributes.contains_key("start"), "got {attributes:?}");
        assert!(!attributes.contains_key("end"), "got {attributes:?}");
    }

    /// A timed cue is unaffected by the untimed-block handling.
    #[test]
    fn should_still_emit_timing_attributes_for_a_timed_cue() {
        let parsed = parse_track("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello\n");
        let attributes = cue_attributes(&parsed.cues[0]);

        assert_eq!(attributes.get("start").map(String::as_str), Some("00:00:01.000"));
        assert_eq!(attributes.get("end").map(String::as_str), Some("00:00:02.500"));
    }

    /// Warnings emitted for a lossy decode specifically, identified by message content
    /// since `"vtt"` is also the source of unrelated grammar warnings (e.g. a missing
    /// signature line).
    fn decode_warnings(doc: &InternalDocument) -> Vec<String> {
        doc.processing_warnings
            .iter()
            .filter(|w| w.message.contains("not valid UTF-8"))
            .map(|w| w.message.to_string())
            .collect()
    }

    /// #395: invalid UTF-8 bytes must be reported via `processing_warnings`, not just
    /// silently replaced -- `decode_with_provenance` captures the loss at decode time
    /// so it survives regardless of the `quality` feature's downstream mojibake cleanup.
    ///
    /// Deliberately not run under `quality`: there chardetng resolves these bytes to a
    /// single-byte encoding that maps all of 0x00-0xFF, so nothing is *replaced* -- see
    /// the identical note on `extractors::text::should_warn_when_text_source_is_not_valid_utf8`.
    #[cfg(not(feature = "quality"))]
    #[tokio::test]
    async fn should_warn_when_vtt_source_is_not_valid_utf8() {
        let extractor = WebVttExtractor::new();
        let config = ExtractionConfig::default();
        let content: &[u8] = &[b'A', 0xFF, 0xFE, b'B'];

        let result = extractor
            .extract_content(content, "text/vtt", &config)
            .await
            .expect("extraction of invalid UTF-8 must still succeed");

        let warnings = decode_warnings(&result);
        assert_eq!(
            warnings.len(),
            1,
            "expected exactly one decode warning, got {warnings:?}"
        );
        assert!(
            warnings[0].contains("replacement character"),
            "warning must describe the lossy decode, got {warnings:?}"
        );
    }

    /// A valid UTF-8 track must not produce a lossy-decode warning.
    #[tokio::test]
    async fn valid_utf8_vtt_source_produces_zero_decode_warnings() {
        let extractor = WebVttExtractor::new();
        let config = ExtractionConfig::default();
        let content = b"WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nHello\n";

        let result = extractor
            .extract_content(content, "text/vtt", &config)
            .await
            .expect("extraction should succeed");

        assert!(
            decode_warnings(&result).is_empty(),
            "valid UTF-8 must not warn about a lossy decode, got {:?}",
            decode_warnings(&result)
        );
    }
}
