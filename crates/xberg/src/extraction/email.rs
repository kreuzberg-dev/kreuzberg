//! Email extraction functions.
//!
//! Parses .eml (RFC822) and .msg (Outlook) email files using `mail-parser`.
//! Extracts message content, headers, and attachment information.
//!
//! # Features
//!
//! - **EML support**: RFC822 format parsing
//! - **HTML to text**: Strips HTML tags from HTML email bodies
//! - **Metadata extraction**: Sender, recipients, subject, message ID
//! - **Attachments**: Lists attachment metadata and recursively extracts supported content
//!
//! # Example
//!
//! ```ignore
//! use xberg::extraction::email::parse_eml_content;
//!
//! # fn example() -> xberg::Result<()> {
//! let eml_bytes = std::fs::read("message.eml")?;
//! let result = parse_eml_content(&eml_bytes)?;
//!
//! println!("From: {:?}", result.from_email);
//! println!("Subject: {:?}", result.subject);
//! # Ok(())
//! # }
//! ```
use crate::error::{Result, XbergError};
use crate::extractors::security::{SecurityBudget, SecurityLimits};
use crate::text::utf8_validation;
use crate::text::windows_codepage::encoding_for_windows_codepage;
use crate::types::{EmailAttachment, EmailExtractionResult, ProcessingWarning};
use bytes::Bytes;
use mail_parser::MimeHeaders;
use regex::Regex;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

static HTML_TAG_RE: OnceLock<Regex> = OnceLock::new();
static SCRIPT_RE: OnceLock<Regex> = OnceLock::new();
static STYLE_RE: OnceLock<Regex> = OnceLock::new();
static WHITESPACE_RE: OnceLock<Regex> = OnceLock::new();

const PID_TAG_HTML: u16 = 0x1013;
const PID_TAG_INTERNET_CODEPAGE: u16 = 0x3FDE;
const PID_TAG_MESSAGE_CODEPAGE: u16 = 0x3FFD;

/// PR_ATTACH_METHOD (PidTagAttachMethod): how an attachment's data is stored.
const PID_TAG_ATTACH_METHOD: u16 = 0x3705;
/// PR_ATTACH_DATA_OBJECT (PidTagAttachDataObject): for `afEmbeddedMessage`
/// attachments, this property is PT_OBJECT (type `000D`) and names a nested
/// CFB *storage* (not a stream) holding the embedded message's own properties.
const PID_TAG_ATTACH_DATA_OBJECT: u16 = 0x3701;
/// `afEmbeddedMessage`: the attachment is itself a Message object stored as a
/// nested CFB storage rather than binary stream data.
const ATTACH_METHOD_EMBEDDED_MSG: u32 = 5;

pub(crate) struct ParsedEmailContent {
    pub(crate) result: EmailExtractionResult,
    pub(crate) nested_messages: Vec<NestedMessagePayload>,
    /// Embedded `message/rfc822`-equivalent MSG attachments (`attach_method ==
    /// afEmbeddedMessage`), fully parsed. Flat list across all nesting depths.
    pub(crate) nested_embedded_messages: Vec<EmailExtractionResult>,
    /// Non-fatal warnings raised while parsing (e.g. a `.msg`-in-`.msg` chain
    /// that hit the configured nesting-depth cap and was left unextracted).
    pub(crate) warnings: Vec<ProcessingWarning>,
}

pub(crate) struct NestedMessagePayload {
    part_id: usize,
    pub(crate) data: Option<Bytes>,
    pub(crate) size: usize,
}

fn html_tag_regex() -> &'static Regex {
    HTML_TAG_RE.get_or_init(|| Regex::new(r"<[^>]+>").unwrap())
}

fn script_regex() -> &'static Regex {
    SCRIPT_RE.get_or_init(|| Regex::new(r"(?is)<script[^>]*>.*?</script>").unwrap())
}

fn style_regex() -> &'static Regex {
    STYLE_RE.get_or_init(|| Regex::new(r"(?is)<style[^>]*>.*?</style>").unwrap())
}

fn whitespace_regex() -> &'static Regex {
    WHITESPACE_RE.get_or_init(|| Regex::new(r"\s+").unwrap())
}

/// Detect UTF-16 encoding (with or without BOM) and transcode to UTF-8 if needed.
///
/// `mail_parser` expects ASCII/UTF-8 input. If the EML file is encoded as
/// UTF-16, we transcode it to UTF-8 first.
///
/// Detection strategy:
/// 1. Check for BOM (`FF FE` = LE, `FE FF` = BE)
/// 2. If no BOM, use heuristic: EML files start with ASCII headers, so
///    alternating zero bytes indicate UTF-16 encoding.
fn maybe_transcode_utf16(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 4 {
        return None;
    }

    let (is_le, skip) = if data[0] == 0xFF && data[1] == 0xFE {
        (true, 2)
    } else if data[0] == 0xFE && data[1] == 0xFF {
        (false, 2)
    } else if data.len() >= 16 {
        let is_le_heuristic = data[1] == 0x00 && data[3] == 0x00 && data[5] == 0x00 && data[7] == 0x00;
        let is_be_heuristic = data[0] == 0x00 && data[2] == 0x00 && data[4] == 0x00 && data[6] == 0x00;

        if is_le_heuristic || is_be_heuristic {
            let mut detector = chardetng::EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
            detector.feed(data, true);
            let guess = detector.guess(None, chardetng::Utf8Detection::Allow);
            if guess.name() == "UTF-8" || guess.name() == "windows-1252" {
                (is_le_heuristic, 0)
            } else {
                return None;
            }
        } else {
            return None;
        }
    } else {
        return None;
    };

    let payload = &data[skip..];
    let even_len = payload.len() & !1;
    let u16_iter = (0..even_len).step_by(2).map(|i| {
        if is_le {
            u16::from_le_bytes([payload[i], payload[i + 1]])
        } else {
            u16::from_be_bytes([payload[i], payload[i + 1]])
        }
    });

    match String::from_utf16(&u16_iter.collect::<Vec<u16>>()) {
        Ok(s) => Some(s.into_bytes()),
        Err(_) => None,
    }
}

/// Parse .eml file content (RFC822 format)
pub(crate) fn parse_eml_content(data: &[u8]) -> Result<EmailExtractionResult> {
    Ok(parse_eml_content_internal(data, false, None)?.result)
}

