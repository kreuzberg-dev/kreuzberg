use super::DecodeOutcome;
use ahash::AHashMap;
use chardetng::EncodingDetector;
use encoding_rs::Encoding;
use regex::Regex;
use std::borrow::Cow;
use std::collections::VecDeque;
use std::env;
use std::sync::LazyLock;
use std::sync::RwLock;

static CONTROL_CHARS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\x00-\x08\x0B-\x0C\x0E-\x1F\x7F-\x9F]")
        .expect("Control chars regex pattern is valid and should compile")
});
static REPLACEMENT_CHARS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\u{FFFD}+").expect("Replacement chars regex pattern is valid and should compile"));
static ISOLATED_COMBINING: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\u{0300}-\u{036F}]+")
        .expect("Isolated combining diacritics regex pattern is valid and should compile")
});
static HEBREW_AS_CYRILLIC: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[\u{0400}-\u{04FF}]{3,}")
        .expect("Hebrew misencoded as Cyrillic regex pattern is valid and should compile")
});

struct EncodingCache {
    entries: AHashMap<String, &'static Encoding>,
    order: VecDeque<String>,
    max_entries: usize,
    max_bytes: usize,
    current_bytes: usize,
}

impl EncodingCache {
    fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            entries: AHashMap::new(),
            order: VecDeque::with_capacity(max_entries),
            max_entries,
            max_bytes,
            current_bytes: 0,
        }
    }

    fn get(&mut self, key: &str) -> Option<&'static Encoding> {
        if let Some(&encoding) = self.entries.get(key) {
            if let Some(pos) = self.order.iter().position(|existing| existing == key)
                && pos + 1 != self.order.len()
                && let Some(entry) = self.order.remove(pos)
            {
                self.order.push_back(entry);
            }
            return Some(encoding);
        }

        None
    }

    fn insert(&mut self, key: String, encoding: &'static Encoding) {
        let key_len = key.len();

        if let Some(pos) = self.order.iter().position(|existing| existing == &key) {
            self.order.remove(pos);
            self.current_bytes = self.current_bytes.saturating_sub(key_len);
        }

        if self.entries.contains_key(&key) {
            self.current_bytes = self.current_bytes.saturating_sub(key_len);
        }

        self.entries.insert(key.clone(), encoding);
        self.current_bytes = self.current_bytes.saturating_add(key_len);
        self.order.push_back(key);

        self.enforce_bounds();
    }

    fn enforce_bounds(&mut self) {
        while self.order.len() > self.max_entries || self.current_bytes > self.max_bytes {
            if let Some(oldest) = self.order.pop_front() {
                if self.entries.remove(&oldest).is_some() {
                    self.current_bytes = self.current_bytes.saturating_sub(oldest.len());
                }
            } else {
                break;
            }
        }
    }

    #[cfg(test)]
    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.current_bytes = 0;
    }

    #[cfg(test)]
    fn set_limits(&mut self, max_entries: usize, max_bytes: usize) {
        self.max_entries = max_entries.max(1);
        self.max_bytes = max_bytes.max(1);
        self.enforce_bounds();
    }
}

const DEFAULT_CACHE_MAX_ENTRIES: usize = 256;
const DEFAULT_CACHE_MAX_BYTES: usize = 256 * 1024;
const CACHE_ENV_MAX_ENTRIES: &str = "XBERG_ENCODING_CACHE_MAX_ENTRIES";
const CACHE_ENV_MAX_BYTES: &str = "XBERG_ENCODING_CACHE_MAX_BYTES";

fn cache_limits() -> (usize, usize) {
    let max_entries = env::var(CACHE_ENV_MAX_ENTRIES)
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|&v| v > 0)
        .unwrap_or(DEFAULT_CACHE_MAX_ENTRIES);

    let max_bytes = env::var(CACHE_ENV_MAX_BYTES)
        .ok()
        .and_then(|val| val.parse::<usize>().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(DEFAULT_CACHE_MAX_BYTES);

    (max_entries, max_bytes)
}

static ENCODING_CACHE: LazyLock<RwLock<EncodingCache>> = LazyLock::new(|| {
    let (entries, bytes) = cache_limits();
    RwLock::new(EncodingCache::new(entries, bytes))
});

