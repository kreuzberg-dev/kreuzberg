use ahash::AHashMap;
use once_cell::sync::Lazy;
use regex::Regex;
use std::borrow::Cow;

use memchr::memmem;

// ============================================================================
// ============================================================================

const OCR_PENALTY_WEIGHT: f64 = 0.3;
const SCRIPT_PENALTY_WEIGHT: f64 = 0.2;
const NAV_PENALTY_WEIGHT: f64 = 0.1;
const STRUCTURE_BONUS_WEIGHT: f64 = 0.2;
const METADATA_BONUS_WEIGHT: f64 = 0.1;

const MIN_TEXT_LENGTH: usize = 10;
const MIN_SENTENCE_WORDS: f64 = 10.0;
const MAX_SENTENCE_WORDS: f64 = 30.0;
const MIN_PARAGRAPH_WORDS: f64 = 50.0;
const MAX_PARAGRAPH_WORDS: f64 = 300.0;

static DASH_PATTERN: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[-]{3,}").expect("Dash pattern regex is valid and should compile"));

/// Combined OCR artifact pattern for single-pass scanning (used in calculate_ocr_penalty).
/// This pattern combines 5 of the 6 OCR patterns with alternation to reduce regex passes
/// from 5 separate find_iter calls to 1. The dash pattern is handled separately due to
/// line-based context checking.
static COMBINED_OCR_ARTIFACTS_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?x)
        \b[a-zA-Z]\s{2,}[a-zA-Z]\s{2,}[a-zA-Z]\b |  # Scattered chars
        [.]{3,}|[_]{3,} |                              # Repeated punctuation
        \s[.,;:!?]\s |                                 # Isolated punctuation
        \b[a-zA-Z]+[0-9]+[a-zA-Z]+[a-zA-Z0-9]*\b |   # Malformed words
        \s{3,}                                        # Excessive whitespace
    ",
    )
    .expect("Combined OCR artifacts regex pattern is valid and should compile")
});

static JS_FUNCTION_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)function\s+\w+\s*\([^)]*\)\s*\{[^}]*\}")
        .expect("JavaScript function regex pattern is valid and should compile")
});
static CSS_RULES_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\.[a-zA-Z][\w-]*\s*\{[^}]*\}").expect("CSS rules regex pattern is valid and should compile")
});

static NAV_WORDS_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:Skip to main content|Back to top|Main navigation|Site navigation)\b")
        .expect("Navigation words regex pattern is valid and should compile")
});
static BREADCRUMB_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?:Home\s*[>»]\s*|[>»]\s*){2,}").expect("Breadcrumb regex pattern is valid and should compile")
});
static PAGINATION_PATTERN: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:Page \d+ of \d+|First page|Last page|Previous page|Next page|^\d+ of \d+$)\b")
        .expect("Pagination regex pattern is valid and should compile")
});

static SENTENCE_DETECT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[.!?]\s+[A-Z]").expect("Sentence detection regex pattern is valid and should compile"));
static PUNCTUATION_DETECT: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"[.!?]").expect("Punctuation detection regex pattern is valid and should compile"));

#[cfg(test)]
static NEWLINE_CLEANUP: Lazy<Regex> =
    Lazy::new(|| Regex::new(r"\n+").expect("Newline cleanup regex pattern is valid and should compile"));

#[inline]
fn sum_match_lengths(text: &str, pattern: &Regex) -> usize {
    pattern.find_iter(text).map(|m| m.len()).sum()
}