fn parse_eml_content_internal(
    data: &[u8],
    include_nested_messages: bool,
    max_nested_message_bytes: Option<u64>,
) -> Result<ParsedEmailContent> {
    let data = if let Some(transcoded) = maybe_transcode_utf16(data) {
        std::borrow::Cow::Owned(transcoded)
    } else {
        std::borrow::Cow::Borrowed(data)
    };

    let message = mail_parser::MessageParser::default()
        .parse(&data)
        .ok_or_else(|| XbergError::parsing("Failed to parse EML file: invalid email format".to_string()))?;

    let subject = message.subject().map(|s| s.to_string());

    let sender = message.from().and_then(|from| from.first());
    let from_email = sender.and_then(|address| address.address().map(str::to_string));
    let from_name = sender
        .and_then(|address| address.name().map(str::trim))
        .filter(|name| !name.is_empty())
        .map(str::to_string);

    let to_emails: Vec<String> = message
        .to()
        .map(|to| {
            to.iter()
                .filter_map(|addr| addr.address().map(|email| email.to_string()))
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let cc_emails: Vec<String> = message
        .cc()
        .map(|cc| {
            cc.iter()
                .filter_map(|addr| addr.address().map(|email| email.to_string()))
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let bcc_emails: Vec<String> = message
        .bcc()
        .map(|bcc| {
            bcc.iter()
                .filter_map(|addr| addr.address().map(|email| email.to_string()))
                .collect()
        })
        .unwrap_or_else(Vec::new);

    let date = extract_raw_date_header(&data).or_else(|| {
        message.date().and_then(|d| {
            let rfc3339 = d.to_rfc3339();
            if rfc3339.starts_with("2000-00") || rfc3339.starts_with("0000-") {
                None
            } else {
                Some(rfc3339)
            }
        })
    });

    let message_id = message.message_id().map(|id| id.to_string());

    let reply_to: Vec<String> = message
        .reply_to()
        .map(|addrs| {
            addrs
                .iter()
                .filter_map(|addr| addr.address().map(|email| email.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let in_reply_to: Vec<String> = message
        .in_reply_to()
        .as_text_list()
        .map(|list| list.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let references: Vec<String> = message
        .references()
        .as_text_list()
        .map(|list| list.iter().map(|s| s.to_string()).collect())
        .unwrap_or_default();

    let raw_headers = extract_raw_headers(&data);

    let should_treat_as_html = message
        .content_type()
        .and_then(|ct| ct.subtype())
        .map(|subtype| subtype.eq_ignore_ascii_case("html"))
        .unwrap_or(false);

    let has_genuine_text_part = has_genuine_text_body(&message);
    let plain_text = if has_genuine_text_part || should_treat_as_html {
        let mut all_text = Vec::new();
        let mut i = 0;
        while let Some(text) = message.body_text(i) {
            all_text.push(text.to_string());
            i += 1;
        }
        collect_nested_message_text(&message, &mut all_text);
        if all_text.is_empty() {
            None
        } else {
            Some(all_text.join("\n\n"))
        }
    } else {
        None
    };

    let has_genuine_html_part = has_genuine_html_body(&message);
    let html_content = if has_genuine_html_part || should_treat_as_html {
        let mut all_html = Vec::new();
        let mut i = 0;
        while let Some(html) = message.body_html(i) {
            all_html.push(html.to_string());
            i += 1;
        }
        collect_nested_message_html(&message, &mut all_html);
        if all_html.is_empty() {
            None
        } else {
            Some(all_html.join("\n\n"))
        }
    } else {
        None
    };

    let html_content = if should_treat_as_html && html_content.is_none() && plain_text.is_some() {
        plain_text.clone()
    } else {
        html_content
    };

    let content = if let Some(html) = &html_content {
        clean_html_content(html)
    } else if let Some(plain) = &plain_text {
        plain.clone()
    } else {
        String::new()
    };

    let nested_messages = if include_nested_messages {
        collect_nested_message_payloads(&message, max_nested_message_bytes)
    } else {
        Vec::new()
    };

    let mut attachments = Vec::with_capacity(message.attachments().count().min(20));
    for &part_id in &message.attachments {
        let Some(attachment) = message.parts.get(part_id as usize) else {
            continue;
        };
        let filename = attachment.attachment_name().map(|s| s.to_string());

        let mime_type = if matches!(&attachment.body, mail_parser::PartType::Message(_)) {
            "message/rfc822".to_string()
        } else {
            attachment
                .content_type()
                .map(|ct| {
                    let content_type_str = format!("{}/{}", ct.ctype(), ct.subtype().unwrap_or("octet-stream"));
                    parse_content_type(&content_type_str)
                })
                .unwrap_or_else(|| "application/octet-stream".to_string())
        };

        let (data, size) = match &attachment.body {
            mail_parser::PartType::Message(nested) => {
                let raw_message = nested.raw_message();
                let data = if include_nested_messages {
                    nested_messages
                        .iter()
                        .find(|payload| payload.part_id == part_id as usize)
                        .and_then(|payload| payload.data.clone())
                } else {
                    Some(Bytes::copy_from_slice(raw_message))
                };
                (data, raw_message.len())
            }
            _ => {
                let contents = attachment.contents();
                (Some(Bytes::copy_from_slice(contents)), contents.len())
            }
        };

        let is_image = is_image_mime_type(&mime_type);

        attachments.push(EmailAttachment {
            name: filename.clone(),
            filename,
            mime_type: Some(mime_type),
            size: Some(size),
            is_image,
            data,
        });
    }

    let mut metadata = build_metadata(
        &subject,
        &from_email,
        &to_emails,
        &cc_emails,
        &bcc_emails,
        &date,
        &message_id,
        &attachments,
    );

    if let Some(from_name) = from_name {
        metadata.insert("from_name".to_string(), from_name);
    }

    if !reply_to.is_empty() {
        metadata.insert("reply_to".to_string(), reply_to.join(", "));
    }
    if !in_reply_to.is_empty() {
        metadata.insert("in_reply_to".to_string(), in_reply_to.join(", "));
    }
    if !references.is_empty() {
        metadata.insert("references".to_string(), references.join(", "));
    }

    for (key, value) in &raw_headers {
        metadata.insert(key.clone(), value.clone());
    }

    if !attachments.is_empty() {
        let attachment_details: Vec<String> = attachments
            .iter()
            .map(|att| {
                let name = att.filename.as_deref().or(att.name.as_deref()).unwrap_or("unnamed");
                let mime = att.mime_type.as_deref().unwrap_or("application/octet-stream");
                let size = att.size.unwrap_or(0);
                format!("{}|{}|{}", name, mime, size)
            })
            .collect();
        metadata.insert("attachment_details".to_string(), attachment_details.join("; "));
    }

    Ok(ParsedEmailContent {
        result: EmailExtractionResult {
            subject,
            from_email,
            to_emails,
            cc_emails,
            bcc_emails,
            date,
            message_id,
            plain_text,
            html_content,
            content,
            attachments,
            metadata,
        },
        nested_messages,
        nested_embedded_messages: Vec::new(),
        warnings: Vec::new(),
    })
}

fn collect_nested_message_payloads(
    message: &mail_parser::Message<'_>,
    max_nested_message_bytes: Option<u64>,
) -> Vec<NestedMessagePayload> {
    use mail_parser::PartType;

    message
        .parts
        .iter()
        .enumerate()
        .filter_map(|(part_id, part)| match &part.body {
            PartType::Message(nested) if !nested.raw_message().is_empty() => {
                let raw_message = nested.raw_message();
                let data = max_nested_message_bytes
                    .is_none_or(|cap| raw_message.len() as u64 <= cap)
                    .then(|| Bytes::copy_from_slice(raw_message));
                Some(NestedMessagePayload {
                    part_id,
                    data,
                    size: raw_message.len(),
                })
            }
            _ => None,
        })
        .collect()
}

/// Check whether a message has at least one genuine `text/plain` body part.
///
/// `mail-parser`'s `body_text()` auto-converts HTML to plain text using a naive
/// tag-stripper that doesn't remove `<script>` or `<style>` content. This helper
/// inspects the actual `PartType` of each `text_body` entry to determine if a
/// real `text/plain` part exists. For HTML-only messages (all text_body entries
/// are `PartType::Html`), callers should use `clean_html_content()` instead.
fn has_genuine_text_body(message: &mail_parser::Message<'_>) -> bool {
    use mail_parser::PartType;
    for &part_id in &message.text_body {
        if let Some(part) = message.parts.get(part_id as usize)
            && matches!(&part.body, PartType::Text(_))
        {
            return true;
        }
    }
    false
}

/// Check whether a message has at least one genuine `text/html` body part.
///
/// `mail-parser`'s `body_html()` auto-converts text/plain bodies into HTML when
/// no real HTML part exists. This helper inspects each `html_body` entry's
/// `PartType` so callers can avoid treating plain-text emails as HTML.
fn has_genuine_html_body(message: &mail_parser::Message<'_>) -> bool {
    use mail_parser::PartType;
    for &part_id in &message.html_body {
        if let Some(part) = message.parts.get(part_id as usize)
            && matches!(&part.body, PartType::Html(_))
        {
            return true;
        }
    }
    false
}

/// Recursively collect plain text from nested `message/rfc822` sub-messages.
///
/// In `multipart/digest` emails, each part is itself an RFC822 message stored as
/// `PartType::Message`. The top-level `body_text()` won't return these; we must
/// recurse into the nested `Message` to extract their text bodies.
fn collect_nested_message_text(message: &mail_parser::Message<'_>, out: &mut Vec<String>) {
    use mail_parser::PartType;
    for part in &message.parts {
        if let PartType::Message(sub_msg) = &part.body {
            let mut i = 0;
            while let Some(text) = sub_msg.body_text(i) {
                out.push(text.to_string());
                i += 1;
            }
            collect_nested_message_text(sub_msg, out);
        }
    }
}

/// Recursively collect HTML from nested `message/rfc822` sub-messages.
fn collect_nested_message_html(message: &mail_parser::Message<'_>, out: &mut Vec<String>) {
    use mail_parser::PartType;
    for part in &message.parts {
        if let PartType::Message(sub_msg) = &part.body {
            let mut i = 0;
            while let Some(html) = sub_msg.body_html(i) {
                out.push(html.to_string());
                i += 1;
            }
            collect_nested_message_html(sub_msg, out);
        }
    }
}

/// Parse .msg file content (Outlook format).
///
/// Reads MSG files directly via the CFB (OLE Compound Document) format,
/// extracting text properties and attachment metadata without the overhead
/// of hex-encoding attachment binary data (which caused hangs on large files
/// with the previous `msg_parser` dependency).
///
/// Some MSG files have FAT headers declaring more sectors than the file
/// actually contains.  The strict `cfb` crate rejects these.  When that
/// happens we pad the data with zero bytes so the sector count matches
/// the FAT and retry – the real streams are still within the original
/// data range and parse correctly.
///
pub(crate) fn parse_msg_content(data: &[u8], fallback_codepage: Option<u32>) -> Result<EmailExtractionResult> {
    parse_msg_content_with_nested(data, fallback_codepage, &SecurityLimits::default()).map(|(result, _, _)| result)
}

/// Parse .msg file content (Outlook format), also returning any embedded
/// (`afEmbeddedMessage`) MSG attachments found while parsing, flattened
/// across nesting depth, plus any warnings raised (e.g. a `.msg`-in-`.msg`
/// chain that hit `security_limits`' nesting-depth cap).
pub(crate) fn parse_msg_content_with_nested(
    data: &[u8],
    fallback_codepage: Option<u32>,
    security_limits: &SecurityLimits,
) -> Result<(
    EmailExtractionResult,
    Vec<EmailExtractionResult>,
    Vec<ProcessingWarning>,
)> {
    use std::borrow::Cow;
    use std::io::Cursor;

    let padded: Cow<'_, [u8]>;
    let data_ref: &[u8] = match cfb::CompoundFile::open(Cursor::new(data)) {
        Ok(_) => data,
        Err(_first_err) => {
            padded = pad_cfb_to_fat_size(data);
            if std::ptr::eq(padded.as_ref(), data) {
                return Err(XbergError::parsing(format!("Failed to parse MSG file: {_first_err}")));
            }
            &padded
        }
    };

    let mut comp = cfb::CompoundFile::open(Cursor::new(data_ref))
        .map_err(|e| XbergError::parsing(format!("Failed to parse MSG file: {e}")))?;

    // Reuse the same nesting-depth counter `SecurityLimits` provides for
    // every other format (XML/HTML/JSON) to guard the `.msg`-in-`.msg`
    // recursion hazard, rather than a bespoke counter local to this parser.
    let mut budget = SecurityBudget::from_limits(security_limits);
    let mut warnings = Vec::new();
    let (result, nested_embedded_messages) =
        extract_msg_from_cfb_at(&mut comp, "", fallback_codepage, &mut budget, &mut warnings)?;

    Ok((result, nested_embedded_messages, warnings))
}

/// Pad an OLE/CFB file so the sector count matches the FAT header.
///
/// Some MSG writers emit FAT tables that reference sectors beyond the
/// physical end of the file.  The `cfb` crate rightfully rejects these
/// as "Malformed FAT".  By zero-padding to the declared size we let cfb
/// open the file; streams within the original range parse normally while
/// the padded area is treated as free sectors.
fn pad_cfb_to_fat_size(data: &[u8]) -> std::borrow::Cow<'_, [u8]> {
    use std::borrow::Cow;

    if data.len() < 76 || data[..4] != [0xD0, 0xCF, 0x11, 0xE0] {
        return Cow::Borrowed(data);
    }

    let sector_power = u16::from_le_bytes([data[30], data[31]]) as u32;
    if !(9..=16).contains(&sector_power) {
        return Cow::Borrowed(data);
    }
    let sector_size = 1u64 << sector_power;

    let fat_sectors = u32::from_le_bytes([data[44], data[45], data[46], data[47]]) as u64;
    let fat_entries = fat_sectors * (sector_size / 4);
    let needed = (1 + fat_entries) * sector_size;

    if needed > 256 * 1024 * 1024 || (data.len() as u64) >= needed {
        return Cow::Borrowed(data);
    }

    let mut padded = data.to_vec();
    padded.resize(needed as usize, 0);
    Cow::Owned(padded)
}

/// Maximum capacity pre-allocated for a decompressed RTF body (16 MiB).
///
/// `raw_size` in the OXRTFCP header is attacker-controlled; using it directly
/// as a `Vec::with_capacity` argument lets a crafted `.msg` file trigger a 4 GiB
/// allocation.  This constant caps the hint — the `Vec` grows freely beyond it if
/// the actual decompressed content is larger, so correctness is unaffected.
const MAX_RTF_DECOMPRESSED_CAPACITY: usize = 16 * 1024 * 1024;

/// Pre-filled dictionary used by the MS-OXRTFCP compressed RTF format.
///
/// See [MS-OXRTFCP] section 2.1.3.1.
const COMPRESSED_RTF_PREBUF: &[u8] = b"{\\rtf1\\ansi\\mac\\deff0\\deftab720{\\fonttbl;}\
{\\f0\\fnil \\froman \\fswiss \\fmodern \\fscript \\fdecor MS Sans SerifSymbolArialTimes New Roman\
Courier{\\colortbl\\red0\\green0\\blue0\r\n\\par \\pard\\plain\\f0\\fs20\\b\\i\\ul\\ob\\strike\
\\scaps\\outline\\shadow\\imprint\\emboss\\lang1024\\sbasedon1033\\fcharset0 {\\*\\cs10 \\additive \
Default Paragraph Font}";

/// Decompress a PR_RTF_COMPRESSED stream per the MS-OXRTFCP specification.
///
/// Returns `None` when the data is too short, has a bad magic number, or
/// the decompression runs past declared bounds.
pub(crate) fn decompress_rtf_compressed(data: &[u8]) -> Option<Vec<u8>> {
    if data.len() < 16 {
        return None;
    }

    let comp_size = u32::from_le_bytes(data[0..4].try_into().ok()?) as usize;
    let raw_size = u32::from_le_bytes(data[4..8].try_into().ok()?);
    let magic = u32::from_le_bytes(data[8..12].try_into().ok()?);

    if magic == 0x414c_454d {
        return Some(data.get(16..16 + comp_size.saturating_sub(12))?.to_vec());
    }
    if magic != 0x75465a4c {
        return None;
    }

    let mut dict = [0u8; 4096];
    let prebuf_len = COMPRESSED_RTF_PREBUF.len();
    dict[..prebuf_len].copy_from_slice(COMPRESSED_RTF_PREBUF);
    let mut dict_write = prebuf_len;

    let input = data.get(16..)?;
    let end = (comp_size.saturating_sub(12)).min(input.len());

    let mut output = Vec::with_capacity((raw_size as usize).min(MAX_RTF_DECOMPRESSED_CAPACITY));
    let mut pos = 0usize;

    while pos < end {
        let control = *input.get(pos)?;
        pos += 1;

        for bit in (0..8).rev() {
            if pos >= end {
                return Some(output);
            }

            if control & (1 << bit) != 0 {
                let hi = *input.get(pos)? as u16;
                let lo = *input.get(pos + 1)? as u16;
                pos += 2;

                let offset = ((hi << 4) | (lo >> 4)) as usize;
                let length = (lo & 0x0F) as usize + 2;

                for i in 0..length {
                    let byte = dict[(offset + i) & 0xFFF];
                    output.push(byte);
                    dict[dict_write & 0xFFF] = byte;
                    dict_write += 1;
                }
            } else {
                let byte = *input.get(pos)?;
                pos += 1;
                output.push(byte);
                dict[dict_write & 0xFFF] = byte;
                dict_write += 1;
            }
        }
    }

    Some(output)
}

/// Strip RTF control sequences and extract plain text.
///
/// Handles `\par` → newline, `\uN` unicode escapes, `{` `}` grouping,
/// and discards other `\command` sequences.  This is intentionally
/// simplified — it covers the typical content produced by Outlook.
pub(crate) fn strip_rtf_to_plain_text(rtf: &[u8]) -> String {
    let text = String::from_utf8_lossy(rtf);
    let bytes = text.as_bytes();
    let len = bytes.len();
    let mut output = String::with_capacity(len / 2);
    let mut i = 0;
    let mut skip_depth: Option<usize> = None;
    let mut depth: usize = 0;

    while i < len {
        match bytes[i] {
            b'{' => {
                depth += 1;
                i += 1;
                if i + 1 < len && bytes[i] == b'\\' && bytes.get(i + 1) == Some(&b'*') && skip_depth.is_none() {
                    skip_depth = Some(depth);
                }
                if skip_depth.is_none() {
                    let rest = &text[i..];
                    if rest.starts_with("\\fonttbl")
                        || rest.starts_with("\\colortbl")
                        || rest.starts_with("\\stylesheet")
                        || rest.starts_with("\\info")
                    {
                        skip_depth = Some(depth);
                    }
                }
            }
            b'}' => {
                if let Some(sd) = skip_depth
                    && depth <= sd
                {
                    skip_depth = None;
                }
                depth = depth.saturating_sub(1);
                i += 1;
            }
            b'\\' if skip_depth.is_none() => {
                i += 1;
                if i >= len {
                    break;
                }
                match bytes[i] {
                    b'\\' => {
                        output.push('\\');
                        i += 1;
                    }
                    b'{' => {
                        output.push('{');
                        i += 1;
                    }
                    b'}' => {
                        output.push('}');
                        i += 1;
                    }
                    b'\'' => {
                        i += 1;
                        if i + 2 <= len {
                            if let Ok(hex_str) = std::str::from_utf8(&bytes[i..i + 2])
                                && let Ok(byte_val) = u8::from_str_radix(hex_str, 16)
                            {
                                let byte_arr = [byte_val];
                                let (decoded, _, _) = encoding_rs::WINDOWS_1252.decode(&byte_arr);
                                output.push_str(&decoded);
                            }
                            i += 2;
                        }
                    }
                    b'u' if i + 1 < len && (bytes[i + 1].is_ascii_digit() || bytes[i + 1] == b'-') => {
                        i += 1;
                        let start = i;
                        if i < len && bytes[i] == b'-' {
                            i += 1;
                        }
                        while i < len && bytes[i].is_ascii_digit() {
                            i += 1;
                        }
                        if let Ok(num_str) = std::str::from_utf8(&bytes[start..i])
                            && let Ok(code) = num_str.parse::<i32>()
                        {
                            let cp = if code < 0 { (code + 65536) as u32 } else { code as u32 };
                            if let Some(ch) = char::from_u32(cp) {
                                output.push(ch);
                            }
                        }
                        if i < len && bytes[i] == b' ' {
                            i += 1;
                        }
                        if i < len && bytes[i] != b'\\' && bytes[i] != b'{' && bytes[i] != b'}' {
                            i += 1;
                        }
                    }
                    _ => {
                        let word_start = i;
                        while i < len && bytes[i].is_ascii_alphabetic() {
                            i += 1;
                        }
                        let word = &text[word_start..i];

                        if i < len && (bytes[i] == b'-' || bytes[i].is_ascii_digit()) {
                            if bytes[i] == b'-' {
                                i += 1;
                            }
                            while i < len && bytes[i].is_ascii_digit() {
                                i += 1;
                            }
                        }

                        if i < len && bytes[i] == b' ' {
                            i += 1;
                        }

                        match word {
                            "par" | "line" => output.push('\n'),
                            "tab" => output.push('\t'),
                            _ => {}
                        }
                    }
                }
            }
            b'\r' | b'\n' if skip_depth.is_none() => {
                i += 1;
            }
            _ if skip_depth.is_some() => {
                i += 1;
            }
            _ => {
                output.push(bytes[i] as char);
                i += 1;
            }
        }
    }

    let mut result = String::with_capacity(output.len());
    let mut prev_newline_count = 0u32;
    for ch in output.chars() {
        if ch == '\n' {
            prev_newline_count += 1;
            if prev_newline_count <= 2 {
                result.push('\n');
            }
        } else {
            prev_newline_count = 0;
            result.push(ch);
        }
    }

    result.trim().to_string()
}

/// Enumerate storage paths that are *direct* children of `message_root` whose
/// name starts with `name_prefix`.
///
/// A message may contain an embedded message attachment (`afEmbeddedMessage`),
/// whose own attachments/recipients live deeper in the tree under
/// `{message_root}/__attach_.../__substg1.0_3701000D/...`. A naive whole-tree
/// `walk()` filtered only by name prefix would incorrectly attribute those
/// deeper entries to the outer message, so entries are filtered to those whose
/// immediate parent path equals `message_root` exactly.
fn direct_child_storage_paths<F: std::io::Read + std::io::Seek>(
    comp: &cfb::CompoundFile<F>,
    message_root: &str,
    name_prefix: &str,
) -> Vec<String> {
    let entries: Vec<cfb::Entry> = if message_root.is_empty() {
        comp.walk().collect()
    } else {
        match comp.walk_storage(message_root) {
            Ok(iter) => iter.collect(),
            Err(_) => return Vec::new(),
        }
    };

    entries
        .into_iter()
        .filter(|e| e.is_storage() && e.name().starts_with(name_prefix))
        .map(|e| e.path().to_string_lossy().into_owned())
        .filter(|path| path.rsplit_once('/').map(|(parent, _)| parent) == Some(message_root))
        .collect()
}

/// Internal: extract email fields from an already-opened CFB compound file.
///
/// `message_root` is `""` for the top-level MSG message, or the CFB storage
/// path of an embedded message (`afEmbeddedMessage` attachment) when called
/// recursively. `budget` guards against pathological/malicious
/// message-in-message nesting via its `SecurityLimits`-derived
/// [`crate::extractors::security::DepthValidator`] — the same nesting-depth
/// counter every other format uses, rather than a bespoke one for MSG.
/// `warnings` collects a [`ProcessingWarning`] whenever the depth cap stops a
/// nested embedded message from being extracted further.
///
/// Returns the parsed message plus a flat list of any embedded MSG messages
/// discovered at or below this level.
fn extract_msg_from_cfb_at<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    message_root: &str,
    fallback_codepage: Option<u32>,
    budget: &mut SecurityBudget,
    warnings: &mut Vec<ProcessingWarning>,
) -> Result<(EmailExtractionResult, Vec<EmailExtractionResult>)> {
    let message_codepage = read_msg_int_prop(comp, message_root, PID_TAG_MESSAGE_CODEPAGE);
    let internet_codepage = read_msg_int_prop(comp, message_root, PID_TAG_INTERNET_CODEPAGE);
    let codepage = message_codepage.or(internet_codepage).or(fallback_codepage);
    let html_codepage = internet_codepage.or(message_codepage).or(fallback_codepage);

    let subject = read_msg_string_prop(comp, message_root, 0x0037, codepage);
    let sender_name = read_msg_string_prop(comp, message_root, 0x0C1A, codepage);
    let sender_email = read_msg_string_prop(comp, message_root, 0x0C1F, codepage)
        .or_else(|| read_msg_string_prop(comp, message_root, 0x0065, codepage))
        .filter(|s| !s.is_empty());
    let from_email = sender_email;
    let body = read_msg_string_prop(comp, message_root, 0x1000, codepage);
    let html_body = read_msg_html_prop(comp, message_root, html_codepage);
    let message_id = read_msg_string_prop(comp, message_root, 0x1035, codepage).filter(|s| !s.is_empty());

    let date = read_msg_filetime_prop(comp, message_root, 0x0039)
        .or_else(|| read_msg_filetime_prop(comp, message_root, 0x0E06))
        .or_else(|| {
            let headers = read_msg_string_prop(comp, message_root, 0x007D, codepage);
            headers.as_ref().and_then(|h| {
                h.lines()
                    .find(|line| line.starts_with("Date:"))
                    .map(|line| line.trim_start_matches("Date:").trim().to_string())
            })
        });

    let (to_emails, cc_emails, bcc_emails) = read_msg_recipients(comp, message_root, codepage);

    let rtf_body = read_msg_stream(comp, &format!("{message_root}/__substg1.0_10090102"))
        .and_then(|data| decompress_rtf_compressed(&data))
        .map(|rtf| strip_rtf_to_plain_text(&rtf))
        .filter(|s| !s.is_empty());

    let plain_text = body.filter(|s| !s.is_empty());
    let html_content = html_body.filter(|s| !s.is_empty());

    let content = if let Some(ref plain) = plain_text {
        plain.clone()
    } else if let Some(ref html) = html_content {
        clean_html_content(html)
    } else if let Some(ref rtf) = rtf_body {
        rtf.clone()
    } else {
        String::new()
    };

    let attach_paths = direct_child_storage_paths(comp, message_root, "__attach_");

    let mut attachments = Vec::with_capacity(attach_paths.len());
    let mut nested_embedded_messages = Vec::new();
    for path in &attach_paths {
        let long_name = read_msg_string_prop(comp, path, 0x3707, codepage);
        let short_name = read_msg_string_prop(comp, path, 0x3704, codepage);
        let display_name = read_msg_string_prop(comp, path, 0x3001, codepage);
        let extension = read_msg_string_prop(comp, path, 0x3703, codepage);
        let mime_tag = read_msg_string_prop(comp, path, 0x370E, codepage);

        let filename = long_name
            .or(short_name)
            .or_else(|| display_name.clone())
            .or_else(|| extension.map(|ext| format!("attachment{ext}")));

        let attach_method = read_msg_attach_or_recip_int_prop(comp, path, PID_TAG_ATTACH_METHOD);
        let embedded_storage_path = format!("{path}/__substg1.0_{PID_TAG_ATTACH_DATA_OBJECT:04X}000D");
        let is_embedded_message =
            attach_method == Some(ATTACH_METHOD_EMBEDDED_MSG) && comp.is_storage(&embedded_storage_path);

        if is_embedded_message {
            if budget.enter().is_ok() {
                let recursed =
                    extract_msg_from_cfb_at(comp, &embedded_storage_path, fallback_codepage, budget, warnings);
                budget.leave();
                let (nested_result, deeper_nested) = recursed?;
                let size = None;
                let filename = filename
                    .or_else(|| nested_result.subject.clone())
                    .or_else(|| Some("embedded_message".to_string()));

                attachments.push(EmailAttachment {
                    name: filename.clone(),
                    filename,
                    mime_type: Some("message/rfc822".to_string()),
                    size,
                    is_image: false,
                    data: None,
                });

                nested_embedded_messages.push(nested_result);
                nested_embedded_messages.extend(deeper_nested);
            } else {
                // `budget.enter()` still incremented the counter even though it
                // rejected this level; balance it immediately so sibling
                // attachments at the same depth aren't spuriously capped too.
                budget.leave();
                warnings.push(ProcessingWarning {
                    source: Cow::Borrowed("msg_embedded_message_extraction"),
                    message: Cow::Owned(format!(
                        "Stopped extracting embedded message at '{path}': nesting depth cap reached; \
                         the embedded message and anything nested inside it were left unextracted"
                    )),
                });
                attachments.push(EmailAttachment {
                    name: filename.clone(),
                    filename,
                    mime_type: Some("message/rfc822".to_string()),
                    size: None,
                    is_image: false,
                    data: None,
                });
            }
            continue;
        }

        let bin_path = format!("{path}/__substg1.0_37010102");
        let binary_data = read_msg_stream(comp, &bin_path);
        let size = binary_data.as_ref().map(Vec::len);
        let att_data = binary_data.map(Bytes::from);

        let mime_type = mime_tag
            .filter(|s| !s.is_empty())
            .or_else(|| Some("application/octet-stream".to_string()));
        let is_image = mime_type.as_ref().map(|m| is_image_mime_type(m)).unwrap_or(false);

        attachments.push(EmailAttachment {
            name: filename.clone(),
            filename,
            mime_type,
            size,
            is_image,
            data: att_data,
        });
    }

    let mut metadata = HashMap::new();
    if let Some(ref subj) = subject {
        metadata.insert("subject".to_string(), subj.to_string());
    }
    if let Some(ref from) = from_email {
        metadata.insert("email_from".to_string(), from.to_string());
    }
    if let Some(ref name) = sender_name
        && !name.is_empty()
    {
        metadata.insert("from_name".to_string(), name.to_string());
    }
    if !to_emails.is_empty() {
        metadata.insert("email_to".to_string(), to_emails.join(", "));
    }
    if !cc_emails.is_empty() {
        metadata.insert("email_cc".to_string(), cc_emails.join(", "));
    }
    if !bcc_emails.is_empty() {
        metadata.insert("email_bcc".to_string(), bcc_emails.join(", "));
    }
    if let Some(ref dt) = date {
        metadata.insert("date".to_string(), dt.to_string());
    }
    if let Some(ref msg_id) = message_id {
        metadata.insert("message_id".to_string(), msg_id.to_string());
    }

    Ok((
        EmailExtractionResult {
            subject,
            from_email,
            to_emails,
            cc_emails,
            bcc_emails,
            date,
            message_id,
            plain_text,
            html_content,
            content,
            attachments,
            metadata,
        },
        nested_embedded_messages,
    ))
}

/// Read a raw CFB stream by path; returns `None` for missing or empty streams.
fn read_msg_stream<F: std::io::Read + std::io::Seek>(comp: &mut cfb::CompoundFile<F>, path: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut stream = comp.open_stream(path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    if buf.is_empty() { None } else { Some(buf) }
}

/// Read a PT_LONG (0x0003) integer property from a message-level
/// `__properties_version1.0` stream.
///
/// `message_root` is `""` for the top-level MSG message (32-byte property
/// stream header per MS-OXMSG 2.4.3), or the CFB storage path of an embedded
/// message (24-byte header per MS-OXMSG 2.4.3, since the top-level's leading
/// 8 reserved bytes are omitted for embedded messages). Use
/// [`read_msg_attach_or_recip_int_prop`] instead for attachment- or
/// recipient-level properties, whose header is always 8 bytes.
fn read_msg_int_prop<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    message_root: &str,
    prop_id: u16,
) -> Option<u32> {
    use std::io::Read;

    let props_path = format!("{message_root}/__properties_version1.0");
    let mut stream = comp.open_stream(&props_path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;

    const TOP_LEVEL_HEADER_SIZE: usize = 32;
    const EMBEDDED_MESSAGE_HEADER_SIZE: usize = 24;
    let header_size: usize = if message_root.is_empty() {
        TOP_LEVEL_HEADER_SIZE
    } else {
        EMBEDDED_MESSAGE_HEADER_SIZE
    };
    let mut offset = header_size;

    while offset + 16 <= buf.len() {
        let ptype = u16::from_le_bytes([buf[offset], buf[offset + 1]]);
        let pid = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]);

        if pid == prop_id && ptype == 0x0003 {
            return Some(u32::from_le_bytes(buf[offset + 8..offset + 12].try_into().ok()?));
        }
        offset += 16;
    }
    None
}

/// Read a PT_LONG (0x0003) integer property from an attachment's or
/// recipient's `__properties_version1.0` stream, whose header is always 8
/// bytes (MS-OXMSG 2.4.4/2.4.5) regardless of whether the parent message is
/// the top-level message or an embedded message.
fn read_msg_attach_or_recip_int_prop<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    base: &str,
    prop_id: u16,
) -> Option<u32> {
    use std::io::Read;

    const ATTACH_OR_RECIP_HEADER_SIZE: usize = 8;

    let props_path = format!("{base}/__properties_version1.0");
    let mut stream = comp.open_stream(&props_path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;

    let mut offset = ATTACH_OR_RECIP_HEADER_SIZE;
    while offset + 16 <= buf.len() {
        let ptype = u16::from_le_bytes([buf[offset], buf[offset + 1]]);
        let pid = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]);

        if pid == prop_id && ptype == 0x0003 {
            return Some(u32::from_le_bytes(buf[offset + 8..offset + 12].try_into().ok()?));
        }
        offset += 16;
    }
    None
}

/// Read a MAPI string property (tries PT_UNICODE then PT_STRING8).
///
/// `codepage` is the Windows code page to use when decoding PT_STRING8 bytes.
/// Pass `None` to fall back to windows-1252 (safe default for legacy MSG files).
fn read_msg_string_prop<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    base: &str,
    prop_id: u16,
    codepage: Option<u32>,
) -> Option<String> {
    let unicode_path = format!("{base}/__substg1.0_{prop_id:04X}001F");
    if let Some(buf) = read_msg_stream(comp, &unicode_path) {
        return Some(decode_utf16le_bytes(&buf));
    }
    let ansi_path = format!("{base}/__substg1.0_{prop_id:04X}001E");
    read_msg_stream(comp, &ansi_path).map(|buf| {
        let encoding = codepage
            .map(encoding_for_windows_codepage)
            .unwrap_or(encoding_rs::WINDOWS_1252);
        let (decoded, _, _) = encoding.decode(&buf);
        decoded.trim_end_matches('\0').to_string()
    })
}

/// Read the PidTagHtml property, whose canonical MAPI type is PT_BINARY.
///
/// Some producers use a string-typed property instead, so retain those probes
/// before falling back to the standard binary stream. ~keep
fn read_msg_html_prop<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    base: &str,
    codepage: Option<u32>,
) -> Option<String> {
    read_msg_string_prop(comp, base, PID_TAG_HTML, codepage).or_else(|| {
        let path = format!("{base}/__substg1.0_{PID_TAG_HTML:04X}0102");
        let buf = read_msg_stream(comp, &path)?;
        let decoded = if let Some(codepage) = codepage {
            let (text, _, _) = encoding_for_windows_codepage(codepage).decode(&buf);
            text.trim_end_matches('\0').to_string()
        } else {
            match utf8_validation::from_utf8(&buf) {
                Ok(text) => text.trim_start_matches('\u{FEFF}').trim_end_matches('\0').to_string(),
                Err(_) => {
                    let encoding = encoding_rs::WINDOWS_1252;
                    let (text, _, _) = encoding.decode(&buf);
                    text.trim_end_matches('\0').to_string()
                }
            }
        };
        Some(decoded)
    })
}

/// Decode UTF-16LE bytes to a String, stripping trailing NUL chars.
fn decode_utf16le_bytes(data: &[u8]) -> String {
    let u16s: Vec<u16> = data.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&u16s).trim_end_matches('\0').to_string()
}

/// Read a PT_SYSTIME (FILETIME) property from the __properties_version1.0 stream
/// and convert it to an ISO 8601 date string.
///
/// FILETIME is a 64-bit value representing 100-nanosecond intervals since 1601-01-01.
fn read_msg_filetime_prop<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    base: &str,
    prop_id: u16,
) -> Option<String> {
    use std::io::Read;

    let props_path = format!("{base}/__properties_version1.0");
    let mut stream = comp.open_stream(&props_path).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;

    const TOP_LEVEL_HEADER_SIZE: usize = 32;
    const EMBEDDED_MESSAGE_HEADER_SIZE: usize = 24;
    let header_size: usize = if base.is_empty() {
        TOP_LEVEL_HEADER_SIZE
    } else {
        EMBEDDED_MESSAGE_HEADER_SIZE
    };
    let mut offset = header_size;

    while offset + 16 <= buf.len() {
        let ptype = u16::from_le_bytes([buf[offset], buf[offset + 1]]);
        let pid = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]);

        if pid == prop_id && ptype == 0x0040 {
            let filetime = u64::from_le_bytes(buf[offset + 8..offset + 16].try_into().ok()?);
            return filetime_to_iso8601(filetime);
        }
        offset += 16;
    }
    None
}

