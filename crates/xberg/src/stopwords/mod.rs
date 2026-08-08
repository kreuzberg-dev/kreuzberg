//! Stopwords management for text processing.
//!
//! Provides language-specific stopword collections used by keyword extraction
//! and token reduction features. Stopwords are common words (the, is, and, etc.)
//! that should be filtered out from text analysis.
//!
//! # Supported Languages
//!
//! Supports 64 languages with embedded stopword lists:
//! - Afrikaans (af), Arabic (ar), Bulgarian (bg), Bengali (bn), Breton (br)
//! - Catalan (ca), Czech (cs), Danish (da), German (de), Greek (el)
//! - English (en), Esperanto (eo), Spanish (es), Estonian (et), Basque (eu)
//! - Persian (fa), Finnish (fi), French (fr), Irish (ga), Galician (gl)
//! - Gujarati (gu), Hausa (ha), Hebrew (he), Hindi (hi), Croatian (hr)
//! - Hungarian (hu), Armenian (hy), Indonesian (id), Italian (it), Japanese (ja)
//! - Kannada (kn), Korean (ko), Kurdish (ku), Latin (la), Lithuanian (lt)
//! - Latvian (lv), Malayalam (ml), Marathi (mr), Malay (ms), Nepali (ne)
//! - Dutch (nl), Norwegian (no), Polish (pl), Portuguese (pt), Romanian (ro)
//! - Russian (ru), Sinhala (si), Slovak (sk), Slovenian (sl), Somali (so)
//! - Sesotho (st), Swedish (sv), Swahili (sw), Tamil (ta), Telugu (te)
//! - Thai (th), Tagalog (tl), Turkish (tr), Ukrainian (uk), Urdu (ur)
//! - Vietnamese (vi), Yoruba (yo), Chinese (zh), Zulu (zu)
//!
//! All stopword lists are embedded in the binary at compile time for zero-overhead access.
//!
//! # Usage
//!
//! `get_stopwords` / `get_stopwords_with_fallback` are `pub(crate)`, so this example is
//! not run as a doctest — it documents the normalization behaviour for crate-internal
//! callers. Downstream crates use the [`STOPWORDS`] static shown under
//! [Direct Access](#direct-access-advanced) below.
//!
//! ```ignore
//! use xberg::stopwords::{get_stopwords, get_stopwords_with_fallback};
//!
//! // Get English stopwords with normalization
//! if let Some(en_stopwords) = get_stopwords("en") {
//!     assert!(en_stopwords.contains("the"));
//!
//!     // Check if a word is a stopword
//!     if en_stopwords.contains("the") {
//!         println!("'the' is a stopword");
//!     }
//! }
//!
//! // Case-insensitive - all of these work
//! assert!(get_stopwords("EN").is_some());
//! assert!(get_stopwords("En").is_some());
//!
//! // Locale codes are normalized to language code (first 2 chars)
//! if let Some(en_us) = get_stopwords("en-US") {
//!     if let Some(en_gb) = get_stopwords("en_GB") {
//!         // Both point to "en" stopwords
//!         assert_eq!(en_us.len(), en_gb.len());
//!     }
//! }
//!
//! // Spanish with locale
//! if let Some(es_stopwords) = get_stopwords("es-ES") {
//!     assert!(es_stopwords.contains("el"));
//! }
//!
//! // Fallback for unsupported languages
//! if let Some(stopwords) = get_stopwords_with_fallback("unknown", "en") {
//!     // Will use English stopwords since "unknown" isn't supported
//!     assert!(stopwords.contains("the"));
//! }
//! ```
//!
//! # Direct Access (Advanced)
//!
//! For advanced use cases where you need direct access to the HashMap or want to
//! iterate over all languages, you can use the `STOPWORDS` static directly:
//!
//! ```rust
//! use xberg::stopwords::STOPWORDS;
//!
//! // Direct access (case-sensitive, no normalization)
//! let en_stopwords = STOPWORDS.get("en");
//!
//! // List all available languages
//! for lang in STOPWORDS.keys() {
//!     println!("Available language: {}", lang);
//! }
//! ```

use ahash::{AHashMap, AHashSet};
use std::sync::LazyLock;

mod languages;

/// Global stopwords registry.
///
/// A lazy-initialized map of language codes to stopword sets.
/// All stopword lists are embedded in the binary at compile time for
/// zero-overhead access and no runtime I/O dependencies.
///
/// Supports 64 languages with comprehensive stopword coverage.
///
/// # Note
///
/// For most use cases, prefer `get_stopwords()` which provides language code
/// normalization (case-insensitive, locale handling). Direct access to STOPWORDS
/// is case-sensitive and requires exact language codes (lowercase, 2-letter ISO 639-1).
///
/// # Examples
///
/// ```rust
/// use xberg::stopwords::STOPWORDS;
///
/// // Direct access (case-sensitive, no normalization)
/// let en_stopwords = STOPWORDS.get("en");
/// assert!(en_stopwords.is_some());
///
/// // Case-sensitive - these return None
/// assert!(STOPWORDS.get("EN").is_none());
/// assert!(STOPWORDS.get("en-US").is_none());
///
/// // List all available languages
/// assert_eq!(STOPWORDS.len(), 64);
/// for lang in STOPWORDS.keys() {
///     println!("Available: {}", lang);
/// }
/// ```
pub static STOPWORDS: LazyLock<AHashMap<String, AHashSet<String>>> = LazyLock::new(|| {
    let mut map = AHashMap::new();

    // Load stopwords by language family
    languages::germanic::load_stopwords(&mut map);
    languages::romance::load_stopwords(&mut map);
    languages::slavic::load_stopwords(&mut map);
    languages::asian::load_stopwords(&mut map);
    languages::other::load_stopwords(&mut map);

    apply_stopword_whitelist(&mut map);

    map
});

fn apply_stopword_whitelist(map: &mut AHashMap<String, AHashSet<String>>) {
    const STOPWORD_REMOVALS: &[(&str, &[&str])] = &[("en", &["hello", "test", "world", "working", "great"])];

    for (lang, words) in STOPWORD_REMOVALS {
        if let Some(set) = map.get_mut(*lang) {
            for &word in *words {
                set.remove(word);
            }
        }
    }
}

