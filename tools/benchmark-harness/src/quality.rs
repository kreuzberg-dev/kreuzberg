//! Quality scoring module for benchmark results.
//!
//! Computes F1-based quality metrics by comparing extracted text against ground truth.
//! Uses token-level (bag-of-words) precision and recall.
//!
//! # Scoring weights
//!
//! Text-only scoring uses a **0.6 / 0.4 text / numeric split**:
//!
//! ```text
//! quality_score = 0.6 * f1_text + 0.4 * f1_numeric
//! ```
//!
//! Numeric tokens receive disproportionate weight (40% despite typically being
//! a small fraction of the token count) because financial documents, scientific
//! papers, and tabular data depend heavily on number accuracy. A single wrong
//! digit can invalidate an entire table row or equation.
//!
//! When markdown ground truth is available, **combined scoring** kicks in:
//!
//! ```text
//! quality_score = 0.5 * f1_text + 0.2 * f1_numeric + 0.3 * f1_layout
//! ```
//!
//! The layout component (`f1_layout`) is canonical SF1 from
//! [`structural_sidecar`] and captures structural fidelity across paragraph,
//! heading, list, table, binding-edge, and reading-order dimensions.
//!
//! # Tokenization
//!
//! Tokenization is intentionally simple: lowercase, split on whitespace,
//! strip non-alphanumeric characters except periods and commas embedded between
//! alphanumeric characters (preserving decimal numbers like "3.14" and European
//! format "3,14"). CJK runs use character bigrams because Chinese, Japanese,
//! and Korean OCR engines commonly insert or reorder layout-dependent line breaks.
//! This preserves punctuation that is semantically meaningful while ignoring
//! decorative punctuation.

use crate::types::{OutputFormat, QualityMetrics};
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

// The structural-sidecar file lives at `src/structural_sidecar.rs`; it is attached ~keep
// here (rather than in `lib.rs`) via `#[path]` so the crate root stays untouched.
#[path = "structural_sidecar.rs"]
pub mod structural_sidecar;

/// Regex to strip markdown image syntax `![alt](url)` → `alt`
static MD_IMAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"!\[([^\]]*)\]\([^)]*\)").expect("invalid regex"));

/// Regex to strip markdown link syntax `[text](url)` → `text`
static MD_LINK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[([^\]]*)\]\([^)]*\)").expect("invalid regex"));

/// Strip markdown link and image syntax so URL components don't become tokens.
/// `![alt](url)` → `alt`, `[text](url)` → `text`.
fn strip_markdown_links(text: &str) -> String {
    let text = MD_IMAGE_RE.replace_all(text, "$1");
    MD_LINK_RE.replace_all(&text, "$1").into_owned()
}

/// Compute quality metrics comparing extracted text against ground truth,
/// optionally including structural quality scoring when markdown GT is available.
///
/// When `output_format` is `Markdown` and `ground_truth_markdown` is `Some`, computes
/// structural F1 from markdown block comparison and adjusts the quality_score formula:
///   quality_score = 0.5 * f1_text + 0.2 * f1_numeric + 0.3 * f1_layout
///
/// When `output_format` is `Plaintext`, returns text-only scoring regardless of
/// markdown ground truth availability:
///   quality_score = 0.6 * f1_text + 0.4 * f1_numeric
///   f1_score_layout = None
///
/// When `output_format` is `Markdown` but `ground_truth_markdown` is `None`, falls back
/// to text-only scoring:
///   quality_score = 0.6 * f1_text + 0.4 * f1_numeric
pub fn compute_quality_with_structure(
    extracted: &str,
    ground_truth: &str,
    ground_truth_markdown: Option<&str>,
    output_format: OutputFormat,
) -> QualityMetrics {
    if output_format == OutputFormat::Plaintext {
        return compute_quality(extracted, ground_truth);
    }

    let mut metrics = compute_quality(extracted, ground_truth);

    if let Some(md_gt) = ground_truth_markdown {
        let structural_f1 = structural_sidecar::score_markdown(extracted, md_gt).sf1;
        metrics.f1_score_layout = Some(structural_f1);
        metrics.quality_score = if has_any_numeric_tokens(extracted, ground_truth) {
            0.5 * metrics.f1_score_text + 0.2 * metrics.f1_score_numeric + 0.3 * structural_f1
        } else {
            0.625 * metrics.f1_score_text + 0.375 * structural_f1
        };
    }

    metrics.correct = metrics.quality_score >= 0.95;
    metrics
}