/// Convert a Windows FILETIME (100-ns intervals since 1601-01-01) to ISO 8601.
fn filetime_to_iso8601(filetime: u64) -> Option<String> {
    const EPOCH_DIFF: u64 = 116_444_736_000_000_000;
    if filetime < EPOCH_DIFF {
        return None;
    }
    let hundred_ns = filetime - EPOCH_DIFF;
    let secs = (hundred_ns / 10_000_000) as i64;
    let nanos = ((hundred_ns % 10_000_000) * 100) as u32;

    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let (hour, min, sec) = (time_of_day / 3600, (time_of_day % 3600) / 60, time_of_day % 60);

    let z = days_since_epoch + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    if nanos == 0 {
        Some(format!("{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}+00:00"))
    } else {
        let frac = nanos / 1_000_000;
        Some(format!(
            "{y:04}-{m:02}-{d:02}T{hour:02}:{min:02}:{sec:02}.{frac:03}+00:00"
        ))
    }
}

/// Read recipients from MSG __recip_version1.0_#XXXXXXXX substorages.
///
/// Returns (to, cc, bcc) vectors. Each entry is formatted as `"Name" <email>` or just `email`.
fn read_msg_recipients<F: std::io::Read + std::io::Seek>(
    comp: &mut cfb::CompoundFile<F>,
    message_root: &str,
    codepage: Option<u32>,
) -> (Vec<String>, Vec<String>, Vec<String>) {
    let recip_paths = direct_child_storage_paths(comp, message_root, "__recip_version1.0_");

    let mut to_emails = Vec::new();
    let mut cc_emails = Vec::new();
    let mut bcc_emails = Vec::new();

    for path in &recip_paths {
        let display_name = read_msg_string_prop(comp, path, 0x3001, codepage);
        let email_addr = read_msg_string_prop(comp, path, 0x39FE, codepage)
            .or_else(|| read_msg_string_prop(comp, path, 0x3003, codepage))
            .filter(|s| !s.is_empty());

        let formatted = match (&display_name, &email_addr) {
            (Some(name), Some(email)) if !name.is_empty() && name != email => {
                format!("\"{}\" <{}>", name, email)
            }
            (_, Some(email)) => email.clone(),
            (Some(name), None) if !name.is_empty() => name.clone(),
            _ => continue,
        };

        let recip_type = read_msg_recip_type(comp, path);
        match recip_type {
            1 => to_emails.push(formatted),
            2 => cc_emails.push(formatted),
            3 => bcc_emails.push(formatted),
            _ => to_emails.push(formatted),
        }
    }

    (to_emails, cc_emails, bcc_emails)
}