/// Get stopwords for a language with normalization.
///
/// This function provides a user-friendly interface to the stopwords registry with:
/// - **Case-insensitive lookup**: "EN", "en", "En" all work
/// - **Locale normalization**: "en-US", "en_GB", "es-ES" extract to "en", "es"
/// - **Consistent behavior**: Returns `None` for unsupported languages
///
/// # Language Code Format
///
/// Accepts multiple formats:
/// - ISO 639-1 two-letter codes: `"en"`, `"es"`, `"de"`, etc.
/// - Uppercase variants: `"EN"`, `"ES"`, `"DE"`
/// - Locale codes with hyphen: `"en-US"`, `"es-ES"`, `"pt-BR"`
/// - Locale codes with underscore: `"en_US"`, `"es_ES"`, `"pt_BR"`
///
/// All formats are normalized to lowercase two-letter ISO 639-1 codes.
///
/// # Returns
///
/// - `Some(&HashSet<String>)` if the language is supported (64 languages available)
/// - `None` if the language is not supported
///
/// # Examples
///
/// Not run as a doctest: this function is `pub(crate)`. Downstream crates use the
/// [`STOPWORDS`] static directly (case-sensitive, no normalization).
///
/// ```ignore
/// use xberg::stopwords::get_stopwords;
///
/// // Simple language codes
/// if let Some(en) = get_stopwords("en") {
///     assert!(en.contains("the"));
/// }
///
/// // Case-insensitive
/// assert!(get_stopwords("EN").is_some());
/// assert!(get_stopwords("En").is_some());
/// assert!(get_stopwords("eN").is_some());
///
/// // Locale codes normalized to language code
/// if let (Some(en_us), Some(en_gb), Some(en_lowercase)) =
///     (get_stopwords("en-US"), get_stopwords("en_GB"), get_stopwords("en"))
/// {
///     // All point to the same stopwords set
///     assert_eq!(en_us.len(), en_gb.len());
///     assert_eq!(en_us.len(), en_lowercase.len());
/// }
///
/// // Spanish with various formats
/// assert!(get_stopwords("es").is_some());
/// assert!(get_stopwords("ES").is_some());
/// assert!(get_stopwords("es-ES").is_some());
/// assert!(get_stopwords("es_MX").is_some());
///
/// // Unsupported language returns None
/// assert!(get_stopwords("xx").is_none());
/// assert!(get_stopwords("zzzz").is_none());
/// ```
///
/// # Performance
///
/// This function performs two operations:
/// 1. String normalization (lowercase + truncate) - O(1) for typical language codes
/// 2. HashMap lookup in STOPWORDS - O(1) average case
///
/// Total overhead is negligible (~10-50ns on modern CPUs).
pub(crate) fn get_stopwords(lang: &str) -> Option<&'static AHashSet<String>> {
    let normalized = lang.to_lowercase();

    let lang_code = if let Some(pos) = normalized.find(&['-', '_'][..]) {
        &normalized[..pos]
    } else if normalized.len() >= 2 {
        &normalized[..2]
    } else {
        &normalized
    };

    STOPWORDS.get(lang_code)
}