/// Compute quality metrics comparing extracted text against ground truth
///
/// Algorithm:
/// 1. Tokenize both texts: lowercase, split on whitespace, strip non-alphanumeric chars except periods and commas
///    - "3.14" is preserved as a single token
///    - "3,14" is preserved as a single token (European decimal format)
/// 2. Build token multisets (bag of words with counts)
/// 3. Compute precision = |intersection| / |extracted tokens|
/// 4. Compute recall = |intersection| / |ground truth tokens|
/// 5. F1 = 2 * precision * recall / (precision + recall)
///    - If both token sets are empty, F1 = 1.0 (vacuously perfect match)
/// 6. Separate F1 for all tokens vs numeric-only tokens
/// 7. quality_score = 0.6 * f1_text + 0.4 * f1_numeric
pub fn compute_quality(extracted: &str, ground_truth: &str) -> QualityMetrics {
    let extracted_tokens = tokenize(extracted);
    let truth_tokens = tokenize(ground_truth);

    let f1_score_text = compute_f1(&extracted_tokens, &truth_tokens);

    let extracted_numeric = filter_numeric(&extracted_tokens);
    let truth_numeric = filter_numeric(&truth_tokens);
    let f1_score_numeric = compute_f1(&extracted_numeric, &truth_numeric);

    let quality_score = if extracted_numeric.is_empty() && truth_numeric.is_empty() {
        f1_score_text
    } else {
        0.6 * f1_score_text + 0.4 * f1_score_numeric
    };

    let (missing_tokens, extra_tokens) = compute_token_diff(&extracted_tokens, &truth_tokens);

    let correct = quality_score >= 0.95;

    QualityMetrics {
        f1_score_text,
        f1_score_numeric,
        f1_score_layout: None,
        quality_score,
        missing_tokens,
        extra_tokens,
        correct,
    }
}

/// Tokenize text: lowercase, split on whitespace, strip non-alphanumeric characters
/// (preserving `.` and `,` only when embedded between alphanumeric chars, e.g. "3.14", "3,14")
pub fn tokenize(text: &str) -> Vec<String> {
    let text = strip_markdown_links(text);
    let tokens = text
        .to_lowercase()
        .split_whitespace()
        .map(|w| {
            let kept: String = w
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '.' || *c == ',')
                .collect();
            kept.trim_matches(|c: char| c == '.' || c == ',').to_string()
        })
        .filter(|w| !w.is_empty())
        .collect();
    tokenize_cjk_bigrams(tokens)
}

fn normalize_numeric_token(token: String) -> String {
    let digit_count = token.chars().filter(|c| c.is_ascii_digit()).count();
    if digit_count == 0 || digit_count > 15 {
        return token;
    }
    // Normalize thousands separators ("1,000" -> "1000") before the numeric parse so a
    // grouped number and its bare form become the same token. Only strip commas that form
    // well-shaped 3-digit groups, to avoid corrupting European decimals like "3,14". ~keep
    let candidate = if is_thousands_grouped(&token) {
        token.replace(',', "")
    } else {
        token.clone()
    };
    candidate
        .parse::<f64>()
        .map_or(token.clone(), |number| format!("{number}"))
}

/// Expand CJK script runs into overlapping bigrams while preserving non-CJK tokens.
/// Whitespace between CJK characters is decorative OCR layout, not a word boundary. ~keep
fn tokenize_cjk_bigrams(tokens: Vec<String>) -> Vec<String> {
    let mut result = Vec::new();
    let mut cjk_run = String::new();
    for token in tokens {
        let characters: Vec<char> = token.chars().collect();
        let mut start = 0;
        while start < characters.len() {
            let is_cjk = is_cjk_character(characters[start]);
            let mut end = start + 1;
            while end < characters.len() && is_cjk_character(characters[end]) == is_cjk {
                end += 1;
            }
            let run: String = characters[start..end].iter().collect();
            if is_cjk {
                cjk_run.push_str(&run);
            } else if run.chars().all(|character| matches!(character, '.' | ',')) {
                // Punctuation between CJK characters is decorative and must not split the run. ~keep
            } else {
                push_cjk_bigrams(&mut result, &mut cjk_run);
                result.push(normalize_numeric_token(run));
            }
            start = end;
        }
    }
    push_cjk_bigrams(&mut result, &mut cjk_run);
    result
}