/// Read PR_RECIPIENT_TYPE (0x0C15) from a recipient's __properties_version1.0 stream.
/// Returns 1 (To), 2 (CC), 3 (BCC), or 0 if not found.
fn read_msg_recip_type<F: std::io::Read + std::io::Seek>(comp: &mut cfb::CompoundFile<F>, base: &str) -> u32 {
    use std::io::Read;

    let props_path = format!("{base}/__properties_version1.0");
    let mut stream = match comp.open_stream(&props_path) {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let mut buf = Vec::new();
    if stream.read_to_end(&mut buf).is_err() {
        return 0;
    }

    let mut offset = 8;
    while offset + 16 <= buf.len() {
        let ptype = u16::from_le_bytes([buf[offset], buf[offset + 1]]);
        let pid = u16::from_le_bytes([buf[offset + 2], buf[offset + 3]]);

        if pid == 0x0C15 && ptype == 0x0003 {
            return u32::from_le_bytes([buf[offset + 8], buf[offset + 9], buf[offset + 10], buf[offset + 11]]);
        }
        offset += 16;
    }
    0
}

/// Maximum number of bytes scanned for the Date header when no header/body
/// separator is found in the message.
const DATE_HEADER_SCAN_CAP: usize = 8192;

/// Maximum number of bytes scanned for additional raw headers when no
/// header/body separator is found in the message.
const RAW_HEADER_SCAN_CAP: usize = 16384;

/// Find the byte offset of the end of the header section (the blank line
/// separating headers from body), falling back to `cap` bytes if no
/// separator is found.
///
/// Operates purely on bytes so the returned offset is always safe to use as
/// a slice bound on `data` (byte-slice indexing never panics on non-UTF-8
/// boundaries, unlike `&str` indexing).
fn find_header_section_end(data: &[u8], cap: usize) -> usize {
    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
        return pos;
    }
    if let Some(pos) = data.windows(2).position(|w| w == b"\n\n") {
        return pos;
    }
    data.len().min(cap)
}

