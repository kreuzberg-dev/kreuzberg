use std::collections::VecDeque;

use ahash::{AHashMap, AHashSet};
use indexmap::IndexMap;

use self::context::Contexts;
use self::plural_helper::PluralHelper;
use self::preprocessor::{split_into_sentences, split_into_words};
use self::result_item::{ResultItem, remove_duplicates};
use self::stats::{OnlineStats, median};
use self::tag::{Tag, build_punctuation_table};

use super::config::KeywordConfig;
use super::types::{Keyword, KeywordAlgorithm};
use crate::Result;

mod context;
mod counter;
mod plural_helper;
mod preprocessor;
mod result_item;
mod stats;
mod tag;

/// Default punctuation character set (matching original YAKE).
const DEFAULT_PUNCTUATION: &str = r##"!"#$%&'()*+,-./:,<=>?@[\]^_`{|}~"##;

type Sentences = Vec<Sentence>;
type Candidates<'s> = IndexMap<&'s [String], Candidate<'s>>;
type Features<'s> = AHashMap<&'s str, TermScore>;
type Words<'s> = AHashMap<&'s str, Vec<Occurrence<'s>>>;

#[derive(PartialEq, Eq, Hash, Debug)]
struct Occurrence<'s> {
    sentence_idx: usize,
    word: &'s str,
    tag: Tag,
}

#[derive(Debug, Default)]
struct TermScore {
    tf: f64,
    score: f64,
}

#[derive(Debug, Default)]
struct TermStats {
    tf: f64,
    tf_a: f64,
    tf_n: f64,
    casing: f64,
    position: f64,
    frequency: f64,
    relatedness: f64,
    sentences: f64,
    score: f64,
}

impl From<TermStats> for TermScore {
    fn from(s: TermStats) -> Self {
        Self {
            tf: s.tf,
            score: s.score,
        }
    }
}

#[derive(Debug, Clone)]
struct Sentence {
    words: Vec<String>,
    lc_terms: Vec<String>,
    uq_terms: Vec<String>,
    tags: Vec<Tag>,
}

#[derive(Debug, Default, Clone)]
struct Candidate<'s> {
    occurrences: usize,
    raw: &'s [String],
    lc_terms: &'s [String],
    uq_terms: &'s [String],
    score: f64,
}

/// YAKE configuration (internal to vendored code).
#[derive(Debug, Clone)]
struct YakeConfig {
    ngrams: usize,
    punctuation_table: [u8; 256],
    window_size: usize,
    strict_capital: bool,
    only_alphanumeric_and_hyphen: bool,
    minimum_chars: usize,
    remove_duplicates: bool,
    deduplication_threshold: f64,
}

impl Default for YakeConfig {
    fn default() -> Self {
        Self {
            ngrams: 3,
            punctuation_table: build_punctuation_table(DEFAULT_PUNCTUATION),
            window_size: 1,
            strict_capital: true,
            only_alphanumeric_and_hyphen: false,
            minimum_chars: 3,
            remove_duplicates: true,
            deduplication_threshold: 0.9,
        }
    }
}

struct Yake<'a> {
    config: YakeConfig,
    stopwords: &'a AHashSet<String>,
}

impl<'a> Yake<'a> {
    fn get_n_best(&self, text: &str, n: usize) -> Vec<ResultItem> {
        let sentences = self.preprocess_text(text);
        if sentences.is_empty() {
            return Vec::new();
        }

        let (context, vocabulary) = self.build_context_and_vocabulary(&sentences);
        let features = self.extract_features(&context, vocabulary, &sentences);

        let mut ngrams: Candidates = self.ngram_selection(self.config.ngrams, &sentences);
        self.candidate_weighting(&features, &context, &mut ngrams);

        let mut results: Vec<ResultItem> = ngrams.into_values().map(Into::into).collect();
        results.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap_or(std::cmp::Ordering::Equal));

