//! RAKE (Rapid Automatic Keyword Extraction) backend implementation.

use super::config::{KeywordConfig, RakeParams};
use super::types::{Keyword, KeywordAlgorithm};
use crate::Result;
use crate::stopwords::{STOPWORDS, get_stopwords};
use rake::*;
use std::collections::HashSet;

/// Extract keywords using RAKE algorithm.
///
/// RAKE is a co-occurrence based keyword extraction method that:
/// - Identifies candidate keywords using stop words as delimiters
/// - Calculates word scores based on frequency and degree
/// - Combines scores for multi-word phrases
///
/// # Arguments
///
/// * `text` - The text to extract keywords from
/// * `config` - Keyword extraction configuration
///
/// # Returns
///
/// A vector of keywords sorted by relevance (highest score first).
///
/// # Errors
///
/// Returns an error if keyword extraction fails.
pub(crate) fn extract_keywords_rake(text: &str, config: &KeywordConfig) -> Result<Vec<Keyword>> {
    let params = config.rake_params.as_ref().cloned().unwrap_or_default();
    let lang = config.language.as_deref().unwrap_or("en");
    let normalized_lang = normalize_language_code(lang);

    let mut keywords = run_rake_with_stopwords(text, config, &params, stopwords_for_language(lang));

    if keywords.is_empty() && normalized_lang != "en" {
        keywords = run_rake_with_stopwords(text, config, &params, stopwords_for_language("en"));
    }

    Ok(keywords)
}

fn stopwords_for_language(lang: &str) -> StopWords {
    if let Some(words) = get_stopwords(lang) {
        let set: HashSet<String> = words.iter().cloned().collect();
        return StopWords::from(set);
    }

    let english: HashSet<String> = STOPWORDS
        .get("en")
        .map(|set| set.iter().cloned().collect())
        .unwrap_or_default();
    StopWords::from(english)
}

fn run_rake_with_stopwords(
    text: &str,
    config: &KeywordConfig,
    params: &RakeParams,
    stopwords: StopWords,
) -> Vec<Keyword> {
    let rake = Rake::new(stopwords);
    let results = rake.run(text);

    let filtered_results: Vec<_> = results
        .into_iter()
        .filter_map(|keyword_score| {
            let keyword = keyword_score.keyword.clone();

            if keyword.len() < params.min_word_length {
                return None;
            }

            let word_count = keyword.split_whitespace().count();
            if word_count > params.max_words_per_phrase {
                return None;
            }

            if word_count < config.ngram_range.0 || word_count > config.ngram_range.1 {
                return None;
            }

            Some((keyword, keyword_score.score))
        })
        .collect();

    finalize_rake_keywords(filtered_results, config, text)
}

/// Normalise raw RAKE scores, populate positions, apply `min_score`
/// filtering, sort, and truncate to `max_keywords`.
///
/// Scores are scaled relative to the strongest candidate in `filtered_results`
/// (`raw_score / max_score`), so the top keyword lands at ~1.0. Deliberately
/// NOT min-max normalisation: rescaling against both the min and max forces
/// the single lowest-ranked keyword's score to exactly 0.0, which any
/// positive `config.min_score` threshold then unconditionally deletes
/// regardless of how relevant that keyword actually was (#266). RAKE scores
/// are always positive (each word contributes at least 1.0 of
/// degree/frequency), so `max_score` is always > 0.0 for a non-empty
/// `filtered_results`.
fn finalize_rake_keywords(
    filtered_results: Vec<(String, f64)>,
    config: &KeywordConfig,
    source_text: &str,
) -> Vec<Keyword> {
    if filtered_results.is_empty() {
        return Vec::new();
    }

    let max_score = filtered_results
        .iter()
        .map(|(_, s)| *s)
        .fold(f64::NEG_INFINITY, f64::max);

    let mut keywords: Vec<_> = filtered_results
        .into_iter()
        .map(|(keyword, raw_score)| {
            let normalized_score = if max_score > 0.0 {
                (raw_score / max_score).clamp(0.0, 1.0)
            } else {
                0.0
            };

            Keyword::with_positions(keyword, normalized_score as f32, KeywordAlgorithm::Rake, source_text)
        })
        .collect();

    if config.min_score > 0.0 {
        keywords.retain(|k| k.score >= config.min_score);
    }

    keywords.sort_by(|a, b| b.score.total_cmp(&a.score));

    keywords.truncate(config.max_keywords);

    keywords
}