fn push_cjk_bigrams(result: &mut Vec<String>, run: &mut String) {
    let characters: Vec<char> = run.chars().collect();
    if characters.len() == 1 {
        result.push(run.clone());
    } else {
        result.extend(characters.windows(2).map(|pair| pair.iter().collect()));
    }
    run.clear();
}

/// Whether a character belongs to a Chinese, Japanese, or Korean script block.
fn is_cjk_character(character: char) -> bool {
    matches!(
        character,
        '\u{1100}'..='\u{11ff}'
            | '\u{2e80}'..='\u{2eff}'
            // Japanese iteration marks such as `々` survive alphanumeric filtering; excluding
            // them would disable bigram expansion for an entire whitespace-free OCR line. ~keep
            | '\u{3000}'..='\u{303f}'
            | '\u{3040}'..='\u{30ff}'
            | '\u{3130}'..='\u{318f}'
            | '\u{31f0}'..='\u{31ff}'
            | '\u{3400}'..='\u{4dbf}'
            | '\u{4e00}'..='\u{9fff}'
            | '\u{a960}'..='\u{a97f}'
            | '\u{ac00}'..='\u{d7af}'
            | '\u{d7b0}'..='\u{d7ff}'
            | '\u{f900}'..='\u{faff}'
            | '\u{ff66}'..='\u{ff9f}'
            | '\u{20000}'..='\u{2fa1f}'
    )
}

/// Whether a numeric token uses `,` as a thousands separator in well-formed 3-digit groups
/// (e.g. `1,000`, `12,345,678`, `1,234.56`) — as opposed to a European decimal comma (`3,14`),
/// which must be left untouched.
fn is_thousands_grouped(token: &str) -> bool {
    let Some(int_part) = token.split('.').next() else {
        return false;
    };
    let groups: Vec<&str> = int_part.split(',').collect();
    if groups.len() < 2 {
        return false;
    }
    if groups[0].is_empty() || groups[0].len() > 3 || !groups[0].bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    groups[1..]
        .iter()
        .all(|g| g.len() == 3 && g.bytes().all(|b| b.is_ascii_digit()))
}

/// Check whether either text has any numeric tokens (used to decide scoring formula).
fn has_any_numeric_tokens(text_a: &str, text_b: &str) -> bool {
    let a_tokens = tokenize(text_a);
    let b_tokens = tokenize(text_b);
    !filter_numeric(&a_tokens).is_empty() || !filter_numeric(&b_tokens).is_empty()
}

/// Filter tokens to only those containing numeric characters (Unicode-aware)
fn filter_numeric(tokens: &[String]) -> Vec<String> {
    tokens
        .iter()
        .filter(|t| t.chars().any(|c| c.is_numeric()))
        .cloned()
        .collect()
}

/// Compute F1 score between two token bags using multiset intersection
pub fn compute_f1(extracted: &[String], truth: &[String]) -> f64 {
    if extracted.is_empty() && truth.is_empty() {
        return 1.0;
    }
    if extracted.is_empty() || truth.is_empty() {
        return 0.0;
    }

    let extracted_counts = build_counts(extracted);
    let truth_counts = build_counts(truth);

    let intersection: usize = truth_counts
        .iter()
        .map(|(token, &count)| {
            let ext_count = extracted_counts.get(token).copied().unwrap_or(0);
            ext_count.min(count)
        })
        .sum();

    let precision = intersection as f64 / extracted.len() as f64;
    let recall = intersection as f64 / truth.len() as f64;

    if precision + recall == 0.0 {
        return 0.0;
    }

    2.0 * precision * recall / (precision + recall)
}

/// Above this character length, char-level edit distance (O(n·m)) is too slow to
/// run per document, so the CER helpers report NaN rather than stall the bench.
const CER_MAX_CHARS: usize = 16_384;