/// Extract the raw Date header value from email bytes.
///
/// Scans for `Date:` in the header section (before the blank line that separates
/// headers from body) and returns the raw value, handling continuation lines.
///
/// The header section is decoded with `String::from_utf8_lossy` scoped to just
/// the header bytes, so a non-UTF-8 body (or a non-UTF-8 byte sequence split by
/// the scan cap) never causes header parsing to be skipped or to panic.
fn extract_raw_date_header(data: &[u8]) -> Option<String> {
    let header_end = find_header_section_end(data, DATE_HEADER_SCAN_CAP);
    let headers = String::from_utf8_lossy(&data[..header_end]);
    let headers = headers.as_ref();

    let mut date_value = None;
    for line in headers.lines() {
        if let Some(val) = line.strip_prefix("Date:").or_else(|| line.strip_prefix("date:")) {
            date_value = Some(val.trim().to_string());
        } else if date_value.is_some() && (line.starts_with(' ') || line.starts_with('\t')) {
            if let Some(ref mut dv) = date_value {
                dv.push(' ');
                dv.push_str(line.trim());
            }
        } else if date_value.is_some() {
            break;
        }
    }

    date_value.filter(|s| !s.is_empty())
}

/// Extract additional raw headers from email bytes.
///
/// Scans for Content-Type, MIME-Version, X-Mailer, User-Agent, List-Id,
/// and List-Unsubscribe headers in the header section.
///
/// The header section is decoded with `String::from_utf8_lossy` scoped to just
/// the header bytes, so a non-UTF-8 body (or a non-UTF-8 byte sequence split by
/// the scan cap) never causes header parsing to be skipped or to panic.
fn extract_raw_headers(data: &[u8]) -> HashMap<String, String> {
    let mut headers = HashMap::new();

    let header_end = find_header_section_end(data, RAW_HEADER_SCAN_CAP);
    let header_section = String::from_utf8_lossy(&data[..header_end]);
    let header_section = header_section.as_ref();

    let target_headers: &[(&str, &str)] = &[
        ("content-type:", "content_type"),
        ("mime-version:", "mime_version"),
        ("x-mailer:", "x_mailer"),
        ("user-agent:", "user_agent"),
        ("list-id:", "list_id"),
        ("list-unsubscribe:", "list_unsubscribe"),
    ];

    let mut current_key: Option<&str> = None;
    let mut current_value = String::new();

    for line in header_section.lines() {
        if line.starts_with(' ') || line.starts_with('\t') {
            if current_key.is_some() {
                current_value.push(' ');
                current_value.push_str(line.trim());
            }
            continue;
        }

        if let Some(key) = current_key {
            if !current_value.is_empty() {
                headers.insert(key.to_string(), current_value.clone());
            }
            current_key = None;
            current_value.clear();
        }

        let line_lower = line.to_lowercase();
        for &(prefix, meta_key) in target_headers {
            if line_lower.starts_with(prefix) {
                current_key = Some(meta_key);
                current_value = line[prefix.len()..].trim().to_string();
                break;
            }
        }
    }

    if let Some(key) = current_key
        && !current_value.is_empty()
    {
        headers.insert(key.to_string(), current_value);
    }

    headers
}

/// Extract email content from either .eml or .msg format
pub(crate) fn extract_email_content(
    data: &[u8],
    mime_type: &str,
    fallback_codepage: Option<u32>,
) -> Result<EmailExtractionResult> {
    if data.is_empty() {
        return Err(XbergError::validation("Email content is empty".to_string()));
    }

    match mime_type {
        "message/rfc822" | "text/plain" => parse_eml_content(data),
        "application/vnd.ms-outlook" => parse_msg_content(data, fallback_codepage),
        _ => Err(XbergError::validation(format!(
            "Unsupported email MIME type: {}",
            mime_type
        ))),
    }
}

pub(crate) fn extract_email_content_with_nested(
    data: &[u8],
    mime_type: &str,
    fallback_codepage: Option<u32>,
    max_nested_message_bytes: Option<u64>,
    security_limits: &SecurityLimits,
) -> Result<ParsedEmailContent> {
    if data.is_empty() {
        return Err(XbergError::validation("Email content is empty".to_string()));
    }

    match mime_type {
        "message/rfc822" | "text/plain" => parse_eml_content_internal(data, true, max_nested_message_bytes),
        "application/vnd.ms-outlook" => {
            let (result, nested_embedded_messages, warnings) =
                parse_msg_content_with_nested(data, fallback_codepage, security_limits)?;
            Ok(ParsedEmailContent {
                result,
                nested_messages: Vec::new(),
                nested_embedded_messages,
                warnings,
            })
        }
        _ => Err(XbergError::validation(format!(
            "Unsupported email MIME type: {}",
            mime_type
        ))),
    }
}

