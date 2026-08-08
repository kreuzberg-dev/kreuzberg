use crate::text::token_reduction::cjk_utils::CjkTokenizer;
use ahash::AHashMap;

use super::analysis::TextAnalyzer;

/// Handles word filtering and token removal operations.
pub struct WordFilter {
    cjk_tokenizer: CjkTokenizer,
    /// Mirrors `TokenReductionConfig::preserve_important_words` (#269): when
    /// `true`, words with "important" characteristics (all-caps, digits,
    /// mixed case, very long) are always kept by
    /// [`Self::remove_additional_common_words`] regardless of frequency;
    /// when `false`, those words are judged by the same frequency/length
    /// heuristics as everything else.
    preserve_important_words: bool,
}

impl WordFilter {
    pub(crate) fn new(preserve_important_words: bool) -> Self {
        Self {
            cjk_tokenizer: CjkTokenizer::new(),
            preserve_important_words,
        }
    }

    /// Removes additional common words based on frequency and characteristics.
    pub(crate) fn remove_additional_common_words(&self, text: &str) -> String {
        let words = self.universal_tokenize(text);

        if words.len() < 4 {
            return text.to_string();
        }

        let estimated_unique = (words.len() as f32 * 0.7).ceil() as usize;
        let mut word_freq = AHashMap::with_capacity(estimated_unique);

        let mut word_lengths = Vec::with_capacity(words.len());

        for word in &words {
            let clean_word = if word.chars().all(|c| c.is_alphabetic()) {
                word.to_lowercase()
            } else {
                word.chars()
                    .filter(|c| c.is_alphabetic())
                    .collect::<String>()
                    .to_lowercase()
            };

            if !clean_word.is_empty() {
                *word_freq.entry(clean_word.clone()).or_insert(0) += 1;
                word_lengths.push(clean_word.chars().count());
            }
        }

        let avg_length = if !word_lengths.is_empty() {
            word_lengths.iter().sum::<usize>() as f32 / word_lengths.len() as f32
        } else {
            5.0
        };

        let original_count = words.len();
        let has_cjk_content = text.chars().any(|c| c as u32 >= 0x4E00 && (c as u32) <= 0x9FFF);

        let mut filtered_words = Vec::with_capacity(words.len());
        for word in &words {
            let clean_word = if word.chars().all(|c| c.is_alphabetic()) {
                word.to_lowercase()
            } else {
                word.chars()
                    .filter(|c| c.is_alphabetic())
                    .collect::<String>()
                    .to_lowercase()
            };

            if clean_word.is_empty() {
                filtered_words.push(word.clone());
            } else {
                let freq = word_freq.get(&clean_word).unwrap_or(&0);
                let word_len = clean_word.chars().count() as f32;

                if (self.preserve_important_words && TextAnalyzer::has_important_characteristics(word))
                    || (*freq <= 2 && word_len >= avg_length * 0.8)
                    || (word_len >= avg_length * 1.5)
                {
                    filtered_words.push(word.clone());
                }
            }
        }

        let fallback_threshold = if has_cjk_content {
            original_count / 5
        } else {
            original_count / 3
        };

        if filtered_words.len() < fallback_threshold {
            let mut fallback_words = Vec::with_capacity(words.len());
            for word in &words {
                let clean_word = if word.chars().all(|c| c.is_alphabetic()) {
                    word.to_lowercase()
                } else {
                    word.chars().filter(|c| c.is_alphabetic()).collect::<String>()
                };

                if clean_word.is_empty()
                    || clean_word.chars().count() >= 3
                    || (self.preserve_important_words && TextAnalyzer::has_important_characteristics(word))
                {
                    fallback_words.push(word.clone());
                }
            }
            self.smart_join(&fallback_words, has_cjk_content)
        } else {
            self.smart_join(&filtered_words, has_cjk_content)
        }
    }

    /// Smart joins tokens based on language type (CJK vs. other).
    pub(crate) fn smart_join(&self, tokens: &[String], has_cjk_content: bool) -> String {
        if has_cjk_content {
            tokens.join("")
        } else {
            tokens.join(" ")
        }
    }

    /// Universal tokenizer that handles both CJK and non-CJK text.
    pub(crate) fn universal_tokenize(&self, text: &str) -> Vec<String> {
        self.cjk_tokenizer.tokenize_mixed_text(text)
    }
}

impl Default for WordFilter {
    fn default() -> Self {
        Self::new(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_universal_tokenize_english() {
        let filter = WordFilter::new(true);
        let tokens = filter.universal_tokenize("hello world test");
        assert_eq!(tokens, vec!["hello", "world", "test"]);
    }

    #[test]
    fn test_universal_tokenize_cjk() {
        let filter = WordFilter::new(true);
        let tokens = filter.universal_tokenize("中文");
        assert!(!tokens.is_empty());
    }

    #[test]
    fn test_fallback_threshold() {
        let filter = WordFilter::new(true);
        let input = "a the is of to in for on at by";
        let result = filter.remove_additional_common_words(input);
        assert!(!result.is_empty());
    }

    /// Shared fixture for the `preserve_important_words` tests below.
    ///
    /// Fifteen distinct long words (each appearing once) keep
    /// `filtered_words` comfortably above the fallback threshold on their
    /// own — via the frequency/length heuristic, independent of
    /// `preserve_important_words` — so the fallback path (which would mask
    /// the effect under test by keeping every word of length >= 3) never
    /// triggers. "CEO" (all-caps, len 3) repeats three times and is short
    /// relative to the average word length, so neither of those heuristics
    /// keeps it: only `has_important_characteristics` does (#269).
    const IMPORTANT_WORD_FIXTURE: &str = "documentation implementation specification organization architecture \
         infrastructure optimization configuration authentication authorization synchronization virtualization \
         containerization orchestration serialization CEO CEO CEO";

    #[test]
    fn preserve_important_words_true_keeps_the_acronym() {
        let filter = WordFilter::new(true);
        let result = filter.remove_additional_common_words(IMPORTANT_WORD_FIXTURE);
        assert!(result.contains("CEO"), "expected CEO to survive, got: {result}");
    }

    #[test]
    fn preserve_important_words_false_lets_the_acronym_be_removed() {
        let filter = WordFilter::new(false);
        let result = filter.remove_additional_common_words(IMPORTANT_WORD_FIXTURE);
        assert!(
            !result.contains("CEO"),
            "expected CEO to be removed once the important-word guard is disabled, got: {result}"
        );
    }
}