/// Count the total byte span of all `<open_tag>...</close_tag>` pairs using `memmem` scanners.
///
/// The text is lowercased once before scanning so the needles can be lowercase-only, matching
/// `<script>`, `<SCRIPT>`, `<Script>`, etc. The byte lengths are measured on the lowercased
/// copy but equal those of the original because the tags are pure ASCII.
///
/// This replaces `(?is)<script[^>]*>.*?</script>` and the equivalent style regex.  Those
/// patterns trigger the `regex_automata` `BoundedBacktracker` on large inputs, causing
/// multi-MiB transient allocations (observed in staging heap profiles at ~1.6 MiB combined).
fn count_tag_bytes(text: &str, open_needle: &[u8], close_needle: &[u8]) -> usize {
    let lower = text.to_ascii_lowercase();
    let bytes = lower.as_bytes();

    let open_finder = memmem::Finder::new(open_needle);
    let close_finder = memmem::Finder::new(close_needle);

    let mut total = 0usize;
    let mut pos = 0usize;

    while pos < bytes.len() {
        let Some(open_start) = open_finder.find(&bytes[pos..]) else {
            break;
        };
        let open_start = pos + open_start;

        let tag_body_start = match bytes[open_start..].iter().position(|&b| b == b'>') {
            Some(rel) => open_start + rel + 1,
            None => break,
        };

        let Some(close_rel) = close_finder.find(&bytes[tag_body_start..]) else {
            break;
        };
        let close_end = tag_body_start + close_rel + close_needle.len();

        total += close_end - open_start;
        pos = close_end;
    }

    total
}

/// Score an extracted text on the closed interval `[0.0, 1.0]`, where higher is better.
///
/// `1.0` is the neutral score for clean prose; penalties (OCR artifacts, embedded
/// script/style noise, navigation chrome) subtract, structural cues (headings,
/// punctuation) add. The result is clamped to `[0.0, 1.0]`.
///
/// Pass `metadata` as `None` when the caller has no extraction metadata available;
/// the metadata bonus simply isn't applied in that case. Texts shorter than
/// `MIN_TEXT_LENGTH` short-circuit to `0.1` regardless of metadata.
#[cfg_attr(alef, alef(skip))]
pub fn calculate_quality_score(text: &str, metadata: Option<&AHashMap<Cow<'static, str>, serde_json::Value>>) -> f64 {
    if text.is_empty() || text.trim().is_empty() {
        return 0.0;
    }

    let total_chars = text.len() as f64;

    if text.len() < MIN_TEXT_LENGTH {
        return 0.1;
    }

    // Every penalty/bonus applies regardless of `text.len()`. This used to
    // short-circuit script and navigation penalties for texts at or below
    // `LARGE_TEXT_LENGTH`, so a short HTML-derived fragment full of nav chrome
    // (breadcrumbs, "Skip to main content", pagination) scored as clean prose
    // purely because it was short (#267).
    let mut score = 1.0;

    let ocr_penalty = calculate_ocr_penalty(text, total_chars);
    let script_penalty = calculate_script_penalty(text, total_chars);
    let nav_penalty = calculate_navigation_penalty(text, total_chars);
    let structure_bonus = calculate_structure_bonus(text);

    score -= ocr_penalty * OCR_PENALTY_WEIGHT;
    score -= script_penalty * SCRIPT_PENALTY_WEIGHT;
    score -= nav_penalty * NAV_PENALTY_WEIGHT;
    score += structure_bonus * STRUCTURE_BONUS_WEIGHT;

    if let Some(metadata) = metadata {
        score += calculate_metadata_bonus(metadata) * METADATA_BONUS_WEIGHT;
    }

    score.clamp(0.0, 1.0)
}

#[inline]
fn calculate_ocr_penalty(text: &str, total_chars: f64) -> f64 {
    if total_chars == 0.0 {
        return 0.0;
    }

    if memmem::find(text.as_bytes(), b"  ").is_none() && memmem::find(text.as_bytes(), b"...").is_none() {
        return 0.0;
    }

    let artifact_chars =
        sum_match_lengths(text, &COMBINED_OCR_ARTIFACTS_PATTERN) + count_non_table_dash_artifacts(text);

    (artifact_chars as f64 / total_chars).min(1.0)
}

#[inline]
fn count_non_table_dash_artifacts(text: &str) -> usize {
    let mut artifact_count = 0;

    for line in text.lines() {
        let trimmed = line.trim();
        let is_table_separator = trimmed.starts_with('|')
            && trimmed.ends_with('|')
            && trimmed
                .chars()
                .all(|c| c == '|' || c == '-' || c.is_whitespace() || c == ':');

        if !is_table_separator {
            for m in DASH_PATTERN.find_iter(line) {
                artifact_count += m.len();
            }
        }
    }

    artifact_count
}