#[inline]
fn chain_replacements<'a>(mut text: Cow<'a, str>, replacements: &[(&Regex, &str)]) -> Cow<'a, str> {
    for (pattern, replacement) in replacements {
        if pattern.is_match(&text) {
            text = Cow::Owned(pattern.replace_all(&text, *replacement).into_owned());
        }
    }
    text
}

fn calculate_cache_key(data: &[u8]) -> String {
    use ahash::AHasher;
    use std::hash::{Hash, Hasher};

    let mut hasher = AHasher::default();
    let sample = if data.len() > 1024 { &data[..1024] } else { data };
    sample.hash(&mut hasher);
    data.len().hash(&mut hasher);
    format!("{:x}", hasher.finish())
}

/// Decode raw bytes into UTF-8, using heuristics and fallback encodings when necessary.
///
/// The function prefers an explicit `encoding`, falls back to the cached guess, probes
/// an encoding detector, and finally tries a small curated list before returning a
/// mojibake-cleaned string.
///
/// Thin wrapper over [`super::decode_with_provenance`] for the many existing callers
/// that only want the text. Callers that need to know whether the decode was lossy
/// should call [`super::decode_with_provenance`] directly (#395): the U+FFFD marker
/// this function's mojibake cleanup used to leave behind is stripped before it gets
/// here, so checking the returned `String` for it can never detect a lossy decode.
pub(crate) fn safe_decode(byte_data: &[u8], encoding: Option<&str>) -> String {
    let outcome = super::decode_with_provenance(byte_data, encoding);
    // Surface the provenance this wrapper otherwise discards so it is not silently
    // lost for every caller that has not migrated to `decode_with_provenance` yet
    // (#395) -- cheap at `trace` level, and the only place in a `safe_decode`-only
    // call chain where `fell_back` / `replaced_characters` are ever inspected.
    tracing::trace!(
        target: "xberg::encoding",
        fell_back = outcome.fell_back,
        replaced_characters = outcome.replaced_characters,
        "safe_decode provenance"
    );
    outcome.text
}

/// Decode raw bytes into UTF-8 like [`safe_decode`], but report fallback/replacement
/// provenance captured at the point each decode actually happens -- before
/// [`fix_mojibake_internal`] can strip the only evidence of a lossy decode (#395).
pub(crate) fn safe_decode_with_provenance(byte_data: &[u8], encoding: Option<&str>) -> DecodeOutcome {
    if byte_data.is_empty() {
        return DecodeOutcome {
            text: String::new(),
            fell_back: false,
            replaced_characters: false,
        };
    }

    if let Some(enc_name) = encoding
        && let Some(enc) = Encoding::for_label(enc_name.as_bytes())
    {
        let (decoded, actual_encoding, had_errors) = enc.decode(byte_data);
        return DecodeOutcome {
            text: fix_mojibake_internal(&decoded).into_owned(),
            fell_back: actual_encoding != encoding_rs::UTF_8,
            replaced_characters: had_errors,
        };
    }

    let cache_key = calculate_cache_key(byte_data);

    // OSError/RuntimeError must bubble up - system errors need user reports ~keep
    match ENCODING_CACHE.write() {
        Ok(mut cache) => {
            if let Some(cached_encoding) = cache.get(&cache_key) {
                let (decoded, actual_encoding, had_errors) = cached_encoding.decode(byte_data);
                return DecodeOutcome {
                    text: fix_mojibake_internal(&decoded).into_owned(),
                    fell_back: actual_encoding != encoding_rs::UTF_8,
                    replaced_characters: had_errors,
                };
            }
        }
        Err(e) => {
            // Lock poisoning should never happen in normal operation ~keep
            tracing::debug!(error = %e, "encoding cache read lock poisoned; continuing without cache");
        }
    }

    let mut detector = EncodingDetector::new(chardetng::Iso2022JpDetection::Allow);
    detector.feed(byte_data, true);
    let guessed_encoding = detector.guess(None, chardetng::Utf8Detection::Allow);

    // OSError/RuntimeError must bubble up - system errors need user reports ~keep
    match ENCODING_CACHE.write() {
        Ok(mut cache) => {
            cache.insert(cache_key, guessed_encoding);
        }
        Err(e) => {
            // Lock poisoning should never happen in normal operation ~keep
            tracing::debug!(error = %e, "encoding cache write lock poisoned; continuing without cache");
        }
    }

    let (decoded, actual_encoding, had_errors) = guessed_encoding.decode(byte_data);

    if had_errors {
        for enc_name in &[
            "windows-1255",
            "iso-8859-8",
            "windows-1256",
            "iso-8859-6",
            "windows-1252",
            "cp1251",
        ] {
            if let Some(enc) = Encoding::for_label(enc_name.as_bytes()) {
                let (test_decoded, test_actual_encoding, test_errors) = enc.decode(byte_data);
                if !test_errors && calculate_text_confidence_internal(&test_decoded) > 0.5 {
                    return DecodeOutcome {
                        text: fix_mojibake_internal(&test_decoded).into_owned(),
                        fell_back: test_actual_encoding != encoding_rs::UTF_8,
                        // Gated on `!test_errors` above, so this is always false --
                        // the candidate is only accepted when it decoded cleanly.
                        replaced_characters: false,
                    };
                }
            }
        }
    }

    let final_text = fix_mojibake_internal(&decoded).into_owned();

    if had_errors {
        let confidence = calculate_text_confidence_internal(&final_text);
        if confidence < 0.6 {
            let preview: String = final_text.chars().filter(|c| !c.is_control()).take(80).collect();

            tracing::debug!(
                target: "xberg::encoding",
                "safe_decode produced low-confidence output after fallback attempts; encoding={}, confidence={:.3}, len={}, preview=\"{}\"",
                guessed_encoding.name(),
                confidence,
                final_text.len(),
                preview
            );
        }
    }

    DecodeOutcome {
        text: final_text,
        fell_back: actual_encoding != encoding_rs::UTF_8,
        replaced_characters: had_errors,
    }
}