/// Build text output from email extraction result
///
/// Renders Subject/From/To/CC/BCC/Date plus, when present, Reply-To, Message-ID,
/// In-Reply-To, References, List-Id, and List-Unsubscribe. Headers with no value
/// are omitted rather than rendered as empty lines.
pub(crate) fn build_email_text_output(result: &EmailExtractionResult) -> String {
    let mut text_parts = Vec::with_capacity(16);

    if let Some(ref subject) = result.subject {
        text_parts.push(format!("Subject: {}", subject));
    }

    if let Some(ref from) = result.from_email {
        text_parts.push(format!("From: {}", from));
    }

    if !result.to_emails.is_empty() {
        text_parts.push(format!("To: {}", result.to_emails.join(", ")));
    }

    if !result.cc_emails.is_empty() {
        text_parts.push(format!("CC: {}", result.cc_emails.join(", ")));
    }

    if !result.bcc_emails.is_empty() {
        text_parts.push(format!("BCC: {}", result.bcc_emails.join(", ")));
    }

    if let Some(reply_to) = result.metadata.get("reply_to") {
        text_parts.push(format!("Reply-To: {}", reply_to));
    }

    if let Some(ref date) = result.date {
        text_parts.push(format!("Date: {}", date));
    }

    if let Some(ref message_id) = result.message_id {
        text_parts.push(format!("Message-ID: {}", message_id));
    }

    if let Some(in_reply_to) = result.metadata.get("in_reply_to") {
        text_parts.push(format!("In-Reply-To: {}", in_reply_to));
    }

    if let Some(references) = result.metadata.get("references") {
        text_parts.push(format!("References: {}", references));
    }

    if let Some(list_id) = result.metadata.get("list_id") {
        text_parts.push(format!("List-Id: {}", list_id));
    }

    if let Some(list_unsubscribe) = result.metadata.get("list_unsubscribe") {
        text_parts.push(format!("List-Unsubscribe: {}", list_unsubscribe));
    }

    text_parts.push(result.content.clone());

    text_parts.join("\n")
}

pub(crate) fn clean_html_content(html: &str) -> String {
    if html.is_empty() {
        return String::new();
    }

    #[cfg(feature = "html")]
    {
        if let Ok(text) = crate::extraction::html::convert_html_to_markdown(
            html,
            None,
            Some(crate::core::config::OutputFormat::Plain),
        ) {
            let trimmed = text.trim().to_string();
            if !trimmed.is_empty() {
                return trimmed;
            }
        }
    }

    let cleaned = script_regex().replace_all(html, "");
    let cleaned = style_regex().replace_all(&cleaned, "");
    let cleaned = html_tag_regex().replace_all(&cleaned, "");
    let cleaned = whitespace_regex().replace_all(&cleaned, " ");

    cleaned.trim().to_string()
}

fn is_image_mime_type(mime_type: &str) -> bool {
    mime_type.starts_with("image/")
}

fn parse_content_type(content_type: &str) -> String {
    let trimmed = content_type.trim();
    if trimmed.is_empty() {
        return "application/octet-stream".to_string();
    }
    trimmed
        .split(';')
        .next()
        .unwrap_or("application/octet-stream")
        .trim()
        .to_lowercase()
}