/// Cap applied to `text` before running `JS_FUNCTION_PATTERN` and `CSS_RULES_PATTERN`.
///
/// Both patterns use bounded character-class repetition (`[^)]*`, `[^}]*`) that still triggers
/// the `regex_automata` `BoundedBacktracker` when the input slice is large. Because these are
/// noise-detection heuristics (not structural parsers), capping at 64 KiB does not meaningfully
/// change scores for real documents.
const JS_CSS_PATTERN_INPUT_CAP: usize = 64 * 1024;

#[inline]
fn calculate_script_penalty(text: &str, total_chars: f64) -> f64 {
    if total_chars == 0.0 {
        return 0.0;
    }

    let bytes = text.as_bytes();
    if memmem::find(bytes, b"function").is_none()
        && memmem::find(bytes, b"<script").is_none()
        && memmem::find(bytes, b"<style").is_none()
        && memmem::find(bytes, b"FUNCTION").is_none()
        && memmem::find(bytes, b"<SCRIPT").is_none()
        && memmem::find(bytes, b"<STYLE").is_none()
    {
        return 0.0;
    }

    let truncated = if text.len() > JS_CSS_PATTERN_INPUT_CAP {
        let mut end = JS_CSS_PATTERN_INPUT_CAP;
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        &text[..end]
    } else {
        text
    };

    let script_chars = sum_match_lengths(truncated, &JS_FUNCTION_PATTERN)
        + sum_match_lengths(truncated, &CSS_RULES_PATTERN)
        + count_tag_bytes(text, b"<script", b"</script>")
        + count_tag_bytes(text, b"<style", b"</style>");

    (script_chars as f64 / total_chars).min(1.0)
}

#[inline]
fn calculate_navigation_penalty(text: &str, total_chars: f64) -> f64 {
    if total_chars == 0.0 {
        return 0.0;
    }

    let nav_chars = sum_match_lengths(text, &NAV_WORDS_PATTERN)
        + sum_match_lengths(text, &BREADCRUMB_PATTERN)
        + sum_match_lengths(text, &PAGINATION_PATTERN);

    (nav_chars as f64 / total_chars).min(1.0)
}

#[inline]
fn calculate_structure_bonus(text: &str) -> f64 {
    if text.is_empty() {
        return 0.0;
    }

    let sentence_count = SENTENCE_DETECT.find_iter(text).count() as f64;
    let paragraph_count = memmem::find_iter(text.as_bytes(), b"\n\n").count() as f64 + 1.0;
    let words = text.split_whitespace().count() as f64;

    if words == 0.0 {
        return 0.0;
    }

    let avg_words_per_sentence = words / sentence_count.max(1.0);
    let avg_words_per_paragraph = words / paragraph_count;

    let mut structure_score: f64 = 0.0;

    if (MIN_SENTENCE_WORDS..=MAX_SENTENCE_WORDS).contains(&avg_words_per_sentence) {
        structure_score += 0.3;
    }

    if (MIN_PARAGRAPH_WORDS..=MAX_PARAGRAPH_WORDS).contains(&avg_words_per_paragraph) {
        structure_score += 0.3;
    }

    if paragraph_count > 1.0 {
        structure_score += 0.2;
    }

    if PUNCTUATION_DETECT.is_match(text) {
        structure_score += 0.2;
    }

    structure_score.min(1.0)
}

#[inline]
fn calculate_metadata_bonus(metadata: &AHashMap<Cow<'static, str>, serde_json::Value>) -> f64 {
    const IMPORTANT_FIELDS: &[&str] = &["title", "author", "subject", "description", "keywords"];

    let present_fields = IMPORTANT_FIELDS
        .iter()
        .filter(|&&field| metadata.contains_key(field))
        .count();

    present_fields as f64 / IMPORTANT_FIELDS.len() as f64
}

#[cfg(test)]
static WHITESPACE_NORMALIZE: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"[ \t\f\v\r\xa0\u{2000}-\u{200b}\u{2028}\u{2029}\u{3000}]+")
        .expect("Whitespace normalization regex pattern is valid and should compile")
});