fn calculate_text_confidence_internal(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }

    let total_chars = text.len() as f64;

    let replacement_count = REPLACEMENT_CHARS.find_iter(text).count() as f64;
    let control_count = CONTROL_CHARS.find_iter(text).count() as f64;

    let penalty = (replacement_count + control_count * 2.0) / total_chars;

    let readable_chars = text
        .chars()
        .filter(|c| c.is_ascii_graphic() || c.is_whitespace())
        .count() as f64;

    let readability_score = readable_chars / total_chars;

    let cyrillic_matches = HEBREW_AS_CYRILLIC.find_iter(text);
    let cyrillic_length: usize = cyrillic_matches.map(|m| m.len()).sum();

    let mut final_penalty = penalty;
    if cyrillic_length as f64 > total_chars * 0.1 {
        final_penalty += 0.3;
    }

    (readability_score - final_penalty).clamp(0.0, 1.0)
}

fn fix_mojibake_internal(text: &str) -> Cow<'_, str> {
    if text.is_empty() {
        return Cow::Borrowed("");
    }

    let replacements = [
        (&*CONTROL_CHARS, ""),
        (&*REPLACEMENT_CHARS, ""),
        (&*ISOLATED_COMBINING, ""),
    ];

    chain_replacements(Cow::Borrowed(text), &replacements)
}

#[cfg(test)]
mod tests {
    use super::*;
    use encoding_rs::Encoding;

    #[test]
    fn test_safe_decode_empty() {
        assert_eq!(safe_decode(b"", None), "");
    }

    #[test]
    fn test_safe_decode_ascii() {
        let text = b"Hello, World!";
        assert_eq!(safe_decode(text, None), "Hello, World!");
    }

    #[test]
    fn test_safe_decode_utf8() {
        let text = "Hello, 世界! مرحبا".as_bytes();
        assert_eq!(safe_decode(text, None), "Hello, 世界! مرحبا");
    }

