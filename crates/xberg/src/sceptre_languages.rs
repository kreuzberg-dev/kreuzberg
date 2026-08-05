//! Shared Sceptre Gen2 language-group routing.

use sceptre::Language;

pub(crate) fn language_group(language: &str) -> Option<Language> {
    let normalized = language.trim().to_ascii_lowercase().replace('_', "-");
    if ENGLISH_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::English)
    } else if LATIN_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::Latin)
    } else if CHINESE_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::ChineseSimplified)
    } else if JAPANESE_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::Japanese)
    } else if KOREAN_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::Korean)
    } else if TELUGU_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::Telugu)
    } else if KANNADA_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::Kannada)
    } else if CYRILLIC_LANGUAGES.contains(&normalized.as_str()) {
        Some(Language::Cyrillic)
    } else {
        None
    }
}

#[cfg(sceptre_ocr)]
pub(crate) fn language_group_name(language: Language) -> &'static str {
    match language {
        Language::English => "english",
        Language::Latin => "latin",
        Language::ChineseSimplified => "simplified_chinese",
        Language::Japanese => "japanese",
        Language::Korean => "korean",
        Language::Telugu => "telugu",
        Language::Kannada => "kannada",
        Language::Cyrillic => "cyrillic",
    }
}

#[cfg(any(sceptre_ocr, test))]
pub(crate) fn supported_language_aliases() -> Vec<&'static str> {
    let mut languages = ENGLISH_LANGUAGES
        .iter()
        .chain(LATIN_LANGUAGES)
        .chain(CHINESE_LANGUAGES)
        .chain(JAPANESE_LANGUAGES)
        .chain(KOREAN_LANGUAGES)
        .chain(TELUGU_LANGUAGES)
        .chain(KANNADA_LANGUAGES)
        .chain(CYRILLIC_LANGUAGES)
        .copied()
        .collect::<Vec<_>>();
    languages.sort_unstable();
    languages.dedup();
    languages
}

const ENGLISH_LANGUAGES: &[&str] = &["english", "en", "eng"];
const LATIN_LANGUAGES: &[&str] = &[
    "latin", "af", "afr", "az", "aze", "bs", "bos", "cs", "ces", "cze", "cy", "cym", "wel", "da", "dan", "de", "deu",
    "ger", "es", "spa", "et", "est", "fr", "fra", "fre", "ga", "gle", "hr", "hrv", "hu", "hun", "id", "ind", "is",
    "isl", "ice", "it", "ita", "ku", "kur", "la", "lat", "lt", "lit", "lv", "lav", "mi", "mri", "mao", "ms", "msa",
    "may", "mt", "mlt", "nl", "nld", "dut", "no", "nor", "oc", "oci", "pi", "pli", "pl", "pol", "pt", "por", "ro",
    "ron", "rum", "rs-latin", "sr-latn", "srp-latn", "sk", "slk", "slo", "sl", "slv", "sq", "sqi", "alb", "sv", "swe",
    "sw", "swa", "tl", "fil", "tr", "tur", "uz", "uzb", "vi", "vie",
];
const CHINESE_LANGUAGES: &[&str] = &[
    "chinese-simplified",
    "simplified-chinese",
    "ch-sim",
    "zh",
    "zh-cn",
    "zh-hans",
    "zho",
    "chi",
    "chs",
];
const JAPANESE_LANGUAGES: &[&str] = &["japanese", "ja", "jpn", "jpn-vert"];
const KOREAN_LANGUAGES: &[&str] = &["korean", "ko", "kor"];
const TELUGU_LANGUAGES: &[&str] = &["telugu", "te", "tel"];
const KANNADA_LANGUAGES: &[&str] = &["kannada", "kn", "kan"];
const CYRILLIC_LANGUAGES: &[&str] = &[
    "cyrillic",
    "ru",
    "rus",
    "rs-cyrillic",
    "sr-cyrl",
    "srp-cyrl",
    "be",
    "bel",
    "bg",
    "bul",
    "uk",
    "ukr",
    "mn",
    "mon",
    "abq",
    "ady",
    "kbd",
    "ava",
    "dar",
    "inh",
    "che",
    "lbe",
    "lez",
    "tab",
    "tjk",
    "tg",
    "tgk",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_route_every_advertised_alias() {
        for alias in supported_language_aliases() {
            assert!(language_group(alias).is_some(), "{alias} must resolve");
        }
    }

    #[test]
    fn should_normalize_easyocr_script_tokens() {
        assert_eq!(language_group("CH_SIM"), Some(Language::ChineseSimplified));
        assert_eq!(language_group("rs_latin"), Some(Language::Latin));
        assert_eq!(language_group("rs-cyrillic"), Some(Language::Cyrillic));
    }

    #[test]
    fn should_reject_languages_without_gen2_models() {
        for language in ["ara", "cat", "kaz", "tam", "tha"] {
            assert_eq!(language_group(language), None, "{language} must be rejected");
        }
    }
}