fn normalize_language_code(lang: &str) -> String {
    let lower = lang.to_lowercase();
    if let Some(pos) = lower.find(['-', '_']) {
        lower[..pos].to_string()
    } else if lower.len() >= 2 {
        lower[..2].to_string()
    } else {
        lower
    }
}

#[cfg(all(test, feature = "keywords"))]
mod tests {
    use super::*;
    use crate::keywords::config::RakeParams;

    #[test]
    fn finalize_rake_keywords_normalizes_relative_to_the_max_score() {
        let filtered = vec![
            ("alpha".to_string(), 10.0),
            ("beta".to_string(), 9.0),
            ("gamma".to_string(), 8.0),
        ];
        let config = KeywordConfig::rake();

        let keywords = finalize_rake_keywords(filtered, &config, "alpha beta gamma");

        assert_eq!(keywords.len(), 3);
        assert_eq!(keywords[0].text, "alpha");
        assert_eq!(keywords[0].score, 1.0);
        assert_eq!(keywords[1].text, "beta");
        assert_eq!(keywords[1].score, 0.9);
        assert_eq!(keywords[2].text, "gamma");
        assert_eq!(keywords[2].score, 0.8);
    }

    #[test]
    fn finalize_rake_keywords_does_not_annihilate_the_lowest_ranked_keyword() {
        // Before the fix, min-max normalisation forced the lowest-ranked
        // keyword's score to exactly 0.0 regardless of how close its raw
        // score was to the top pick, so any positive `min_score` always
        // deleted it. Here "gamma" (raw 8.0) is 80% as strong as "alpha"
        // (raw 10.0) and must survive a 0.3 threshold (#266).
        let filtered = vec![
            ("alpha".to_string(), 10.0),
            ("beta".to_string(), 9.0),
            ("gamma".to_string(), 8.0),
        ];
        let config = KeywordConfig::rake().with_min_score(0.3);

        let keywords = finalize_rake_keywords(filtered, &config, "alpha beta gamma");

        let texts: Vec<&str> = keywords.iter().map(|k| k.text.as_str()).collect();
        assert_eq!(texts, vec!["alpha", "beta", "gamma"], "gamma must not be annihilated");
        assert_eq!(keywords[2].score, 0.8);
    }

    #[test]
    fn finalize_rake_keywords_still_filters_genuinely_weak_keywords() {
        let filtered = vec![("alpha".to_string(), 10.0), ("weak".to_string(), 1.0)];
        let config = KeywordConfig::rake().with_min_score(0.5);

        let keywords = finalize_rake_keywords(filtered, &config, "alpha weak");

        assert_eq!(keywords.len(), 1);
        assert_eq!(keywords[0].text, "alpha");
    }

    #[test]
    fn finalize_rake_keywords_populates_positions_from_source_text() {
        let filtered = vec![("learning".to_string(), 4.0)];
        let config = KeywordConfig::rake();

        let keywords = finalize_rake_keywords(filtered, &config, "machine learning is great");

        assert_eq!(keywords.len(), 1);
        assert_eq!(keywords[0].positions, Some(vec![8]));
    }

    #[test]
    fn finalize_rake_keywords_returns_empty_for_empty_input() {
        let config = KeywordConfig::rake();
        assert!(finalize_rake_keywords(Vec::new(), &config, "irrelevant").is_empty());
    }

    #[test]
    fn test_rake_extraction_basic() {
        let text = "Rust is a systems programming language. \
                    Rust provides memory safety and performance. \
                    Memory safety is achieved without garbage collection.";

        let config = KeywordConfig::rake();

        let keywords = extract_keywords_rake(text, &config).unwrap();

        assert!(!keywords.is_empty(), "Should extract keywords");
        assert!(
            keywords.len() <= config.max_keywords,
            "Should respect max_keywords limit"
        );

        for i in 1..keywords.len() {
            assert!(
                keywords[i - 1].score >= keywords[i].score,
                "Keywords should be sorted by score"
            );
        }

        for keyword in &keywords {
            assert_eq!(keyword.algorithm, KeywordAlgorithm::Rake);
        }
    }