        if self.config.remove_duplicates {
            remove_duplicates(self.config.deduplication_threshold, results, n)
        } else {
            results.truncate(n);
            results
        }
    }

    #[inline]
    fn get_unique_term(&self, word: &str) -> String {
        word.to_single().to_lowercase()
    }

    #[inline]
    fn is_stopword(&self, lc_term: &str) -> bool {
        self.stopwords.contains(lc_term)
            || self.stopwords.contains(lc_term.to_single())
            || lc_term
                .to_single()
                .bytes()
                .filter(|&b| self.config.punctuation_table[b as usize] == 0)
                .count()
                < 3
    }

    fn preprocess_text(&self, text: &str) -> Sentences {
        split_into_sentences(text)
            .into_iter()
            .map(|sentence| {
                let words = split_into_words(&sentence);
                let lc_terms: Vec<String> = words.iter().map(|w| w.to_lowercase()).collect();
                let uq_terms: Vec<String> = lc_terms.iter().map(|w| self.get_unique_term(w)).collect();
                let tags: Vec<Tag> = words
                    .iter()
                    .enumerate()
                    .map(|(idx, w)| {
                        Tag::classify(w, idx == 0, &self.config.punctuation_table, self.config.strict_capital)
                    })
                    .collect();
                Sentence {
                    words,
                    lc_terms,
                    uq_terms,
                    tags,
                }
            })
            .collect()
    }

    fn build_context_and_vocabulary<'s>(&self, sentences: &'s [Sentence]) -> (Contexts<'s>, Words<'s>) {
        let mut ctx = Contexts::default();
        let mut words = Words::default();

        for (idx, sentence) in sentences.iter().enumerate() {
            let mut window: VecDeque<(&str, Tag)> = VecDeque::with_capacity(self.config.window_size + 1);

            for ((word, term), &tag) in sentence.words.iter().zip(&sentence.uq_terms).zip(&sentence.tags) {
                if tag == Tag::Punctuation {
                    window.clear();
                    continue;
                }

                let occurrence = Occurrence {
                    sentence_idx: idx,
                    word: word.as_str(),
                    tag,
                };
                words.entry(term.as_str()).or_default().push(occurrence);

                if tag != Tag::Digit && tag != Tag::Unparsable {
                    for &(left_uterm, left_tag) in window.iter() {
                        if left_tag == Tag::Digit || left_tag == Tag::Unparsable {
                            continue;
                        }
                        ctx.track(left_uterm, term.as_str());
                    }
                }

                if window.len() == self.config.window_size {
                    window.pop_front();
                }
                window.push_back((term.as_str(), tag));
            }
        }

        (ctx, words)
    }

    fn extract_features<'s>(&self, ctx: &Contexts, words: Words<'s>, sentences: &'s Sentences) -> Features<'s> {
        let candidate_words: AHashMap<&str, &str> = sentences
            .iter()
            .flat_map(|s| s.lc_terms.iter().zip(&s.uq_terms).zip(&s.tags))
            .filter(|&(_, &tag)| tag != Tag::Punctuation)
            .map(|((lc, uq), _)| (lc.as_str(), uq.as_str()))
            .collect();

        let non_stop_words: AHashMap<&str, usize> = candidate_words
            .iter()
            .filter(|&(lc, _)| !self.is_stopword(lc))
            .map(|(_, &uq)| {
                let occurrences = words.get(uq).map_or(0, Vec::len);
                (uq, occurrences)
            })
            .collect();

        let (nsw_tf_std, nsw_tf_mean) = {
            let tfs: OnlineStats = non_stop_words.values().map(|&freq| freq as f64).collect();
            (tfs.stddev(), tfs.mean())
        };

        let max_tf = words.values().map(Vec::len).max().unwrap_or(0) as f64;

        let mut features = Features::default();

        for (_, u_term) in &candidate_words {
            let occurrences = match words.get(u_term) {
                Some(o) => o,
                None => continue,
            };
            let mut stats = TermStats {
                tf: occurrences.len() as f64,
                ..Default::default()
            };

            stats.tf_a = occurrences.iter().filter(|occ| occ.tag == Tag::Acronym).count() as f64;
            stats.tf_n = occurrences.iter().filter(|occ| occ.tag == Tag::Uppercase).count() as f64;
            stats.casing = stats.tf_a.max(stats.tf_n);
            stats.casing /= 1.0 + stats.tf.ln();

            {
                let mut sentence_ids: Vec<f64> = occurrences.iter().map(|o| o.sentence_idx as f64).collect();
                sentence_ids.dedup();
                stats.position = 3.0 + median(sentence_ids.into_iter()).unwrap_or(0.0);
                stats.position = stats.position.ln().ln();
            }

            stats.frequency = stats.tf;
            stats.frequency /= nsw_tf_mean + nsw_tf_std;

            {
                let (dl, dr) = ctx.dispersion_of(u_term);
                stats.relatedness = 1.0 + (dr + dl) * (stats.tf / max_tf);
            }

            {
                let mut ids: Vec<usize> = occurrences.iter().map(|o| o.sentence_idx).collect();
                ids.dedup();
                stats.sentences = ids.len() as f64 / sentences.len() as f64;
            }

            stats.score = (stats.relatedness * stats.position)
                / (stats.casing + (stats.frequency / stats.relatedness) + (stats.sentences / stats.relatedness));

            features.insert(u_term, stats.into());
        }

        features
    }

    fn candidate_weighting<'s>(&self, features: &Features<'s>, ctx: &Contexts<'s>, candidates: &mut Candidates<'s>) {
        for (&lc_terms, candidate) in candidates.iter_mut() {
            let uq_terms = candidate.uq_terms;
            let mut prod_ = 1.0_f64;
            let mut sum_ = 0.0_f64;

            for (j, (lc, uq)) in lc_terms.iter().zip(uq_terms).enumerate() {
                if self.is_stopword(lc) {
                    let prob_prev = if j == 0 {
                        0.0
                    } else {
                        match uq_terms.get(j - 1) {
                            None => 0.0,
                            Some(prev_uq) => {
                                let tf = features.get(prev_uq.as_str()).map_or(1.0, |f| f.tf);
                                ctx.cases_term_is_followed(prev_uq.as_str(), uq.as_str()) as f64 / tf
                            }
                        }
                    };

                    let prob_succ = match uq_terms.get(j + 1) {
                        None => 0.0,
                        Some(next_uq) => {
                            let tf = features.get(next_uq.as_str()).map_or(1.0, |f| f.tf);
                            ctx.cases_term_is_followed(uq.as_str(), next_uq.as_str()) as f64 / tf
                        }
                    };

                    let prob = prob_prev * prob_succ;
                    prod_ *= 1.0 + (1.0 - prob);
                    sum_ -= 1.0 - prob;
                } else if let Some(stats) = features.get(uq.as_str()) {
                    prod_ *= stats.score;
                    sum_ += stats.score;
                }
            }

            if sum_ == -1.0 {
                sum_ = 0.999999999;
            }

            let tf = candidate.occurrences as f64;
            candidate.score = prod_ / (tf * (1.0 + sum_));
        }
    }

    fn is_candidate(&self, lc_terms: &[String], tags: &[Tag]) -> bool {
        let is_bad = tags
            .iter()
            .any(|tag| matches!(tag, Tag::Digit | Tag::Punctuation | Tag::Unparsable))
            || self.is_stopword(lc_terms.last().unwrap())
            || lc_terms.iter().map(|w| w.chars().count()).sum::<usize>() < self.config.minimum_chars
            || self.config.only_alphanumeric_and_hyphen
                && !lc_terms
                    .iter()
                    .all(|w| w.chars().all(|ch| ch.is_alphanumeric() || ch == '-'));

        !is_bad
    }

    fn ngram_selection<'s>(&self, n: usize, sentences: &'s Sentences) -> Candidates<'s> {
        let mut candidates = Candidates::new();
        let mut ignored: AHashSet<&[String]> = AHashSet::new();

        for sentence in sentences.iter() {
            let length = sentence.words.len();

            for j in 0..length {
                if self.is_stopword(&sentence.lc_terms[j]) {
                    continue;
                }

                for k in (j + 1..length + 1).take(n) {
                    let lc_terms = &sentence.lc_terms[j..k];

                    if !ignored.contains(lc_terms) {
                        if !self.is_candidate(lc_terms, &sentence.tags[j..k]) {
                            ignored.insert(lc_terms);
                        } else {
                            candidates
                                .entry(lc_terms)
                                .or_insert_with(|| Candidate {
                                    lc_terms,
                                    uq_terms: &sentence.uq_terms[j..k],
                                    raw: &sentence.words[j..k],
                                    ..Default::default()
                                })
                                .occurrences += 1;
                        }
                    }
                }
            }
        }

        candidates
    }
}