#[allow(clippy::too_many_arguments)]
fn build_metadata(
    subject: &Option<String>,
    from_email: &Option<String>,
    to_emails: &[String],
    cc_emails: &[String],
    bcc_emails: &[String],
    date: &Option<String>,
    message_id: &Option<String>,
    attachments: &[EmailAttachment],
) -> HashMap<String, String> {
    let mut metadata = HashMap::new();

    if let Some(subj) = subject {
        metadata.insert("subject".to_string(), subj.clone());
    }
    if let Some(from) = from_email {
        metadata.insert("email_from".to_string(), from.clone());
    }
    if !to_emails.is_empty() {
        metadata.insert("email_to".to_string(), to_emails.join(", "));
    }
    if !cc_emails.is_empty() {
        metadata.insert("email_cc".to_string(), cc_emails.join(", "));
    }
    if !bcc_emails.is_empty() {
        metadata.insert("email_bcc".to_string(), bcc_emails.join(", "));
    }
    if let Some(dt) = date {
        metadata.insert("date".to_string(), dt.clone());
    }
    if let Some(msg_id) = message_id {
        metadata.insert("message_id".to_string(), msg_id.clone());
    }

    if !attachments.is_empty() {
        let attachment_names: Vec<String> = attachments
            .iter()
            .filter_map(|att| att.name.as_ref().or(att.filename.as_ref()))
            .cloned()
            .collect();
        if !attachment_names.is_empty() {
            metadata.insert("attachments".to_string(), attachment_names.join(", "));
        }
    }

    metadata
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clean_html_content() {
        let html = "<p>Hello <b>World</b></p>";
        let cleaned = clean_html_content(html);
        assert_eq!(cleaned, "Hello World");
    }

    #[test]
    fn test_clean_html_with_whitespace() {
        let html = "<div>  Multiple   \n  spaces  </div>";
        let cleaned = clean_html_content(html);
        assert!(
            cleaned.contains("Multiple") && cleaned.contains("spaces"),
            "Should contain text: {}",
            cleaned
        );
        assert!(
            !cleaned.contains("  "),
            "Should not have consecutive spaces: {}",
            cleaned
        );
    }

    #[test]
    fn test_clean_html_with_script_and_style() {
        let html = r#"
            <html>
                <head><style>body { color: red; }</style></head>
                <body>
                    <script>alert('test');</script>
                    <p>Hello World</p>
                </body>
            </html>
        "#;
        let cleaned = clean_html_content(html);
        assert!(!cleaned.contains("<script>"));
        assert!(!cleaned.contains("<style>"));
        assert!(cleaned.contains("Hello World"));
    }

    #[test]
    fn test_is_image_mime_type() {
        assert!(is_image_mime_type("image/png"));
        assert!(is_image_mime_type("image/jpeg"));
        assert!(!is_image_mime_type("text/plain"));
        assert!(!is_image_mime_type("application/pdf"));
    }

    #[test]
    fn test_parse_content_type() {
        assert_eq!(parse_content_type("text/plain"), "text/plain");
        assert_eq!(parse_content_type("text/plain; charset=utf-8"), "text/plain");
        assert_eq!(parse_content_type("image/jpeg; name=test.jpg"), "image/jpeg");
        assert_eq!(parse_content_type(""), "application/octet-stream");
    }

    #[test]
    fn test_extract_email_content_empty_data() {
        let result = extract_email_content(b"", "message/rfc822", None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), XbergError::Validation { .. }));
    }

    #[test]
    fn test_extract_email_content_invalid_mime_type() {
        let result = extract_email_content(b"test", "application/pdf", None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), XbergError::Validation { .. }));
    }

    #[test]
    fn test_parse_eml_content_invalid() {
        let result = parse_eml_content(b"not an email");
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_msg_content_invalid() {
        let result = parse_msg_content(b"not a msg file", None);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), XbergError::Parsing { .. }));
    }

    #[test]
    fn test_simple_eml_parsing() {
        let eml_content =
            b"From: test@example.com\r\nTo: recipient@example.com\r\nSubject: Test Email\r\n\r\nThis is a test email body.";

        let result = parse_eml_content(eml_content).unwrap();
        assert_eq!(result.subject, Some("Test Email".to_string()));
        assert_eq!(result.from_email, Some("test@example.com".to_string()));
        assert_eq!(result.to_emails, vec!["recipient@example.com".to_string()]);
        assert_eq!(result.content, "This is a test email body.");
    }

    #[test]
    fn test_eml_sender_display_name_is_preserved_separately() {
        let eml_content = b"From: Alice Example <alice@example.com>\r\nSubject: Sender\r\n\r\nBody";

        let result = parse_eml_content(eml_content).unwrap();

        assert_eq!(result.from_email.as_deref(), Some("alice@example.com"));
        assert_eq!(
            result.metadata.get("from_name").map(String::as_str),
            Some("Alice Example")
        );
    }

    #[test]
    fn test_build_email_text_output_minimal() {
        let result = EmailExtractionResult {
            subject: Some("Test".to_string()),
            from_email: Some("sender@example.com".to_string()),
            to_emails: vec!["recipient@example.com".to_string()],
            cc_emails: vec![],
            bcc_emails: vec![],
            date: None,
            message_id: None,
            plain_text: None,
            html_content: None,
            content: "Hello World".to_string(),
            attachments: vec![],
            metadata: HashMap::new(),
        };

        let output = build_email_text_output(&result);
        assert!(output.contains("Subject: Test"));
        assert!(output.contains("From: sender@example.com"));
        assert!(output.contains("To: recipient@example.com"));
        assert!(output.contains("Hello World"));
    }

    #[test]
    fn test_build_email_text_output_with_attachments() {
        let result = EmailExtractionResult {
            subject: Some("Test".to_string()),
            from_email: Some("sender@example.com".to_string()),
            to_emails: vec!["recipient@example.com".to_string()],
            cc_emails: vec![],
            bcc_emails: vec![],
            date: None,
            message_id: None,
            plain_text: None,
            html_content: None,
            content: "Hello World".to_string(),
            attachments: vec![EmailAttachment {
                name: Some("file.txt".to_string()),
                filename: Some("file.txt".to_string()),
                mime_type: Some("text/plain".to_string()),
                size: Some(1024),
                is_image: false,
                data: None,
            }],
            metadata: HashMap::new(),
        };

        let output = build_email_text_output(&result);
        assert!(!output.contains("Attachments:"));
        assert!(output.contains("Hello World"));
    }

    #[test]
    fn test_build_metadata() {
        let subject = Some("Test Subject".to_string());
        let from_email = Some("sender@example.com".to_string());
        let to_emails = vec!["recipient@example.com".to_string()];
        let cc_emails = vec!["cc@example.com".to_string()];
        let bcc_emails = vec!["bcc@example.com".to_string()];
        let date = Some("2024-01-01T12:00:00Z".to_string());
        let message_id = Some("<abc123@example.com>".to_string());
        let attachments = vec![];

        let metadata = build_metadata(
            &subject,
            &from_email,
            &to_emails,
            &cc_emails,
            &bcc_emails,
            &date,
            &message_id,
            &attachments,
        );

        assert_eq!(metadata.get("subject"), Some(&"Test Subject".to_string()));
        assert_eq!(metadata.get("email_from"), Some(&"sender@example.com".to_string()));
        assert_eq!(metadata.get("email_to"), Some(&"recipient@example.com".to_string()));
        assert_eq!(metadata.get("email_cc"), Some(&"cc@example.com".to_string()));
        assert_eq!(metadata.get("email_bcc"), Some(&"bcc@example.com".to_string()));
        assert_eq!(metadata.get("date"), Some(&"2024-01-01T12:00:00Z".to_string()));
        assert_eq!(metadata.get("message_id"), Some(&"<abc123@example.com>".to_string()));
    }

    #[test]
    fn test_build_metadata_with_attachments() {
        let attachments = vec![
            EmailAttachment {
                name: Some("file1.pdf".to_string()),
                filename: Some("file1.pdf".to_string()),
                mime_type: Some("application/pdf".to_string()),
                size: Some(1024),
                is_image: false,
                data: None,
            },
            EmailAttachment {
                name: Some("image.png".to_string()),
                filename: Some("image.png".to_string()),
                mime_type: Some("image/png".to_string()),
                size: Some(2048),
                is_image: true,
                data: None,
            },
        ];

        let metadata = build_metadata(&None, &None, &[], &[], &[], &None, &None, &attachments);

        assert_eq!(metadata.get("attachments"), Some(&"file1.pdf, image.png".to_string()));
    }

    #[test]
    fn test_clean_html_content_empty() {
        let cleaned = clean_html_content("");
        assert_eq!(cleaned, "");
    }

    #[test]
    fn test_clean_html_content_only_tags() {
        let html = "<div><span><p></p></span></div>";
        let cleaned = clean_html_content(html);
        assert_eq!(cleaned, "");
    }

    #[test]
    fn test_clean_html_content_nested_tags() {
        let html = "<div><p>Outer <span>Inner <b>Bold</b></span> Text</p></div>";
        let cleaned = clean_html_content(html);
        assert_eq!(cleaned, "Outer Inner Bold Text");
    }

    #[test]
    fn test_clean_html_content_multiple_scripts() {
        let html = r#"
            <script>function a() {}</script>
            <p>Content</p>
            <script>function b() {}</script>
        "#;
        let cleaned = clean_html_content(html);
        assert!(!cleaned.contains("function"));
        assert!(cleaned.contains("Content"));
    }

    #[test]
    fn test_is_image_mime_type_variants() {
        assert!(is_image_mime_type("image/gif"));
        assert!(is_image_mime_type("image/webp"));
        assert!(is_image_mime_type("image/svg+xml"));
        assert!(!is_image_mime_type("video/mp4"));
        assert!(!is_image_mime_type("audio/mp3"));
    }

    #[test]
    fn test_parse_content_type_with_parameters() {
        assert_eq!(parse_content_type("multipart/mixed; boundary=xyz"), "multipart/mixed");
        assert_eq!(parse_content_type("text/html; charset=UTF-8"), "text/html");
    }

    #[test]
    fn test_parse_content_type_whitespace() {
        assert_eq!(parse_content_type("  text/plain  "), "text/plain");
        assert_eq!(parse_content_type(" text/plain ; charset=utf-8 "), "text/plain");
    }

    #[test]
    fn test_parse_content_type_case_insensitive() {
        assert_eq!(parse_content_type("TEXT/PLAIN"), "text/plain");
        assert_eq!(parse_content_type("Image/JPEG"), "image/jpeg");
    }

    #[test]
    fn test_extract_email_content_mime_variants() {
        let eml_content = b"From: test@example.com\r\n\r\nBody";

        assert!(extract_email_content(eml_content, "message/rfc822", None).is_ok());
        assert!(extract_email_content(eml_content, "text/plain", None).is_ok());
    }

    #[test]
    fn test_simple_eml_with_multiple_recipients() {
        let eml_content = b"From: sender@example.com\r\nTo: r1@example.com, r2@example.com\r\nCc: cc@example.com\r\nBcc: bcc@example.com\r\nSubject: Multi-recipient\r\n\r\nBody";

        let result = parse_eml_content(eml_content).unwrap();
        assert_eq!(result.to_emails.len(), 2);
        assert!(result.to_emails.contains(&"r1@example.com".to_string()));
        assert!(result.to_emails.contains(&"r2@example.com".to_string()));
    }

    #[test]
    fn test_simple_eml_with_html_body() {
        let eml_content = b"From: sender@example.com\r\nTo: recipient@example.com\r\nSubject: HTML Email\r\nContent-Type: text/html\r\n\r\n<html><body><p>HTML Body</p></body></html>";

        let result = parse_eml_content(eml_content).unwrap();
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_build_email_text_output_with_all_fields() {
        let result = EmailExtractionResult {
            subject: Some("Complete Email".to_string()),
            from_email: Some("sender@example.com".to_string()),
            to_emails: vec!["recipient@example.com".to_string()],
            cc_emails: vec!["cc@example.com".to_string()],
            bcc_emails: vec!["bcc@example.com".to_string()],
            date: Some("2024-01-01T12:00:00Z".to_string()),
            message_id: Some("<msg123@example.com>".to_string()),
            plain_text: Some("Plain text body".to_string()),
            html_content: Some("<html><body>HTML body</body></html>".to_string()),
            content: "Cleaned body text".to_string(),
            attachments: vec![],
            metadata: HashMap::new(),
        };

        let output = build_email_text_output(&result);
        assert!(output.contains("Subject: Complete Email"));
        assert!(output.contains("From: sender@example.com"));
        assert!(output.contains("To: recipient@example.com"));
        assert!(output.contains("CC: cc@example.com"));
        assert!(output.contains("BCC: bcc@example.com"));
        assert!(output.contains("Date: 2024-01-01T12:00:00Z"));
        assert!(output.contains("Cleaned body text"));
    }

    #[test]
    fn test_build_email_text_output_empty_attachments() {
        let result = EmailExtractionResult {
            subject: Some("Test".to_string()),
            from_email: Some("sender@example.com".to_string()),
            to_emails: vec!["recipient@example.com".to_string()],
            cc_emails: vec![],
            bcc_emails: vec![],
            date: None,
            message_id: None,
            plain_text: None,
            html_content: None,
            content: "Body".to_string(),
            attachments: vec![EmailAttachment {
                name: None,
                filename: None,
                mime_type: Some("application/octet-stream".to_string()),
                size: Some(100),
                is_image: false,
                data: None,
            }],
            metadata: HashMap::new(),
        };

        let output = build_email_text_output(&result);
        assert!(output.contains("Body"));
    }

    #[test]
    fn test_build_metadata_empty_fields() {
        let metadata = build_metadata(&None, &None, &[], &[], &[], &None, &None, &[]);
        assert!(metadata.is_empty());
    }

    #[test]
    fn test_build_metadata_partial_fields() {
        let subject = Some("Test".to_string());
        let date = Some("2024-01-01".to_string());

        let metadata = build_metadata(&subject, &None, &[], &[], &[], &date, &None, &[]);

        assert_eq!(metadata.get("subject"), Some(&"Test".to_string()));
        assert_eq!(metadata.get("date"), Some(&"2024-01-01".to_string()));
        assert_eq!(metadata.len(), 2);
    }

    #[test]
    fn test_clean_html_content_case_insensitive_tags() {
        let html = "<SCRIPT>code</SCRIPT><STYLE>css</STYLE><P>Text</P>";
        let cleaned = clean_html_content(html);
        assert!(!cleaned.contains("code"));
        assert!(!cleaned.contains("css"));
        assert!(cleaned.contains("Text"));
    }

    #[test]
    fn test_simple_eml_with_date() {
        let eml_content = b"From: sender@example.com\r\nTo: recipient@example.com\r\nDate: Mon, 1 Jan 2024 12:00:00 +0000\r\nSubject: Test\r\n\r\nBody";

        let result = parse_eml_content(eml_content).unwrap();
        assert!(result.date.is_some());
    }

    #[test]
    fn test_simple_eml_with_message_id() {
        let eml_content = b"From: sender@example.com\r\nTo: recipient@example.com\r\nMessage-ID: <unique@example.com>\r\nSubject: Test\r\n\r\nBody";

        let result = parse_eml_content(eml_content).unwrap();
        assert!(result.message_id.is_some());
    }

    #[test]
    fn test_simple_eml_minimal() {
        let eml_content = b"From: sender@example.com\r\n\r\nMinimal body";

        let result = parse_eml_content(eml_content).unwrap();
        assert_eq!(result.from_email, Some("sender@example.com".to_string()));
        assert_eq!(result.content, "Minimal body");
    }

    #[test]
    fn test_regex_initialization() {
        let _ = html_tag_regex();
        let _ = script_regex();
        let _ = style_regex();
        let _ = whitespace_regex();

        let _ = html_tag_regex();
        let _ = script_regex();
        let _ = style_regex();
        let _ = whitespace_regex();
    }

    #[test]
    fn test_clean_html_content_multiline_script() {
        let html = r#"<html>
<head>
    <title>HTML Email</title>
    <style>
        body { font-family: Arial, sans-serif; }
        .header { color: blue; }
        .content { margin: 10px; }
    </style>
</head>
<body>
    <div class="header">
        <h1>Welcome to Our Service</h1>
    </div>

    <div class="content">
        <p>This email contains <strong>only HTML</strong> content.</p>

        <p>It includes:</p>
        <ul>
            <li>HTML entities: &lt;, &gt;, &amp;, &quot;</li>
            <li>Special characters: €, ©, ®</li>
            <li>Formatting: <em>italic</em>, <strong>bold</strong></li>
        </ul>

        <p>The Rust implementation should clean this HTML and extract meaningful text.</p>

        <script>
            // This script should be removed during HTML cleaning
            alert('Should not appear in extracted text');
        </script>
    </div>

    <footer>
        <p>&copy; 2024 Example Company</p>
    </footer>
</body>
</html>"#;
        let cleaned = clean_html_content(html);
        assert!(
            !cleaned.contains("script should be removed"),
            "Script content leaked into output: {}",
            cleaned
        );
        assert!(
            !cleaned.contains("alert("),
            "Script content leaked into output: {}",
            cleaned
        );
        assert!(cleaned.contains("Welcome to Our Service"));
    }

    #[test]
    fn test_html_only_eml_script_stripping() {
        let eml_data = std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test_documents/email/html_only.eml"
        ))
        .expect("html_only.eml should exist");
        let result = parse_eml_content(&eml_data).unwrap();
        assert!(
            !result.content.contains("script should be removed"),
            "Script content leaked into content: {}",
            result.content
        );
        assert!(
            !result.content.contains("alert("),
            "Script content leaked into content: {}",
            result.content
        );
        assert!(
            result.content.contains("Welcome to Our Service"),
            "Expected content missing from content: {}",
            result.content
        );
    }

    #[test]
    fn test_parse_msg_content_invalid_with_fallback() {
        let result = parse_msg_content(b"not a msg file", Some(1251));
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), XbergError::Parsing { .. }));
    }

    #[test]
    fn test_extract_email_content_invalid_codepage_is_silent() {
        let eml = b"From: a@b.com\r\nSubject: Test\r\n\r\nBody";
        let result = extract_email_content(eml, "message/rfc822", Some(99999));
        assert!(result.is_ok());
    }

    #[test]
    fn test_parse_msg_default_unchanged_with_real_fixture() {
        let data = include_bytes!("../../../../test_documents/vendored/unstructured/msg/fake-email.msg");
        let result = parse_msg_content(data, None).unwrap();
        assert!(!result.content.is_empty());
    }

    #[test]
    fn test_parse_msg_reads_binary_html_body_with_attachment() {
        let data = include_bytes!("../../../../test_documents/email/msg_with_attachments_alt.msg");

        let result = parse_msg_content(data, None).unwrap();

        assert_eq!(result.subject.as_deref(), Some("This is the subject"));
        assert_eq!(result.from_email.as_deref(), Some("peterpan@neverland.com"));
        assert_eq!(result.to_emails, vec!["crocodile@neverland.com".to_string()]);
        assert_eq!(result.content, "This is a message");
        assert_eq!(result.html_content.as_deref(), Some("This is a message"));
        assert_eq!(result.attachments.len(), 1);
        assert_eq!(result.attachments[0].filename.as_deref(), Some("canvas.png"));
    }

    #[test]
    fn test_parse_msg_invalid_codepage_falls_back_silently() {
        let data = include_bytes!("../../../../test_documents/vendored/unstructured/msg/fake-email.msg");
        let result_invalid = parse_msg_content(data, Some(99999)).unwrap();
        let result_default = parse_msg_content(data, None).unwrap();
        assert_eq!(result_invalid.subject, result_default.subject);
        assert_eq!(result_invalid.content, result_default.content);
    }

    #[test]
    fn test_eml_threading_headers() {
        let eml = b"From: alice@example.com\r\n\
            To: bob@example.com\r\n\
            Subject: Re: Thread test\r\n\
            Message-ID: <msg2@example.com>\r\n\
            In-Reply-To: <msg1@example.com>\r\n\
            References: <msg0@example.com> <msg1@example.com>\r\n\
            Reply-To: noreply@example.com\r\n\
            \r\n\
            Reply body";

        let result = parse_eml_content(eml).unwrap();
        assert!(
            result.metadata.contains_key("in_reply_to"),
            "Should extract In-Reply-To header"
        );
        assert!(
            result.metadata.contains_key("references"),
            "Should extract References header"
        );
        assert!(
            result.metadata.contains_key("reply_to"),
            "Should extract Reply-To header"
        );
        assert!(result.metadata.get("reply_to").unwrap().contains("noreply@example.com"));
    }

    #[test]
    fn test_eml_raw_headers() {
        let eml = b"From: alice@example.com\r\n\
            To: bob@example.com\r\n\
            Subject: Header test\r\n\
            Content-Type: text/plain; charset=utf-8\r\n\
            MIME-Version: 1.0\r\n\
            X-Mailer: TestMailer/1.0\r\n\
            List-Id: <test.example.com>\r\n\
            List-Unsubscribe: <mailto:unsub@example.com>\r\n\
            \r\n\
            Body content";

        let result = parse_eml_content(eml).unwrap();
        assert!(
            result.metadata.contains_key("content_type"),
            "Should extract Content-Type"
        );
        assert!(result.metadata.get("content_type").unwrap().contains("text/plain"));
        assert!(
            result.metadata.contains_key("mime_version"),
            "Should extract MIME-Version"
        );
        assert_eq!(result.metadata.get("mime_version").unwrap(), "1.0");
        assert!(result.metadata.contains_key("x_mailer"), "Should extract X-Mailer");
        assert!(result.metadata.get("x_mailer").unwrap().contains("TestMailer"));
        assert!(result.metadata.contains_key("list_id"), "Should extract List-Id");
        assert!(
            result.metadata.contains_key("list_unsubscribe"),
            "Should extract List-Unsubscribe"
        );
    }

    #[test]
    fn test_eml_attachment_details_metadata() {
        let eml = b"From: alice@example.com\r\n\
            To: bob@example.com\r\n\
            Subject: With attachment\r\n\
            MIME-Version: 1.0\r\n\
            Content-Type: multipart/mixed; boundary=\"BOUNDARY\"\r\n\
            \r\n\
            --BOUNDARY\r\n\
            Content-Type: text/plain\r\n\
            \r\n\
            Body text\r\n\
            --BOUNDARY\r\n\
            Content-Type: application/pdf\r\n\
            Content-Disposition: attachment; filename=\"report.pdf\"\r\n\
            \r\n\
            FAKEPDFDATA\r\n\
            --BOUNDARY--";

        let result = parse_eml_content(eml).unwrap();
        assert!(!result.attachments.is_empty(), "Should have attachment");
        assert!(
            result.metadata.contains_key("attachment_details"),
            "Should have attachment_details"
        );
        let details = result.metadata.get("attachment_details").unwrap();
        assert!(details.contains("report.pdf"), "Should contain filename");
        assert!(details.contains("application/pdf"), "Should contain mime type");
    }

    #[test]
    fn test_extract_raw_headers_function() {
        let data = b"From: alice@example.com\r\n\
            Content-Type: multipart/mixed; boundary=foo\r\n\
            MIME-Version: 1.0\r\n\
            X-Mailer: MyApp/2.0\r\n\
            User-Agent: MyAgent/1.0\r\n\
            \r\n\
            Body";

        let headers = extract_raw_headers(data);
        assert_eq!(headers.get("content_type").unwrap(), "multipart/mixed; boundary=foo");
        assert_eq!(headers.get("mime_version").unwrap(), "1.0");
        assert_eq!(headers.get("x_mailer").unwrap(), "MyApp/2.0");
        assert_eq!(headers.get("user_agent").unwrap(), "MyAgent/1.0");
    }

    #[test]
    fn test_decompress_rtf_compressed_crafted_raw_size_does_not_over_allocate() {
        let mut data = Vec::with_capacity(20);
        data.extend_from_slice(&16u32.to_le_bytes());
        data.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        data.extend_from_slice(&0x75465a4cu32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(&[0x00, b'A', b'B', b'C']);

        let result = decompress_rtf_compressed(&data);
        let out = result.expect("should decompress without error");
        assert!(out.len() < 16, "output should be tiny, not a huge allocation");
    }

    #[test]
    fn test_decompress_rtf_compressed_cap_is_hint_only() {
        let payload: &[u8] = &[
            0x00, b'A', b'B', b'C', b'D', b'E', b'F', b'G', b'H', 0x00, b'I', b'J', b'K', b'L', b'M', b'N', b'O', b'P',
            0x00, b'Q', b'R', b'S', b'T', b'U', b'V', b'W', b'X',
        ];
        let comp_size = (12 + payload.len()) as u32;
        let raw_size = 1u32;
        let mut data: Vec<u8> = Vec::new();
        data.extend_from_slice(&comp_size.to_le_bytes());
        data.extend_from_slice(&raw_size.to_le_bytes());
        data.extend_from_slice(&0x75465a4cu32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(payload);

        let out = decompress_rtf_compressed(&data).expect("should decompress");
        assert_eq!(
            out.len(),
            24,
            "Vec must grow past the raw_size=1 hint to hold all 24 bytes"
        );
        assert_eq!(&out[..8], b"ABCDEFGH");
    }

    #[test]
    fn test_decompress_rtf_compressed_too_short() {
        assert!(decompress_rtf_compressed(&[0u8; 10]).is_none());
    }

    #[test]
    fn test_decompress_rtf_compressed_bad_magic() {
        let mut data = [0u8; 16];
        data[8..12].copy_from_slice(&0xDEAD_BEEFu32.to_le_bytes());
        assert!(decompress_rtf_compressed(&data).is_none());
    }

    #[test]
    fn test_decompress_rtf_compressed_uncompressed_magic() {
        let rtf_payload = b"{\\rtf1 Hello}";
        let comp_size = (rtf_payload.len() + 12) as u32;
        let raw_size = rtf_payload.len() as u32;
        let mut data = Vec::new();
        data.extend_from_slice(&comp_size.to_le_bytes());
        data.extend_from_slice(&raw_size.to_le_bytes());
        data.extend_from_slice(&0x414c_454du32.to_le_bytes());
        data.extend_from_slice(&0u32.to_le_bytes());
        data.extend_from_slice(rtf_payload);

        let result = decompress_rtf_compressed(&data).unwrap();
        assert_eq!(result, rtf_payload);
    }

    #[test]
    fn test_strip_rtf_to_plain_text_basic() {
        let rtf = b"{\\rtf1\\ansi\\deff0 Hello World}";
        let result = strip_rtf_to_plain_text(rtf);
        assert_eq!(result, "Hello World");
    }

    #[test]
    fn test_strip_rtf_to_plain_text_par() {
        let rtf = b"{\\rtf1\\ansi Line 1\\par Line 2}";
        let result = strip_rtf_to_plain_text(rtf);
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
        assert!(result.contains('\n'));
    }

    #[test]
    fn test_strip_rtf_to_plain_text_unicode() {
        let rtf = b"{\\rtf1 Price: \\u8364?100}";
        let result = strip_rtf_to_plain_text(rtf);
        assert!(result.contains('\u{20AC}'));
        assert!(result.contains("100"));
    }

    #[test]
    fn test_strip_rtf_to_plain_text_hex_escape() {
        let rtf = b"{\\rtf1 caf\\'e9}";
        let result = strip_rtf_to_plain_text(rtf);
        assert!(result.contains("caf\u{00e9}"));
    }

    #[test]
    fn test_strip_rtf_to_plain_text_skips_fonttbl() {
        let rtf = b"{\\rtf1{\\fonttbl{\\f0 Arial;}}{\\f0 Visible text}}";
        let result = strip_rtf_to_plain_text(rtf);
        assert!(!result.contains("Arial"));
        assert!(result.contains("Visible text"));
    }

    #[test]
    fn test_strip_rtf_to_plain_text_escaped_braces() {
        let rtf = b"{\\rtf1 Open \\{ and close \\}}";
        let result = strip_rtf_to_plain_text(rtf);
        assert!(result.contains("Open {"));
        assert!(result.contains("close }"));
    }

    #[test]
    fn test_strip_rtf_to_plain_text_empty() {
        let rtf = b"{\\rtf1}";
        let result = strip_rtf_to_plain_text(rtf);
        assert!(result.is_empty());
    }

    #[test]
    fn test_maybe_transcode_utf16_robustness() {
        let tiny_ambiguous = b"t\0e\0";
        assert!(maybe_transcode_utf16(tiny_ambiguous).is_none());

        let mut with_bom_le = vec![0xFF, 0xFE];
        with_bom_le.extend_from_slice(&"Test".encode_utf16().flat_map(|u| u.to_le_bytes()).collect::<Vec<u8>>());
        let result = maybe_transcode_utf16(&with_bom_le).expect("Should transcode LE with BOM");
        assert_eq!(String::from_utf8(result).unwrap(), "Test");

        let mut with_bom_be = vec![0xFE, 0xFF];
        with_bom_be.extend_from_slice(&"Test".encode_utf16().flat_map(|u| u.to_be_bytes()).collect::<Vec<u8>>());
        let result = maybe_transcode_utf16(&with_bom_be).expect("Should transcode BE with BOM");
        assert_eq!(String::from_utf8(result).unwrap(), "Test");

        let long_text = "Subject: This is a long enough string to trigger the heuristic.";
        let long_utf16_le: Vec<u8> = long_text.encode_utf16().flat_map(|u| u.to_le_bytes()).collect();
        let result = maybe_transcode_utf16(&long_utf16_le).expect("Should transcode long non-BOM LE");
        assert_eq!(String::from_utf8(result).unwrap(), long_text);

        let long_utf8 = b"Subject: This is a normal UTF-8 string without any null bytes.";
        assert!(maybe_transcode_utf16(long_utf8).is_none());
    }
}