    #[test]
    fn test_rake_extraction_with_min_score() {
        let text = "Rust programming language provides memory safety without garbage collection.";

        let config = KeywordConfig::rake().with_min_score(0.3);

        let keywords = extract_keywords_rake(text, &config).unwrap();

        for keyword in &keywords {
            assert!(
                keyword.score >= config.min_score,
                "Keyword score {} should be >= min_score {}",
                keyword.score,
                config.min_score
            );
        }
    }

    #[test]
    fn test_rake_extraction_with_ngram_range() {
        let text = "Machine learning models require large datasets for training.";

        let config = KeywordConfig::rake().with_ngram_range(1, 1);
        let keywords = extract_keywords_rake(text, &config).unwrap();

        for keyword in &keywords {
            assert_eq!(
                keyword.text.split_whitespace().count(),
                1,
                "Should only extract unigrams"
            );
        }
    }

    #[test]
    fn test_rake_extraction_empty_text() {
        let config = KeywordConfig::rake();
        let keywords = extract_keywords_rake("", &config).unwrap();
        assert!(keywords.is_empty(), "Empty text should yield no keywords");
    }

    #[test]
    fn test_rake_extraction_with_custom_params() {
        let text = "Natural language processing enables computers to understand human language.";

        let params = RakeParams {
            min_word_length: 3,
            max_words_per_phrase: 2,
        };

        let config = KeywordConfig::rake().with_rake_params(params);

        let keywords = extract_keywords_rake(text, &config).unwrap();

        for keyword in &keywords {
            for word in keyword.text.split_whitespace() {
                assert!(word.len() >= 3, "Word '{}' should have min length 3", word);
            }
        }

        for keyword in &keywords {
            assert!(
                keyword.text.split_whitespace().count() <= 2,
                "Keyword '{}' should have max 2 words",
                keyword.text
            );
        }
    }

    #[cfg_attr(coverage, ignore = "coverage instrumentation interferes with stopword heuristics")]
    #[test]
    fn test_rake_multilingual() {
        let spanish_text = "El idioma español es una lengua romance.";
        let config = KeywordConfig::rake().with_language("es");
        let keywords = extract_keywords_rake(spanish_text, &config).unwrap();
        assert!(!keywords.is_empty(), "Should extract Spanish keywords");

        assert!(
            keywords
                .iter()
                .any(|k| k.text.contains("idioma") || k.text.contains("español") || k.text.contains("lengua"))
        );

        let english_text = "Natural language processing is a subfield of artificial intelligence.";
        let config = KeywordConfig::rake().with_language("fr");
        let keywords = extract_keywords_rake(english_text, &config).unwrap();
        assert!(
            !keywords.is_empty(),
            "Should fall back to English stopwords and extract keywords"
        );
    }

    #[test]
    fn test_rake_score_normalization() {
        let text = "Rust is a systems programming language that provides memory safety and \
                    thread safety without garbage collection. Rust uses a ownership system \
                    with rules that the compiler checks at compile time.";

        let config = KeywordConfig::rake();
        let keywords = extract_keywords_rake(text, &config).unwrap();

        assert!(!keywords.is_empty(), "Should extract keywords");

        for keyword in &keywords {
            assert!(
                keyword.score >= 0.0 && keyword.score <= 1.0,
                "Keyword '{}' score {} should be in range [0.0, 1.0]",
                keyword.text,
                keyword.score
            );
        }

        if !keywords.is_empty() {
            let max_score = keywords.iter().map(|k| k.score).fold(0.0f32, f32::max);
            assert!(
                (max_score - 1.0).abs() < 0.001,
                "Max score should be 1.0, got {}",
                max_score
            );
        }

        for i in 1..keywords.len() {
            assert!(
                keywords[i - 1].score >= keywords[i].score,
                "Keywords should be sorted by score"
            );
        }
    }
}