/// Extract keywords using YAKE algorithm, integrated with xberg's stopwords.
pub(crate) fn extract_keywords_yake(text: &str, config: &KeywordConfig) -> Result<Vec<Keyword>> {
    let params = config.yake_params.as_ref().cloned().unwrap_or_default();

    let lang = config.language.as_deref().unwrap_or("en");
    let stopwords = crate::stopwords::get_stopwords_with_fallback(lang, "en").unwrap_or_else(|| {
        tracing::debug!(
            "Stopwords not available for language '{}', using default English stopwords",
            lang
        );
        static EMPTY: std::sync::LazyLock<AHashSet<String>> = std::sync::LazyLock::new(AHashSet::new);
        &EMPTY
    });

    let yake_config = YakeConfig {
        ngrams: config.ngram_range.1,
        window_size: params.window_size,
        ..YakeConfig::default()
    };

    let yake = Yake {
        config: yake_config,
        stopwords,
    };

    let results = yake.get_n_best(text, config.max_keywords);

    let mut keywords: Vec<Keyword> = results
        .into_iter()
        .filter(|item| {
            let word_count = item.keyword.split_whitespace().count();
            word_count >= config.ngram_range.0
        })
        .map(|item| {
            let normalized_score = if item.score > 0.0 {
                (1.0_f64 / (1.0 + item.score)).clamp(0.0, 1.0)
            } else {
                1.0
            };
            Keyword::with_positions(item.keyword, normalized_score as f32, KeywordAlgorithm::Yake, text)
        })
        .collect();

    if config.min_score > 0.0 {
        keywords.retain(|k| k.score >= config.min_score);
    }

    keywords.sort_by(|a, b| b.score.total_cmp(&a.score));

    Ok(keywords)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keywords::config::YakeParams;

    #[test]
    fn test_yake_extraction_basic() {
        let text = "Rust is a systems programming language. \
                    Rust provides memory safety and performance. \
                    Memory safety is achieved without garbage collection.";

        let config = KeywordConfig::yake();
        let keywords = extract_keywords_yake(text, &config).unwrap();

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
            assert_eq!(keyword.algorithm, KeywordAlgorithm::Yake);
        }
    }

    #[test]
    fn test_yake_extraction_populates_positions() {
        let text = "Rust is a systems programming language. \
                    Rust provides memory safety and performance.";

        let config = KeywordConfig::yake();
        let keywords = extract_keywords_yake(text, &config).unwrap();

        assert!(!keywords.is_empty(), "Should extract keywords");
        let rust_keyword = keywords
            .iter()
            .find(|k| k.text.eq_ignore_ascii_case("rust"))
            .expect("'rust' should be an extracted keyword");
        assert_eq!(rust_keyword.positions, Some(vec![0, 40]));
    }

    #[test]
    fn test_yake_extraction_with_min_score() {
        let text = "Rust programming language provides memory safety without garbage collection.";
        let config = KeywordConfig::yake().with_min_score(0.5);
        let keywords = extract_keywords_yake(text, &config).unwrap();

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
    fn test_yake_extraction_with_ngram_range() {
        let text = "Machine learning models require large datasets for training.";
        let config = KeywordConfig::yake().with_ngram_range(1, 1);
        let keywords = extract_keywords_yake(text, &config).unwrap();

        for keyword in &keywords {
            assert_eq!(
                keyword.text.split_whitespace().count(),
                1,
                "Should only extract unigrams"
            );
        }
    }

    #[test]
    fn test_yake_extraction_empty_text() {
        let config = KeywordConfig::yake();
        let keywords = extract_keywords_yake("", &config).unwrap();
        assert!(keywords.is_empty(), "Empty text should yield no keywords");
    }

    #[test]
    fn test_yake_extraction_with_custom_params() {
        let text = "Natural language processing enables computers to understand human language.";
        let params = YakeParams { window_size: 3 };
        let config = KeywordConfig::yake().with_yake_params(params);
        let keywords = extract_keywords_yake(text, &config).unwrap();
        assert!(!keywords.is_empty(), "Should extract keywords with custom params");
    }

    #[test]
    fn test_large_input_no_panic() {
        let paragraph = "Artificial intelligence and machine learning are transforming industries worldwide. Companies are investing heavily in AI research and development. Natural language processing enables new applications. ";
        let large_text = paragraph.repeat(50_000);
        let config = KeywordConfig::yake().with_max_keywords(10);
        let keywords = extract_keywords_yake(&large_text, &config).unwrap();
        assert!(!keywords.is_empty(), "Large input should produce keywords");
    }
}