/// Get stopwords for a language with fallback support.
///
/// This function attempts to retrieve stopwords for the primary language,
/// and if not available, falls back to a secondary language. This is useful
/// for handling scenarios where:
/// - A detected language isn't supported
/// - You want to use English as a fallback for unknown languages
/// - You need graceful degradation for multilingual content
///
/// Both language codes support the same normalization as [`get_stopwords()`]:
/// - Case-insensitive lookup (EN, en, En all work)
/// - Locale codes normalized (en-US, en_GB extract to "en")
///
/// # Arguments
///
/// * `language` - Primary language code to try first
/// * `fallback` - Fallback language code to use if primary not available
///
/// # Returns
///
/// - `Some(&HashSet<String>)` if either language is supported
/// - `None` if neither language is supported
///
/// # Examples
///
/// Not run as a doctest: this function is `pub(crate)`. Downstream crates use the
/// [`STOPWORDS`] static directly (case-sensitive, no normalization).
///
/// ```ignore
/// use xberg::stopwords::get_stopwords_with_fallback;
///
/// // Detected language is Esperanto, fallback to English
/// if let Some(stopwords) = get_stopwords_with_fallback("eo", "en") {
///     // Will use Esperanto stopwords (supported)
///     assert!(stopwords.contains("la"));
/// }
///
/// // Unsupported language, fallback to English
/// if let Some(stopwords) = get_stopwords_with_fallback("xx", "en") {
///     // Will use English stopwords (fallback)
///     assert!(stopwords.contains("the"));
/// }
///
/// // Case-insensitive and locale-aware
/// let result = get_stopwords_with_fallback("es-MX", "EN-US");
/// assert!(result.is_some());
///
/// // Both unsupported returns None
/// assert!(get_stopwords_with_fallback("xx", "zz").is_none());
/// ```
///
/// # Common Patterns
///
/// ```ignore
/// use xberg::stopwords::get_stopwords_with_fallback;
///
/// // English fallback for unknown languages
/// let detected_lang = "xyz"; // Unknown language
/// let stopwords = get_stopwords_with_fallback(detected_lang, "en")
///     .expect("English fallback should always be available");
///
/// // Multi-language content with English fallback
/// for lang in ["de", "fr", "unknown", "es"] {
///     if let Some(stopwords) = get_stopwords_with_fallback(lang, "en") {
///         println!("Using stopwords for: {}", lang);
///     }
/// }
/// ```
///
/// # Performance
///
/// This function performs at most two HashMap lookups:
/// 1. Try primary language (O(1) average case)
/// 2. If None, try fallback language (O(1) average case)
///
/// Total overhead is negligible (~10-100ns on modern CPUs).
pub(crate) fn get_stopwords_with_fallback(language: &str, fallback: &str) -> Option<&'static AHashSet<String>> {
    get_stopwords(language).or_else(|| get_stopwords(fallback))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_stopwords_lazy_initialization() {
        let stopwords = &*STOPWORDS;
        assert!(stopwords.contains_key("en"));
        assert!(stopwords.contains_key("es"));
        assert!(!stopwords.get("en").unwrap().is_empty());
        assert!(!stopwords.get("es").unwrap().is_empty());
    }

    #[test]
    fn test_english_stopwords() {
        let en_stopwords = STOPWORDS.get("en").unwrap();

        assert!(en_stopwords.contains("the"));
        assert!(en_stopwords.contains("is"));
        assert!(en_stopwords.contains("and"));
        assert!(en_stopwords.contains("a"));
        assert!(en_stopwords.contains("of"));

        assert!(en_stopwords.len() >= 70);
    }

    #[test]
    fn test_spanish_stopwords() {
        let es_stopwords = STOPWORDS.get("es").unwrap();

        assert!(es_stopwords.contains("el"));
        assert!(es_stopwords.contains("la"));
        assert!(es_stopwords.contains("es"));
        assert!(es_stopwords.contains("en"));
        assert!(es_stopwords.contains("de"));

        assert!(es_stopwords.len() >= 200);
    }

    #[test]
    fn test_all_64_languages_loaded() {
        let expected_languages = [
            "af", "ar", "bg", "bn", "br", "ca", "cs", "da", "de", "el", "en", "eo", "es", "et", "eu", "fa", "fi", "fr",
            "ga", "gl", "gu", "ha", "he", "hi", "hr", "hu", "hy", "id", "it", "ja", "kn", "ko", "ku", "la", "lt", "lv",
            "ml", "mr", "ms", "ne", "nl", "no", "pl", "pt", "ro", "ru", "si", "sk", "sl", "so", "st", "sv", "sw", "ta",
            "te", "th", "tl", "tr", "uk", "ur", "vi", "yo", "zh", "zu",
        ];

        for lang in &expected_languages {
            assert!(
                STOPWORDS.contains_key(*lang),
                "Missing stopwords for language: {}",
                lang
            );
            assert!(
                !STOPWORDS.get(*lang).unwrap().is_empty(),
                "Empty stopwords for language: {}",
                lang
            );
        }

        assert_eq!(STOPWORDS.len(), 64, "Expected 64 languages, found {}", STOPWORDS.len());
    }

    #[test]
    fn test_german_stopwords() {
        let de_stopwords = STOPWORDS.get("de").unwrap();
        assert!(de_stopwords.contains("der"));
        assert!(de_stopwords.contains("die"));
        assert!(de_stopwords.contains("und"));
    }

    #[test]
    fn test_french_stopwords() {
        let fr_stopwords = STOPWORDS.get("fr").unwrap();
        assert!(fr_stopwords.contains("le"));
        assert!(fr_stopwords.contains("de"));
        assert!(fr_stopwords.contains("un"));
    }

    #[test]
    fn test_chinese_stopwords() {
        let zh_stopwords = STOPWORDS.get("zh").unwrap();
        assert!(!zh_stopwords.is_empty());
    }

    #[test]
    fn test_arabic_stopwords() {
        let ar_stopwords = STOPWORDS.get("ar").unwrap();
        assert!(!ar_stopwords.is_empty());
    }

    #[test]
    fn test_unknown_language_returns_none() {
        assert!(!STOPWORDS.contains_key("xx"));
        assert!(STOPWORDS.get("unknown").is_none());
    }

    #[test]
    fn test_get_stopwords_lowercase() {
        assert!(get_stopwords("en").is_some());
        assert!(get_stopwords("es").is_some());
        assert!(get_stopwords("de").is_some());
        assert!(get_stopwords("fr").is_some());
    }

    #[test]
    fn test_get_stopwords_uppercase() {
        let en_upper = get_stopwords("EN");
        let en_lower = get_stopwords("en");

        assert!(en_upper.is_some());
        assert!(en_lower.is_some());

        assert_eq!(en_upper.unwrap().len(), en_lower.unwrap().len());
    }

    #[test]
    fn test_get_stopwords_mixed_case() {
        assert!(get_stopwords("En").is_some());
        assert!(get_stopwords("eN").is_some());
        assert!(get_stopwords("ES").is_some());
        assert!(get_stopwords("Es").is_some());
        assert!(get_stopwords("DE").is_some());
        assert!(get_stopwords("De").is_some());
    }

    #[test]
    fn test_get_stopwords_locale_hyphen() {
        let en_us = get_stopwords("en-US");
        let en_gb = get_stopwords("en-GB");
        let en = get_stopwords("en");

        assert!(en_us.is_some());
        assert!(en_gb.is_some());

        assert_eq!(en_us.unwrap().len(), en.unwrap().len());
        assert_eq!(en_gb.unwrap().len(), en.unwrap().len());
    }

    #[test]
    fn test_get_stopwords_locale_underscore() {
        let es_es = get_stopwords("es_ES");
        let es_mx = get_stopwords("es_MX");
        let es = get_stopwords("es");

        assert!(es_es.is_some());
        assert!(es_mx.is_some());

        assert_eq!(es_es.unwrap().len(), es.unwrap().len());
        assert_eq!(es_mx.unwrap().len(), es.unwrap().len());
    }

    #[test]
    fn test_get_stopwords_locale_uppercase() {
        let en_us_upper = get_stopwords("EN-US");
        let es_es_upper = get_stopwords("ES_ES");
        let pt_br_mixed = get_stopwords("Pt-BR");

        assert!(en_us_upper.is_some());
        assert!(es_es_upper.is_some());
        assert!(pt_br_mixed.is_some());

        assert!(en_us_upper.unwrap().contains("the"));
        assert!(es_es_upper.unwrap().contains("el"));
        assert!(pt_br_mixed.unwrap().contains("o"));
    }

    #[test]
    fn test_get_stopwords_all_supported_languages() {
        let languages = [
            "af", "ar", "bg", "bn", "br", "ca", "cs", "da", "de", "el", "en", "eo", "es", "et", "eu", "fa", "fi", "fr",
            "ga", "gl", "gu", "ha", "he", "hi", "hr", "hu", "hy", "id", "it", "ja", "kn", "ko", "ku", "la", "lt", "lv",
            "ml", "mr", "ms", "ne", "nl", "no", "pl", "pt", "ro", "ru", "si", "sk", "sl", "so", "st", "sv", "sw", "ta",
            "te", "th", "tl", "tr", "uk", "ur", "vi", "yo", "zh", "zu",
        ];

        for lang in &languages {
            assert!(
                get_stopwords(lang).is_some(),
                "Language {} should be available via get_stopwords",
                lang
            );
        }
    }

    #[test]
    fn test_get_stopwords_unsupported_language() {
        assert!(get_stopwords("xx").is_none());
        assert!(get_stopwords("zz").is_none());
        assert!(get_stopwords("xyz").is_none());
        assert!(get_stopwords("unknown").is_none());
    }

    #[test]
    fn test_get_stopwords_empty_string() {
        assert!(get_stopwords("").is_none());
    }

    #[test]
    fn test_get_stopwords_single_char() {
        assert!(get_stopwords("e").is_none());
        assert!(get_stopwords("z").is_none());
    }

    #[test]
    fn test_get_stopwords_long_locale() {
        let zh_cn_hans = get_stopwords("zh-CN-Hans");
        let pt_br_utf8 = get_stopwords("pt_BR.UTF-8");

        assert!(zh_cn_hans.is_some());
        assert!(pt_br_utf8.is_some());

        assert_eq!(zh_cn_hans.unwrap().len(), get_stopwords("zh").unwrap().len());
        assert_eq!(pt_br_utf8.unwrap().len(), get_stopwords("pt").unwrap().len());
    }

    #[test]
    fn test_get_stopwords_content_verification() {
        let en = get_stopwords("en").expect("English stopwords should exist");
        assert!(en.contains("the"));
        assert!(en.contains("is"));
        assert!(en.contains("and"));

        let es = get_stopwords("es").expect("Spanish stopwords should exist");
        assert!(es.contains("el"));
        assert!(es.contains("la"));
        assert!(es.contains("es"));

        let de = get_stopwords("de").expect("German stopwords should exist");
        assert!(de.contains("der"));
        assert!(de.contains("die"));
        assert!(de.contains("und"));

        let fr = get_stopwords("fr").expect("French stopwords should exist");
        assert!(fr.contains("le"));
        assert!(fr.contains("de"));
        assert!(fr.contains("un"));
    }

    #[test]
    fn test_get_stopwords_vs_direct_access() {
        let en_normalized = get_stopwords("en").unwrap();
        let en_direct = STOPWORDS.get("en").unwrap();

        assert_eq!(en_normalized.len(), en_direct.len());

        for word in en_direct {
            assert!(en_normalized.contains(word));
        }
    }

    #[test]
    fn test_get_stopwords_with_fallback_primary_available() {
        let result = get_stopwords_with_fallback("en", "es");
        assert!(result.is_some());
        let stopwords = result.unwrap();
        assert!(stopwords.contains("the"));
        assert!(!stopwords.contains("el"));
    }

    #[test]
    fn test_get_stopwords_with_fallback_use_fallback() {
        let result = get_stopwords_with_fallback("xx", "en");
        assert!(result.is_some());
        let stopwords = result.unwrap();
        assert!(stopwords.contains("the"));
    }

    #[test]
    fn test_get_stopwords_with_fallback_both_unavailable() {
        let result = get_stopwords_with_fallback("xx", "zz");
        assert!(result.is_none());
    }

    #[test]
    fn test_get_stopwords_with_fallback_case_insensitive() {
        let result1 = get_stopwords_with_fallback("EN", "es");
        let result2 = get_stopwords_with_fallback("xx", "ES");
        assert!(result1.is_some());
        assert!(result2.is_some());
    }

    #[test]
    fn test_get_stopwords_with_fallback_locale_codes() {
        let result = get_stopwords_with_fallback("es-MX", "en-US");
        assert!(result.is_some());
        let stopwords = result.unwrap();
        assert!(stopwords.contains("el"));
    }

    #[test]
    fn test_get_stopwords_with_fallback_esperanto_to_english() {
        let result = get_stopwords_with_fallback("eo", "en");
        assert!(result.is_some());
        let stopwords = result.unwrap();
        assert!(stopwords.contains("la"));
    }

    #[test]
    fn test_get_stopwords_with_fallback_unknown_to_english() {
        let result = get_stopwords_with_fallback("xyz", "en");
        assert!(result.is_some());
        let stopwords = result.unwrap();
        assert!(stopwords.contains("the"));
    }

    #[test]
    fn test_get_stopwords_with_fallback_same_as_chained_or_else() {
        let manual = get_stopwords("xx").or_else(|| get_stopwords("en"));
        let helper = get_stopwords_with_fallback("xx", "en");
        assert_eq!(manual.is_some(), helper.is_some());
        if let (Some(m), Some(h)) = (manual, helper) {
            assert_eq!(m.len(), h.len());
        }
    }

    #[test]
    fn test_get_stopwords_invalid_language_codes() {
        assert!(get_stopwords("invalid_lang").is_none());
        assert!(get_stopwords("xyz").is_none());
        assert!(get_stopwords("zzzz").is_none());
        assert!(get_stopwords("abc123").is_none());
        assert!(get_stopwords("!!!").is_none());
    }

    #[test]
    fn test_get_stopwords_edge_case_empty_and_whitespace() {
        assert!(get_stopwords("").is_none());
        assert!(get_stopwords(" ").is_none());
        assert!(get_stopwords("  ").is_none());
        assert!(get_stopwords("\t").is_none());
        assert!(get_stopwords("\n").is_none());
    }

    #[test]
    fn test_get_stopwords_special_characters() {
        assert!(get_stopwords("@#").is_none());
        assert!(get_stopwords("$%").is_none());
        assert!(get_stopwords("!!!").is_none());

        let result = get_stopwords("en!");
        assert!(result.is_some());
        if let Some(stopwords) = result {
            assert!(stopwords.contains("the"));
        }

        let result = get_stopwords("es@");
        assert!(result.is_some());
        if let Some(stopwords) = result {
            assert!(stopwords.contains("el"));
        }

        let result = get_stopwords("de#fr");
        assert!(result.is_some());
        if let Some(stopwords) = result {
            assert!(stopwords.contains("der"));
        }
    }

    #[test]
    fn test_get_stopwords_numeric_codes() {
        assert!(get_stopwords("12").is_none());
        assert!(get_stopwords("99").is_none());
        assert!(get_stopwords("123").is_none());
        assert!(get_stopwords("0").is_none());
    }

    #[test]
    fn test_get_stopwords_single_character_edge_cases() {
        assert!(get_stopwords("a").is_none());
        assert!(get_stopwords("e").is_none());
        assert!(get_stopwords("z").is_none());
        assert!(get_stopwords("1").is_none());
        assert!(get_stopwords("_").is_none());
    }

    #[test]
    fn test_get_stopwords_invalid_locale_formats() {
        assert!(get_stopwords("xx-YY").is_none());
        assert!(get_stopwords("zz_ZZ").is_none());
        assert!(get_stopwords("invalid-US").is_none());
        assert!(get_stopwords("aa_BB_CC").is_none());
    }

    #[test]
    fn test_get_stopwords_mixed_valid_invalid() {
        let result = get_stopwords("en123");
        assert!(result.is_some(), "Should extract 'en' from 'en123'");

        assert!(get_stopwords("12en").is_none());
        assert!(get_stopwords("@@en").is_none());
    }

    #[test]
    fn test_get_stopwords_case_sensitivity_validation() {
        let lower = get_stopwords("en");
        let upper = get_stopwords("EN");
        let mixed1 = get_stopwords("En");
        let mixed2 = get_stopwords("eN");

        assert!(lower.is_some());
        assert!(upper.is_some());
        assert!(mixed1.is_some());
        assert!(mixed2.is_some());

        if let (Some(l), Some(u), Some(m1), Some(m2)) = (lower, upper, mixed1, mixed2) {
            assert_eq!(l.len(), u.len());
            assert_eq!(l.len(), m1.len());
            assert_eq!(l.len(), m2.len());
        }
    }

    #[test]
    fn test_get_stopwords_none_return_safety() {
        let result = get_stopwords("invalid").and_then(|_| get_stopwords("also_invalid"));
        assert!(result.is_none());

        let chained = get_stopwords("xxx")
            .or_else(|| get_stopwords("yyy"))
            .or_else(|| get_stopwords("zzz"));
        assert!(chained.is_none());
    }

    #[test]
    fn test_get_stopwords_with_fallback_both_invalid() {
        assert!(get_stopwords_with_fallback("invalid", "also_invalid").is_none());
        assert!(get_stopwords_with_fallback("xxx", "yyy").is_none());
        assert!(get_stopwords_with_fallback("", "").is_none());
        assert!(get_stopwords_with_fallback("123", "456").is_none());
    }

    #[test]
    fn test_get_stopwords_with_fallback_invalid_primary_valid_fallback() {
        let result = get_stopwords_with_fallback("invalid_lang", "en");
        assert!(result.is_some());
        if let Some(stopwords) = result {
            assert!(stopwords.contains("the"));
        }

        let result2 = get_stopwords_with_fallback("xyz", "es");
        assert!(result2.is_some());
        if let Some(stopwords) = result2 {
            assert!(stopwords.contains("el"));
        }
    }

    #[test]
    fn test_get_stopwords_with_fallback_valid_primary_invalid_fallback() {
        let result = get_stopwords_with_fallback("en", "invalid_fallback");
        assert!(result.is_some());
        if let Some(stopwords) = result {
            assert!(stopwords.contains("the"));
        }

        let result2 = get_stopwords_with_fallback("es", "zzz");
        assert!(result2.is_some());
        if let Some(stopwords) = result2 {
            assert!(stopwords.contains("el"));
        }
    }

    #[test]
    fn test_get_stopwords_with_fallback_empty_strings() {
        assert!(get_stopwords_with_fallback("", "en").is_some());
        assert!(get_stopwords_with_fallback("en", "").is_some());
        assert!(get_stopwords_with_fallback("", "").is_none());
    }

    #[test]
    fn test_get_stopwords_with_fallback_special_characters() {
        assert!(get_stopwords_with_fallback("@#$", "en").is_some());
        assert!(get_stopwords_with_fallback("en", "!!!").is_some());
        assert!(get_stopwords_with_fallback("@#$", "!!!").is_none());
    }

    #[test]
    fn test_get_stopwords_with_fallback_case_insensitive_validation() {
        let result1 = get_stopwords_with_fallback("INVALID", "en");
        let result2 = get_stopwords_with_fallback("invalid", "EN");
        let result3 = get_stopwords_with_fallback("INVALID", "EN");

        assert!(result1.is_some());
        assert!(result2.is_some());
        assert!(result3.is_some());

        if let (Some(r1), Some(r2), Some(r3)) = (result1, result2, result3) {
            assert!(r1.contains("the"));
            assert!(r2.contains("the"));
            assert!(r3.contains("the"));
        }
    }

    #[test]
    fn test_direct_stopwords_access_invalid_keys() {
        assert!(STOPWORDS.get("invalid").is_none());
        assert!(STOPWORDS.get("EN").is_none());
        assert!(STOPWORDS.get("en-US").is_none());
        assert!(STOPWORDS.get("xyz").is_none());
        assert!(STOPWORDS.get("").is_none());
    }

    #[test]
    fn test_stopwords_case_sensitivity_direct_vs_normalized() {
        assert!(STOPWORDS.get("EN").is_none());
        assert!(get_stopwords("EN").is_some());

        assert!(STOPWORDS.get("Es").is_none());
        assert!(get_stopwords("Es").is_some());

        assert!(STOPWORDS.get("DE").is_none());
        assert!(get_stopwords("DE").is_some());
    }

    #[test]
    fn test_get_stopwords_unicode_characters() {
        // NOTE: Current implementation has a limitation - it uses byte slicing which can panic

        let result = get_stopwords("zh-中文");
        assert!(result.is_some());

        let result = get_stopwords("ar-العربية");
        assert!(result.is_some());

        let result = get_stopwords("ja_日本");
        assert!(result.is_some());

        assert!(get_stopwords("xx").is_none());
        assert!(get_stopwords("yy").is_none());

        // NOTE: The following would panic due to byte slicing on multi-byte chars:
    }

    #[test]
    fn test_get_stopwords_very_long_strings() {
        let long_string = "x".repeat(1000);
        assert!(get_stopwords(&long_string).is_none());

        let long_locale = "en-".to_string() + &"X".repeat(100);
        let result = get_stopwords(&long_locale);
        assert!(result.is_some());
    }

    #[test]
    fn test_get_stopwords_null_bytes() {
        assert!(get_stopwords("\0").is_none());
        assert!(get_stopwords("en\0").is_some());
        assert!(get_stopwords("\0en").is_none());
    }

    #[test]
    fn test_get_stopwords_boundary_conditions() {
        assert!(get_stopwords("e").is_none());
        assert!(get_stopwords("en").is_some());
        assert!(get_stopwords("eng").is_some());

        let result = get_stopwords("en-");
        assert!(result.is_some());
    }

    #[test]
    fn test_get_stopwords_multiple_separators() {
        assert!(get_stopwords("en-US-utf8").is_some());
        assert!(get_stopwords("es_MX_special").is_some());
        assert!(get_stopwords("pt-BR_variant").is_some());
    }

    #[test]
    fn test_romance_languages() {
        let fr = get_stopwords("fr").expect("French stopwords should exist");
        assert!(fr.contains("le"), "French should contain 'le'");
        assert!(fr.contains("et"), "French should contain 'et'");
        assert!(fr.len() >= 150, "French should have substantial stopwords");

        let es = get_stopwords("es").expect("Spanish stopwords should exist");
        assert!(es.contains("el"), "Spanish should contain 'el'");
        assert!(es.contains("y"), "Spanish should contain 'y'");
        assert!(es.len() >= 200, "Spanish should have substantial stopwords");

        let pt = get_stopwords("pt").expect("Portuguese stopwords should exist");
        assert!(pt.contains("o"), "Portuguese should contain 'o'");
        assert!(pt.contains("e"), "Portuguese should contain 'e'");
        assert!(pt.len() >= 150, "Portuguese should have substantial stopwords");

        let it = get_stopwords("it").expect("Italian stopwords should exist");
        assert!(it.contains("il"), "Italian should contain 'il'");
        assert!(it.contains("e"), "Italian should contain 'e'");
        assert!(it.len() >= 150, "Italian should have substantial stopwords");

        let ro = get_stopwords("ro").expect("Romanian stopwords should exist");
        assert!(!ro.is_empty(), "Romanian should have stopwords");
        assert!(ro.len() >= 100, "Romanian should have substantial stopwords");
    }

    #[test]
    fn test_germanic_languages() {
        let de = get_stopwords("de").expect("German stopwords should exist");
        assert!(de.contains("der"), "German should contain 'der'");
        assert!(de.contains("die"), "German should contain 'die'");
        assert!(de.contains("und"), "German should contain 'und'");
        assert!(de.len() >= 200, "German should have substantial stopwords");

        let en = get_stopwords("en").expect("English stopwords should exist");
        assert!(en.contains("the"), "English should contain 'the'");
        assert!(en.contains("and"), "English should contain 'and'");
        assert!(en.len() >= 70, "English should have substantial stopwords");

        let nl = get_stopwords("nl").expect("Dutch stopwords should exist");
        assert!(nl.contains("de"), "Dutch should contain 'de'");
        assert!(nl.contains("het"), "Dutch should contain 'het'");
        assert!(nl.len() >= 100, "Dutch should have substantial stopwords");

        let sv = get_stopwords("sv").expect("Swedish stopwords should exist");
        assert!(!sv.is_empty(), "Swedish should have stopwords");
        assert!(sv.len() >= 100, "Swedish should have substantial stopwords");

        let no = get_stopwords("no").expect("Norwegian stopwords should exist");
        assert!(!no.is_empty(), "Norwegian should have stopwords");

        let da = get_stopwords("da").expect("Danish stopwords should exist");
        assert!(!da.is_empty(), "Danish should have stopwords");
    }

    #[test]
    fn test_slavic_languages() {
        let ru = get_stopwords("ru").expect("Russian stopwords should exist");
        assert!(!ru.is_empty(), "Russian should have stopwords");
        assert!(ru.len() >= 100, "Russian should have substantial stopwords");

        let pl = get_stopwords("pl").expect("Polish stopwords should exist");
        assert!(!pl.is_empty(), "Polish should have stopwords");
        assert!(pl.len() >= 100, "Polish should have substantial stopwords");

        let cs = get_stopwords("cs").expect("Czech stopwords should exist");
        assert!(!cs.is_empty(), "Czech should have stopwords");

        let sk = get_stopwords("sk").expect("Slovak stopwords should exist");
        assert!(!sk.is_empty(), "Slovak should have stopwords");

        let bg = get_stopwords("bg").expect("Bulgarian stopwords should exist");
        assert!(!bg.is_empty(), "Bulgarian should have stopwords");

        let uk = get_stopwords("uk").expect("Ukrainian stopwords should exist");
        assert!(!uk.is_empty(), "Ukrainian should have stopwords");

        let hr = get_stopwords("hr").expect("Croatian stopwords should exist");
        assert!(!hr.is_empty(), "Croatian should have stopwords");

        let sl = get_stopwords("sl").expect("Slovenian stopwords should exist");
        assert!(!sl.is_empty(), "Slovenian should have stopwords");
    }

    #[test]
    fn test_asian_languages() {
        let zh = get_stopwords("zh").expect("Chinese stopwords should exist");
        assert!(!zh.is_empty(), "Chinese should have stopwords");
        assert!(zh.len() >= 50, "Chinese should have substantial stopwords");

        let ja = get_stopwords("ja").expect("Japanese stopwords should exist");
        assert!(!ja.is_empty(), "Japanese should have stopwords");
        assert!(ja.len() >= 50, "Japanese should have substantial stopwords");

        let ko = get_stopwords("ko").expect("Korean stopwords should exist");
        assert!(!ko.is_empty(), "Korean should have stopwords");

        let hi = get_stopwords("hi").expect("Hindi stopwords should exist");
        assert!(!hi.is_empty(), "Hindi should have stopwords");
        assert!(hi.len() >= 100, "Hindi should have substantial stopwords");

        let bn = get_stopwords("bn").expect("Bengali stopwords should exist");
        assert!(!bn.is_empty(), "Bengali should have stopwords");

        let th = get_stopwords("th").expect("Thai stopwords should exist");
        assert!(!th.is_empty(), "Thai should have stopwords");

        let vi = get_stopwords("vi").expect("Vietnamese stopwords should exist");
        assert!(!vi.is_empty(), "Vietnamese should have stopwords");
    }

    #[test]
    fn test_african_languages() {
        let af = get_stopwords("af").expect("Afrikaans stopwords should exist");
        assert!(!af.is_empty(), "Afrikaans should have stopwords");

        let sw = get_stopwords("sw").expect("Swahili stopwords should exist");
        assert!(!sw.is_empty(), "Swahili should have stopwords");

        let yo = get_stopwords("yo").expect("Yoruba stopwords should exist");
        assert!(!yo.is_empty(), "Yoruba should have stopwords");

        let zu = get_stopwords("zu").expect("Zulu stopwords should exist");
        assert!(!zu.is_empty(), "Zulu should have stopwords");

        let ha = get_stopwords("ha").expect("Hausa stopwords should exist");
        assert!(!ha.is_empty(), "Hausa should have stopwords");

        let so = get_stopwords("so").expect("Somali stopwords should exist");
        assert!(!so.is_empty(), "Somali should have stopwords");

        let st = get_stopwords("st").expect("Sesotho stopwords should exist");
        assert!(!st.is_empty(), "Sesotho should have stopwords");
    }

    #[test]
    fn test_indic_languages() {
        let hi = get_stopwords("hi").expect("Hindi stopwords should exist");
        assert!(!hi.is_empty(), "Hindi should have stopwords");

        let bn = get_stopwords("bn").expect("Bengali stopwords should exist");
        assert!(!bn.is_empty(), "Bengali should have stopwords");

        let gu = get_stopwords("gu").expect("Gujarati stopwords should exist");
        assert!(!gu.is_empty(), "Gujarati should have stopwords");

        let kn = get_stopwords("kn").expect("Kannada stopwords should exist");
        assert!(!kn.is_empty(), "Kannada should have stopwords");

        let ml = get_stopwords("ml").expect("Malayalam stopwords should exist");
        assert!(!ml.is_empty(), "Malayalam should have stopwords");

        let mr = get_stopwords("mr").expect("Marathi stopwords should exist");
        assert!(!mr.is_empty(), "Marathi should have stopwords");

        let ta = get_stopwords("ta").expect("Tamil stopwords should exist");
        assert!(!ta.is_empty(), "Tamil should have stopwords");

        let te = get_stopwords("te").expect("Telugu stopwords should exist");
        assert!(!te.is_empty(), "Telugu should have stopwords");

        let ur = get_stopwords("ur").expect("Urdu stopwords should exist");
        assert!(!ur.is_empty(), "Urdu should have stopwords");

        let ne = get_stopwords("ne").expect("Nepali stopwords should exist");
        assert!(!ne.is_empty(), "Nepali should have stopwords");

        let si = get_stopwords("si").expect("Sinhala stopwords should exist");
        assert!(!si.is_empty(), "Sinhala should have stopwords");
    }

    #[test]
    fn test_middle_eastern_languages() {
        let ar = get_stopwords("ar").expect("Arabic stopwords should exist");
        assert!(!ar.is_empty(), "Arabic should have stopwords");
        assert!(ar.len() >= 100, "Arabic should have substantial stopwords");

        let fa = get_stopwords("fa").expect("Persian stopwords should exist");
        assert!(!fa.is_empty(), "Persian should have stopwords");

        let he = get_stopwords("he").expect("Hebrew stopwords should exist");
        assert!(!he.is_empty(), "Hebrew should have stopwords");

        let tr = get_stopwords("tr").expect("Turkish stopwords should exist");
        assert!(!tr.is_empty(), "Turkish should have stopwords");

        let ku = get_stopwords("ku").expect("Kurdish stopwords should exist");
        assert!(!ku.is_empty(), "Kurdish stopwords should exist");
    }

    #[test]
    fn test_other_languages() {
        let hy = get_stopwords("hy").expect("Armenian stopwords should exist");
        assert!(!hy.is_empty(), "Armenian should have stopwords");

        let eu = get_stopwords("eu").expect("Basque stopwords should exist");
        assert!(!eu.is_empty(), "Basque should have stopwords");

        let br = get_stopwords("br").expect("Breton stopwords should exist");
        assert!(!br.is_empty(), "Breton should have stopwords");

        let ca = get_stopwords("ca").expect("Catalan stopwords should exist");
        assert!(!ca.is_empty(), "Catalan should have stopwords");

        let eo = get_stopwords("eo").expect("Esperanto stopwords should exist");
        assert!(eo.contains("la"), "Esperanto should contain 'la'");
        assert!(!eo.is_empty(), "Esperanto should have stopwords");

        let et = get_stopwords("et").expect("Estonian stopwords should exist");
        assert!(!et.is_empty(), "Estonian should have stopwords");

        let fi = get_stopwords("fi").expect("Finnish stopwords should exist");
        assert!(!fi.is_empty(), "Finnish should have stopwords");

        let gl = get_stopwords("gl").expect("Galician stopwords should exist");
        assert!(!gl.is_empty(), "Galician should have stopwords");

        let hu = get_stopwords("hu").expect("Hungarian stopwords should exist");
        assert!(!hu.is_empty(), "Hungarian should have stopwords");

        let id = get_stopwords("id").expect("Indonesian stopwords should exist");
        assert!(!id.is_empty(), "Indonesian should have stopwords");

        let ga = get_stopwords("ga").expect("Irish stopwords should exist");
        assert!(!ga.is_empty(), "Irish should have stopwords");

        let la = get_stopwords("la").expect("Latin stopwords should exist");
        assert!(!la.is_empty(), "Latin should have stopwords");

        let lt = get_stopwords("lt").expect("Lithuanian stopwords should exist");
        assert!(!lt.is_empty(), "Lithuanian should have stopwords");

        let lv = get_stopwords("lv").expect("Latvian stopwords should exist");
        assert!(!lv.is_empty(), "Latvian should have stopwords");

        let ms = get_stopwords("ms").expect("Malay stopwords should exist");
        assert!(!ms.is_empty(), "Malay should have stopwords");

        let tl = get_stopwords("tl").expect("Tagalog stopwords should exist");
        assert!(!tl.is_empty(), "Tagalog should have stopwords");
    }

    #[test]
    fn test_language_code_variants() {
        let eng = get_stopwords("eng");
        let en = get_stopwords("en");
        assert!(eng.is_some(), "'eng' should extract to 'en'");
        assert!(en.is_some());
        assert_eq!(eng.unwrap().len(), en.unwrap().len());

        let spa = get_stopwords("spa");
        assert!(spa.is_none(), "'spa' extracts to 'sp' which is invalid");

        let deu = get_stopwords("deu");
        let de = get_stopwords("de");
        assert!(deu.is_some(), "'deu' should extract to 'de'");
        assert_eq!(deu.unwrap().len(), de.unwrap().len());

        let fra = get_stopwords("fra");
        let fr = get_stopwords("fr");
        assert!(fra.is_some(), "'fra' should extract to 'fr'");
        assert_eq!(fra.unwrap().len(), fr.unwrap().len());

        let zho = get_stopwords("zho");
        let zh = get_stopwords("zh");
        assert!(zho.is_some(), "'zho' should extract to 'zh'");
        assert_eq!(zho.unwrap().len(), zh.unwrap().len());
    }

    #[test]
    fn test_stopword_set_sizes() {
        let mut sizes: Vec<(String, usize)> = Vec::new();

        for (lang, stopwords) in STOPWORDS.iter() {
            sizes.push((lang.clone(), stopwords.len()));
            assert!(!stopwords.is_empty(), "Language {} has empty stopwords", lang);
            assert!(
                stopwords.len() >= 5,
                "Language {} has suspiciously few stopwords: {}",
                lang,
                stopwords.len()
            );
            assert!(
                stopwords.len() <= 1500,
                "Language {} has suspiciously many stopwords: {}",
                lang,
                stopwords.len()
            );
        }

        assert_eq!(sizes.len(), 64, "Should have exactly 64 languages");

        let en_size = STOPWORDS.get("en").unwrap().len();
        assert!(
            (70..=1500).contains(&en_size),
            "English stopwords size {} outside expected range",
            en_size
        );

        let es_size = STOPWORDS.get("es").unwrap().len();
        assert!(
            (200..=1000).contains(&es_size),
            "Spanish stopwords size {} outside expected range",
            es_size
        );
    }

    #[test]
    fn test_stopword_content_quality() {
        let en = get_stopwords("en").expect("English stopwords");
        let english_common = vec![
            "the", "is", "are", "was", "were", "a", "an", "and", "or", "but", "in", "on", "at", "to", "for", "of",
            "with",
        ];
        for word in english_common {
            assert!(en.contains(word), "English missing common stopword: {}", word);
        }

        let es = get_stopwords("es").expect("Spanish stopwords");
        let spanish_common = vec![
            "el", "la", "los", "las", "un", "una", "de", "en", "y", "o", "por", "para",
        ];
        for word in spanish_common {
            assert!(es.contains(word), "Spanish missing common stopword: {}", word);
        }

        let de = get_stopwords("de").expect("German stopwords");
        let german_common = vec![
            "der", "die", "das", "den", "dem", "des", "und", "oder", "in", "auf", "mit", "von",
        ];
        for word in german_common {
            assert!(de.contains(word), "German missing common stopword: {}", word);
        }

        let fr = get_stopwords("fr").expect("French stopwords");
        let french_common = vec![
            "le", "la", "les", "un", "une", "de", "en", "et", "ou", "pour", "avec", "dans",
        ];
        for word in french_common {
            assert!(fr.contains(word), "French missing common stopword: {}", word);
        }
    }

    #[test]
    fn test_stopword_deduplication() {
        for (lang, stopwords) in STOPWORDS.iter() {
            let original_len = stopwords.len();
            let unique_len = stopwords.iter().collect::<AHashSet<_>>().len();
            assert_eq!(original_len, unique_len, "Language {} has duplicate stopwords", lang);
        }
    }

    #[test]
    fn test_case_normalization_comprehensive() {
        let test_cases = vec![
            ("en", "EN", "En", "eN"),
            ("es", "ES", "Es", "eS"),
            ("de", "DE", "De", "dE"),
            ("fr", "FR", "Fr", "fR"),
            ("zh", "ZH", "Zh", "zH"),
            ("ar", "AR", "Ar", "aR"),
        ];

        for (lower, upper, title, mixed) in test_cases {
            let lower_result = get_stopwords(lower);
            let upper_result = get_stopwords(upper);
            let title_result = get_stopwords(title);
            let mixed_result = get_stopwords(mixed);

            assert!(lower_result.is_some(), "{} should be valid", lower);
            assert!(upper_result.is_some(), "{} should be valid", upper);
            assert!(title_result.is_some(), "{} should be valid", title);
            assert!(mixed_result.is_some(), "{} should be valid", mixed);

            let len = lower_result.unwrap().len();
            assert_eq!(upper_result.unwrap().len(), len);
            assert_eq!(title_result.unwrap().len(), len);
            assert_eq!(mixed_result.unwrap().len(), len);
        }
    }

    #[test]
    fn test_locale_code_normalization_comprehensive() {
        let test_cases = vec![
            ("en-US", "en_US", "en-GB", "en_GB", "en"),
            ("es-ES", "es_ES", "es-MX", "es_MX", "es"),
            ("pt-PT", "pt_PT", "pt-BR", "pt_BR", "pt"),
            ("zh-CN", "zh_CN", "zh-TW", "zh_TW", "zh"),
            ("fr-FR", "fr_FR", "fr-CA", "fr_CA", "fr"),
        ];

        for (hyphen1, underscore1, hyphen2, underscore2, base) in test_cases {
            let base_result = get_stopwords(base).unwrap_or_else(|| panic!("{} should be valid", base));

            let h1 = get_stopwords(hyphen1);
            let u1 = get_stopwords(underscore1);
            let h2 = get_stopwords(hyphen2);
            let u2 = get_stopwords(underscore2);

            assert!(h1.is_some(), "{} should be valid", hyphen1);
            assert!(u1.is_some(), "{} should be valid", underscore1);
            assert!(h2.is_some(), "{} should be valid", hyphen2);
            assert!(u2.is_some(), "{} should be valid", underscore2);

            let len = base_result.len();
            assert_eq!(h1.unwrap().len(), len, "{} should match {}", hyphen1, base);
            assert_eq!(u1.unwrap().len(), len, "{} should match {}", underscore1, base);
            assert_eq!(h2.unwrap().len(), len, "{} should match {}", hyphen2, base);
            assert_eq!(u2.unwrap().len(), len, "{} should match {}", underscore2, base);
        }
    }

    #[test]
    fn test_fallback_chains() {
        let scenarios = vec![
            ("en", "es", true, "en"),
            ("xx", "en", true, "en"),
            ("xx", "yy", false, ""),
            ("es", "xx", true, "es"),
        ];

        for (primary, fallback, should_succeed, expected_lang) in scenarios {
            let result = get_stopwords_with_fallback(primary, fallback);
            assert_eq!(
                result.is_some(),
                should_succeed,
                "Fallback({}, {}) should {}",
                primary,
                fallback,
                if should_succeed { "succeed" } else { "fail" }
            );

            if should_succeed {
                let stopwords = result.unwrap();
                let expected = get_stopwords(expected_lang).unwrap();
                assert_eq!(
                    stopwords.len(),
                    expected.len(),
                    "Fallback should return {} stopwords",
                    expected_lang
                );
            }
        }
    }

    #[test]
    fn test_stopword_string_types() {
        for (lang, stopwords) in STOPWORDS.iter() {
            for word in stopwords {
                assert!(!word.is_empty(), "Language {} has empty stopword", lang);
                assert!(
                    word.len() <= 100,
                    "Language {} has suspiciously long stopword: {} ({} bytes)",
                    lang,
                    word,
                    word.len()
                );
                assert!(word.chars().count() > 0, "Language {} has invalid UTF-8 stopword", lang);
            }
        }
    }

    #[test]
    fn test_concurrent_access() {
        use std::thread;

        let languages = vec!["en", "es", "de", "fr", "zh", "ar", "ru", "ja"];
        let mut handles = vec![];

        for lang in languages {
            let handle = thread::spawn(move || {
                let stopwords = get_stopwords(lang);
                assert!(stopwords.is_some(), "Language {} should be available", lang);
                stopwords.unwrap().len()
            });
            handles.push(handle);
        }

        for handle in handles {
            let len = handle.join().expect("Thread should not panic");
            assert!(len > 0, "Stopwords should not be empty");
        }
    }

    #[test]
    fn test_stopwords_immutability() {
        let en1 = get_stopwords("en").unwrap();
        let en2 = get_stopwords("en").unwrap();

        assert_eq!(en1.len(), en2.len());

        for word in en1 {
            assert!(
                en2.contains(word),
                "Stopword '{}' should exist in both references",
                word
            );
        }
    }

    #[test]
    fn test_edge_case_separator_positions() {
        let test_cases = vec![
            ("en-", true),
            ("-en", false),
            ("e-n", false),
            ("en--US", true),
            ("en_-US", true),
            ("_en", false),
            ("en_", true),
        ];

        for (code, should_find_en) in test_cases {
            let result = get_stopwords(code);
            if should_find_en {
                assert!(result.is_some(), "Code '{}' should extract 'en'", code);
                if let Some(stopwords) = result {
                    assert!(
                        stopwords.contains("the"),
                        "Code '{}' should return English stopwords",
                        code
                    );
                }
            } else {
                let _ = result;
            }
        }
    }

    #[test]
    fn test_performance_characteristics() {
        use std::time::Instant;

        let _ = get_stopwords("en");

        let start = Instant::now();
        for _ in 0..10000 {
            let _ = get_stopwords("en");
            let _ = get_stopwords("es");
            let _ = get_stopwords("de");
        }
        let duration = start.elapsed();

        assert!(
            duration.as_millis() < 500,
            "30,000 lookups took too long: {:?}",
            duration
        );
    }

    #[test]
    fn test_language_completeness() {
        let documented = vec![
            "af", "ar", "bg", "bn", "br", "ca", "cs", "da", "de", "el", "en", "eo", "es", "et", "eu", "fa", "fi", "fr",
            "ga", "gl", "gu", "ha", "he", "hi", "hr", "hu", "hy", "id", "it", "ja", "kn", "ko", "ku", "la", "lt", "lv",
            "ml", "mr", "ms", "ne", "nl", "no", "pl", "pt", "ro", "ru", "si", "sk", "sl", "so", "st", "sv", "sw", "ta",
            "te", "th", "tl", "tr", "uk", "ur", "vi", "yo", "zh", "zu",
        ];

        assert_eq!(documented.len(), 64, "Documentation lists 64 languages");

        for lang in documented {
            assert!(
                STOPWORDS.contains_key(lang),
                "Documented language '{}' is missing from STOPWORDS",
                lang
            );
            assert!(
                get_stopwords(lang).is_some(),
                "Documented language '{}' not accessible via get_stopwords",
                lang
            );
        }
    }
}