#[cfg(test)]
pub(crate) fn normalize_spaces(text: &str) -> String {
    if text.is_empty() || text.trim().is_empty() {
        return String::new();
    }

    let mut result = String::with_capacity(text.len());

    let mut first = true;
    for paragraph in text.split("\n\n") {
        let trimmed = paragraph.trim();
        if trimmed.is_empty() {
            continue;
        }

        if !first {
            result.push_str("\n\n");
        }
        first = false;

        let cleaned = WHITESPACE_NORMALIZE.replace_all(paragraph, " ");
        let cleaned = NEWLINE_CLEANUP.replace_all(&cleaned, "\n");

        let mut first_line = true;
        for line in cleaned.split('\n') {
            let line = line.trim();
            if !line.is_empty() {
                if !first_line {
                    result.push('\n');
                }
                result.push_str(line);
                first_line = false;
            }
        }
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_calculate_quality_score_empty_text() {
        assert_eq!(calculate_quality_score("", None), 0.0);
        assert_eq!(calculate_quality_score("   ", None), 0.0);
        assert_eq!(calculate_quality_score("\n\n\n", None), 0.0);
    }

    #[test]
    fn test_calculate_quality_score_short_text() {
        let text = "Hello";
        let score = calculate_quality_score(text, None);
        assert_eq!(score, 0.1);
    }

    #[test]
    fn test_calculate_quality_score_normal_text() {
        let text =
            "This is a normal sentence with proper punctuation. It has multiple sentences. And proper structure.";
        let score = calculate_quality_score(text, None);
        assert!(score > 0.5);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_normalize_spaces_empty() {
        assert_eq!(normalize_spaces(""), "");
        assert_eq!(normalize_spaces("   "), "");
    }

    #[test]
    fn test_normalize_spaces_single_paragraph() {
        let text = "This  is   a   test";
        let normalized = normalize_spaces(text);
        assert_eq!(normalized, "This is a test");
    }

    #[test]
    fn test_calculate_quality_score_with_metadata() {
        let text = "This is a normal text with proper structure.";
        let mut metadata: AHashMap<Cow<'static, str>, serde_json::Value> = AHashMap::new();
        metadata.insert(Cow::Borrowed("title"), serde_json::json!("Test Title"));
        metadata.insert(Cow::Borrowed("author"), serde_json::json!("Test Author"));

        let score = calculate_quality_score(text, Some(&metadata));
        assert!(score > 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_calculate_ocr_penalty_clean_text() {
        let text = "This is clean text without artifacts";
        let penalty = calculate_ocr_penalty(text, text.len() as f64);
        assert_eq!(penalty, 0.0);
    }

    #[test]
    fn test_calculate_ocr_penalty_with_artifacts() {
        let text = "Text with  excessive   spaces and ....... dots";
        let penalty = calculate_ocr_penalty(text, text.len() as f64);
        assert!(penalty > 0.0);
        assert!(penalty <= 1.0);
    }

    #[test]
    fn test_calculate_script_penalty_clean_text() {
        let text = "This is clean text without scripts";
        let penalty = calculate_script_penalty(text, text.len() as f64);
        assert_eq!(penalty, 0.0);
    }

    #[test]
    fn test_calculate_script_penalty_with_js() {
        let text = "function test() { return 42; }";
        let penalty = calculate_script_penalty(text, text.len() as f64);
        assert!(penalty > 0.0);
    }

    #[test]
    fn test_calculate_navigation_penalty_clean_text() {
        let text = "This is clean text without navigation";
        let penalty = calculate_navigation_penalty(text, text.len() as f64);
        assert_eq!(penalty, 0.0);
    }

    #[test]
    fn test_calculate_navigation_penalty_with_nav() {
        let text = "Skip to main content and Back to top links everywhere";
        let penalty = calculate_navigation_penalty(text, text.len() as f64);
        assert!(penalty > 0.0);
    }

    #[test]
    fn test_calculate_structure_bonus_empty() {
        assert_eq!(calculate_structure_bonus(""), 0.0);
    }

    #[test]
    fn test_calculate_structure_bonus_well_structured() {
        let text = "This is a sentence. This is another sentence.\n\nNew paragraph here. More content.";
        let bonus = calculate_structure_bonus(text);
        assert!(bonus > 0.0);
        assert!(bonus <= 1.0);
    }

    #[test]
    fn test_calculate_metadata_bonus_empty() {
        let metadata: AHashMap<Cow<'static, str>, serde_json::Value> = AHashMap::new();
        let bonus = calculate_metadata_bonus(&metadata);
        assert_eq!(bonus, 0.0);
    }

    #[test]
    fn test_calculate_metadata_bonus_full() {
        let mut metadata: AHashMap<Cow<'static, str>, serde_json::Value> = AHashMap::new();
        metadata.insert(Cow::Borrowed("title"), serde_json::json!("Title"));
        metadata.insert(Cow::Borrowed("author"), serde_json::json!("Author"));
        metadata.insert(Cow::Borrowed("subject"), serde_json::json!("Subject"));
        metadata.insert(Cow::Borrowed("description"), serde_json::json!("Description"));
        metadata.insert(Cow::Borrowed("keywords"), serde_json::json!("Keywords"));

        let bonus = calculate_metadata_bonus(&metadata);
        assert_eq!(bonus, 1.0);
    }

    #[test]
    fn test_normalize_spaces_multiple_paragraphs() {
        let text = "First paragraph.\n\nSecond paragraph.";
        let normalized = normalize_spaces(text);
        assert!(normalized.contains("\n\n"));
    }

    #[test]
    fn test_normalize_spaces_preserves_paragraphs() {
        let text = "Para 1\n\n\n\nPara 2";
        let normalized = normalize_spaces(text);
        assert_eq!(normalized, "Para 1\n\nPara 2");
    }

    #[test]
    fn test_count_non_table_dash_artifacts() {
        let text = "Some text --- with dashes";
        let count = count_non_table_dash_artifacts(text);
        assert!(count > 0);
    }

    #[test]
    fn test_count_non_table_dash_artifacts_preserves_tables() {
        let text = "| Header |\n|--------|\n| Data   |";
        let count = count_non_table_dash_artifacts(text);
        assert_eq!(count, 0);
    }

    #[test]
    fn test_quality_score_large_text_with_ocr_issues() {
        let text = "a".repeat(2000) + "   " + &"b".repeat(2000);
        let score = calculate_quality_score(&text, None);
        assert!(score >= 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_quality_score_clamped_to_range() {
        let perfect_text = "This is perfect text. ".repeat(100);
        let score = calculate_quality_score(&perfect_text, None);
        assert!(score >= 0.0);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_quality_constants() {
        assert_eq!(MIN_TEXT_LENGTH, 10);
        assert_eq!(OCR_PENALTY_WEIGHT, 0.3);
    }

    #[test]
    fn should_apply_navigation_penalty_to_short_text() {
        // Below the removed `LARGE_TEXT_LENGTH` threshold, navigation chrome
        // used to be scored as if it were clean prose (#267). This text is
        // short (well under 1000 bytes) and dominated by nav chrome, so the
        // navigation penalty must pull the score down from the neutral 1.0
        // baseline instead of leaving it unpenalized.
        let text = "Skip to main content. Back to top. Home > Products > Widgets. Page 1 of 12.";
        assert!(text.len() < 1000, "fixture must exercise the short-text path");

        let score = calculate_quality_score(text, None);
        let nav_penalty = calculate_navigation_penalty(text, text.len() as f64);

        assert!(
            nav_penalty > 0.0,
            "fixture must actually trigger the navigation pattern"
        );
        assert!(
            score < 1.0,
            "navigation-heavy short text must be penalized, got score {score}"
        );
    }

    #[test]
    fn should_apply_script_penalty_to_short_text() {
        // Same defect as above but for the script/style penalty (#267).
        let text = "function init() { doStuff(); } Welcome to our site!";
        assert!(text.len() < 1000, "fixture must exercise the short-text path");

        let score = calculate_quality_score(text, None);
        let script_penalty = calculate_script_penalty(text, text.len() as f64);

        assert!(script_penalty > 0.0, "fixture must actually trigger the script pattern");
        assert!(
            score < 1.0,
            "script-heavy short text must be penalized, got score {score}"
        );
    }
}