    /// #395: `safe_decode` must remain a pure wrapper over
    /// `safe_decode_with_provenance` -- the text it returns must never diverge from
    /// the `text` field of the provenance-carrying call, for any input.
    #[test]
    fn should_return_same_text_as_provenance_wrapper_for_every_input() {
        let cases: &[(&[u8], Option<&str>)] = &[
            (b"", None),
            (b"Hello, World!", None),
            ("Hello, 世界! مرحبا".as_bytes(), None),
            (&[b'A', 0xFF, 0xFE, b'B'], Some("utf-8")),
            (&[b'r', 0xE9, b's', 0xE9], Some("windows-1252")),
        ];

        for (bytes, encoding) in cases {
            assert_eq!(
                safe_decode(bytes, *encoding),
                safe_decode_with_provenance(bytes, *encoding).text,
                "safe_decode diverged from safe_decode_with_provenance for {bytes:?} / {encoding:?}"
            );
        }
    }

    #[test]
    fn should_report_no_fallback_and_no_replacement_for_empty_input() {
        let outcome = safe_decode_with_provenance(b"", None);
        assert_eq!(outcome.text, "");
        assert!(!outcome.fell_back);
        assert!(!outcome.replaced_characters);
    }

    #[test]
    fn should_report_no_fallback_and_no_replacement_for_valid_utf8() {
        let input = "Hello, 世界! مرحبا".as_bytes();
        let outcome = safe_decode_with_provenance(input, None);

        assert_eq!(outcome.text, "Hello, 世界! مرحبا");
        assert!(!outcome.fell_back, "valid UTF-8 must not report a fallback");
        assert!(
            !outcome.replaced_characters,
            "valid UTF-8 must not report a replacement"
        );
    }

    /// windows-1252 maps every byte 0x00-0xFF (WHATWG Encoding Standard), so an
    /// explicit windows-1252 decode can reinterpret bytes but never drop them.
    #[test]
    fn should_report_fallback_without_replacement_for_windows_1252_bytes() {
        let input: &[u8] = &[b'r', 0xE9, b's', b'u', b'm', 0xE9];
        let outcome = safe_decode_with_provenance(input, Some("windows-1252"));

        assert_eq!(outcome.text, "résumé");
        assert!(
            outcome.fell_back,
            "windows-1252 is not UTF-8, so this must report a fallback"
        );
        assert!(
            !outcome.replaced_characters,
            "windows-1252 maps every byte 0x00-0xFF, so no replacement character can occur"
        );
    }

    /// #395: this is the case that used to be undetectable under `quality` -- the
    /// text has its U+FFFD characters stripped by `fix_mojibake_internal`, but
    /// `replaced_characters` must still report the loss because it is captured
    /// before that cleanup runs.
    #[test]
    fn should_report_replacement_when_utf8_decode_forces_it() {
        let input: &[u8] = &[b'A', 0xFF, 0xFE, b'B'];
        let outcome = safe_decode_with_provenance(input, Some("utf-8"));

        assert_eq!(outcome.text, "AB", "fix_mojibake_internal strips the U+FFFD characters");
        assert!(!outcome.fell_back, "UTF-8 was used, so this must not report a fallback");
        assert!(
            outcome.replaced_characters,
            "undecodable bytes must be reported as a replacement"
        );
    }

    #[test]
    fn test_encoding_cache_eviction() {
        let mut cache = ENCODING_CACHE.write().unwrap();
        cache.clear();
        cache.set_limits(4, 64);

        let encoding = Encoding::for_label(b"utf-8").expect("utf-8 encoding should exist");

        for i in 0..8 {
            cache.insert(format!("key{}", i), encoding);
        }

        assert!(cache.entries.len() <= 4);
        assert!(!cache.entries.contains_key("key0"));
        assert!(cache.entries.contains_key("key7"));
    }

    #[test]
    fn test_encoding_cache_byte_limit_eviction() {
        let mut cache = ENCODING_CACHE.write().unwrap();
        cache.clear();
        cache.set_limits(16, 16);

        let encoding = Encoding::for_label(b"utf-8").expect("utf-8 encoding should exist");

        cache.insert("short".to_string(), encoding);
        cache.insert("much-longer-key".to_string(), encoding);

        assert!(cache.entries.contains_key("much-longer-key"));
        assert!(!cache.entries.contains_key("short"));
    }
}