/// Levenshtein edit distance between two char slices. Two-row DP: O(n·m) time,
/// O(min(n, m)) space.
fn edit_distance(a: &[char], b: &[char]) -> usize {
    let (a, b) = if a.len() < b.len() { (b, a) } else { (a, b) };
    if b.is_empty() {
        return a.len();
    }
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut curr = vec![0usize; b.len() + 1];
    for (i, &ca) in a.iter().enumerate() {
        curr[0] = i + 1;
        for (j, &cb) in b.iter().enumerate() {
            let substitution = prev[j] + usize::from(ca != cb);
            curr[j + 1] = substitution.min(prev[j + 1] + 1).min(curr[j] + 1);
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    prev[b.len()]
}

/// Character error rate: edit distance normalized by the reference length. 0.0
/// is identical; values above 1.0 are possible when the hypothesis runs long.
/// Returns NaN for an empty reference or when either side exceeds
/// [`CER_MAX_CHARS`] (the metric is a report-only OCR diagnostic).
pub fn char_error_rate(reference: &str, hypothesis: &str) -> f64 {
    let reference_chars: Vec<char> = reference.chars().collect();
    let hypothesis_chars: Vec<char> = hypothesis.chars().collect();
    if reference_chars.is_empty() || reference_chars.len().max(hypothesis_chars.len()) > CER_MAX_CHARS {
        return f64::NAN;
    }
    edit_distance(&reference_chars, &hypothesis_chars) as f64 / reference_chars.len() as f64
}

/// Order- and character-sensitive text similarity in `[0, 1]`: `1 − distance /
/// max(len)`. 1.0 is identical, 0.0 fully dissimilar. Complements the
/// bag-of-words TF1 by penalizing transpositions and character-level OCR slips.
/// Returns NaN when both sides are empty or either exceeds [`CER_MAX_CHARS`].
pub fn normalized_edit_similarity(a: &str, b: &str) -> f64 {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let max_len = a_chars.len().max(b_chars.len());
    if max_len == 0 || max_len > CER_MAX_CHARS {
        return f64::NAN;
    }
    1.0 - edit_distance(&a_chars, &b_chars) as f64 / max_len as f64
}

/// Build a token frequency map
fn build_counts(tokens: &[String]) -> HashMap<&str, usize> {
    let mut counts = HashMap::new();
    for token in tokens {
        *counts.entry(token.as_str()).or_insert(0) += 1;
    }
    counts
}

/// Compute token-level diff between extracted and ground truth token bags.
///
/// Returns (missing_tokens, extra_tokens) where:
/// - missing_tokens: tokens in GT with higher count than in extraction (recall misses)
/// - extra_tokens: tokens in extraction with higher count than in GT (precision misses)
///
/// Both are sorted by deficit/surplus count descending.
pub type TokenDiff = (Vec<(String, usize)>, Vec<(String, usize)>);

pub fn compute_token_diff(extracted: &[String], truth: &[String]) -> TokenDiff {
    let extracted_counts = build_counts(extracted);
    let truth_counts = build_counts(truth);

    let mut missing: Vec<(String, usize)> = truth_counts
        .iter()
        .filter_map(|(&token, &gt_count)| {
            let ext_count = extracted_counts.get(token).copied().unwrap_or(0);
            if gt_count > ext_count {
                Some((token.to_string(), gt_count - ext_count))
            } else {
                None
            }
        })
        .collect();
    missing.sort_by_key(|b| std::cmp::Reverse(b.1));

    let mut extra: Vec<(String, usize)> = extracted_counts
        .iter()
        .filter_map(|(&token, &ext_count)| {
            let gt_count = truth_counts.get(token).copied().unwrap_or(0);
            if ext_count > gt_count {
                Some((token.to_string(), ext_count - gt_count))
            } else {
                None
            }
        })
        .collect();
    extra.sort_by_key(|b| std::cmp::Reverse(b.1));

    (missing, extra)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_text() {
        let text = "Hello world this is a test";
        let result = compute_quality(text, text);
        assert!((result.f1_score_text - 1.0).abs() < 0.001);
        assert!((result.quality_score - 1.0).abs() < 0.01);
    }

    #[test]
    fn test_completely_different() {
        let result = compute_quality("alpha beta gamma", "one two three");
        assert_eq!(result.f1_score_text, 0.0);
    }

    #[test]
    fn test_partial_overlap() {
        let result = compute_quality("hello world foo", "hello world bar");
        assert!((result.f1_score_text - 2.0 / 3.0).abs() < 0.001);
    }

    #[test]
    fn test_numeric_scoring() {
        let result = compute_quality("page 42 section 7", "page 42 section 7");
        assert!((result.f1_score_numeric - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_empty_inputs() {
        let result = compute_quality("", "");
        assert!((result.f1_score_text - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_empty_extracted() {
        let result = compute_quality("", "some ground truth");
        assert_eq!(result.f1_score_text, 0.0);
    }

    #[test]
    fn test_punctuation_stripped() {
        let result = compute_quality("hello, world!", "hello world");
        assert!((result.f1_score_text - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_case_insensitive() {
        let result = compute_quality("Hello World", "hello world");
        assert!((result.f1_score_text - 1.0).abs() < 0.001);
    }

    #[test]
    fn should_penalize_only_changed_boundaries_in_reordered_japanese_ocr_lines() {
        let ground_truth = concat!(
            "元来日本語は漢文に倣い、文字を上から下へ、また行を右から左へと進めて表記を行っていた。",
            "漢字と仮名の筆順も縦書きを前提としており、横書き不能な書体も存在する。"
        );
        let extracted = concat!(
            "から下へまた行を右から左へと進\n",
            "の筆順も縦書きを前提としており、\n",
            "めて表記を行っていた。漢字と仮名\n",
            "横書き不能な書体も存在する。\n",
            "元来日本語は漢文に倣い、文字を上"
        );

        let result = compute_quality(extracted, ground_truth);

        assert_eq!(result.f1_score_text, 0.9444444444444444);
        assert_eq!(result.quality_score, 0.9444444444444444);
        assert_eq!(result.extra_tokens.len(), 4);
        assert!(!result.correct);
    }

    #[test]
    fn should_not_award_partial_credit_for_unrelated_cjk_strings() {
        let result = compute_quality("日本語の文書", "本日の歴史");

        assert_eq!(result.f1_score_text, 0.0);
    }

    #[test]
    fn should_use_cjk_bigrams_with_single_character_fallback() {
        assert_eq!(tokenize("日本語"), vec!["日本", "本語"]);
        assert_eq!(tokenize("日"), vec!["日"]);
    }

    #[test]
    fn should_ignore_line_wrapping_between_cjk_characters() {
        let continuous = tokenize("日本語の文書");
        let wrapped = tokenize("日本\n語の\n文書");

        assert_eq!(wrapped, continuous);
        assert_eq!(compute_quality("日本\n語の\n文書", "日本語の文書").f1_score_text, 1.0);
    }

    #[test]
    fn should_ignore_decorative_punctuation_between_cjk_characters() {
        assert_eq!(tokenize("日本,語"), tokenize("日本語"));
        assert_eq!(tokenize("日本。語"), tokenize("日本語"));
    }

    #[test]
    fn should_tokenize_cjk_around_embedded_latin_and_numeric_runs() {
        assert_eq!(
            tokenize("日本語HS令和5年"),
            vec!["日本", "本語", "hs", "令和", "5", "年"]
        );
    }

    #[test]
    fn should_keep_iteration_marks_inside_whitespace_free_japanese_bigrams() {
        assert_eq!(tokenize("時々刻々"), vec!["時々", "々刻", "刻々"]);
    }

    #[test]
    fn should_preserve_latin_whitespace_tokenization() {
        assert_eq!(tokenize("Hello, world! 15.0"), vec!["hello", "world", "15"]);
    }

    #[test]
    fn test_tokenize_number_normalization() {
        let tokens_a = tokenize("15.0");
        let tokens_b = tokenize("15");
        assert_eq!(tokens_a, tokens_b, "15.0 and 15 should normalize to the same token");
        assert_eq!(tokens_a, vec!["15"]);

        assert_eq!(tokenize("100.00"), vec!["100"]);
    }

    #[test]
    fn test_compute_f1_number_equivalence() {
        let extracted = tokenize("price 15.0 dollars");
        let truth = tokenize("price 15 dollars");
        let f1 = compute_f1(&extracted, &truth);
        assert!(
            (f1 - 1.0).abs() < 0.001,
            "F1 should be 1.0 for semantically equivalent numeric tokens, got {f1}"
        );
    }

    #[test]
    fn test_tokenize_preserves_decimals() {
        assert_eq!(tokenize("3.14"), vec!["3.14"]);
        assert_eq!(tokenize("0.5"), vec!["0.5"]);
        assert_eq!(tokenize("12.345"), vec!["12.345"]);
    }

    #[test]
    fn test_no_numbers_no_boost() {
        let result = compute_quality("hello world foo", "hello world bar");
        let expected_text_f1 = 2.0 / 3.0;
        assert!(
            (result.f1_score_text - expected_text_f1).abs() < 0.001,
            "text F1 should be 2/3, got {}",
            result.f1_score_text
        );
        assert!(
            (result.quality_score - expected_text_f1).abs() < 0.001,
            "quality_score should equal text F1 ({expected_text_f1}) when no numbers, got {}",
            result.quality_score
        );
    }

    #[test]
    fn test_url_stripped_from_tokens() {
        let tokens = tokenize("[link text](https://example.com)");
        assert_eq!(tokens, vec!["link", "text"]);

        let tokens = tokenize("![alt text](https://example.com/image.png)");
        assert_eq!(tokens, vec!["alt", "text"]);

        let tokens = tokenize("See [docs](https://example.com/docs) for details");
        assert_eq!(tokens, vec!["see", "docs", "for", "details"]);
    }

    #[test]
    fn test_large_number_preserved() {
        let tokens = tokenize("10000000000000001");
        assert_eq!(
            tokens,
            vec!["10000000000000001"],
            "17-digit number should be preserved as-is, not rounded by f64"
        );

        let tokens = tokenize("12345678901234.0");
        assert_eq!(
            tokens,
            vec!["12345678901234"],
            "15-digit number with trailing .0 should still normalize"
        );
    }

    #[test]
    fn test_thousands_separators_normalize_to_bare_number() {
        // "1,000" and "1000" must tokenize identically (previously "1,000" failed f64 parse). ~keep
        assert_eq!(tokenize("1,000"), tokenize("1000"));
        assert_eq!(tokenize("12,345,678"), tokenize("12345678"));
        assert_eq!(tokenize("1,234.56"), tokenize("1234.56"));
        // A European-decimal comma (2-digit group) must NOT be treated as a thousands separator. ~keep
        assert_eq!(tokenize("3,14"), vec!["3,14"]);
    }

    #[test]
    fn structured_quality_uses_canonical_sf1() {
        let extracted = "# Title\n\nParagraph.\n\n- first\n- second";
        let ground_truth = "## Title\n\nParagraph.\n\n1. first\n2. second";
        let expected = structural_sidecar::score_markdown(extracted, ground_truth).sf1;

        let metrics = compute_quality_with_structure(
            extracted,
            "Title Paragraph first second",
            Some(ground_truth),
            OutputFormat::Markdown,
        );

        assert_eq!(metrics.f1_score_layout, Some(expected));
    }

    #[test]
    fn edit_distance_counts_single_edits() {
        let chars = |s: &str| s.chars().collect::<Vec<_>>();
        assert_eq!(edit_distance(&chars("kitten"), &chars("sitting")), 3);
        assert_eq!(edit_distance(&chars(""), &chars("abc")), 3);
        assert_eq!(edit_distance(&chars("abc"), &chars("abc")), 0);
    }

    #[test]
    fn normalized_edit_similarity_is_order_sensitive() {
        assert!((normalized_edit_similarity("hello world", "hello world") - 1.0).abs() < 1e-9);
        let sim = normalized_edit_similarity("ab", "ba");
        assert!(
            (0.0..1.0).contains(&sim),
            "transposition should lower similarity, got {sim}"
        );
        assert!((normalized_edit_similarity("abcdefghij", "abcdefghiX") - 0.9).abs() < 1e-9);
    }

    #[test]
    fn char_metrics_guard_empty_and_oversized_inputs() {
        assert!(char_error_rate("", "anything").is_nan(), "empty reference ⇒ NaN CER");
        assert!(
            normalized_edit_similarity("", "").is_nan(),
            "empty pair ⇒ NaN similarity"
        );
        let huge = "a".repeat(CER_MAX_CHARS + 1);
        assert!(
            normalized_edit_similarity(&huge, &huge).is_nan(),
            "oversized input is skipped"
        );
        assert!(
            (char_error_rate("abcd", "abXd") - 0.25).abs() < 1e-9,
            "one of four chars wrong ⇒ 0.25"
        );
    }
}
