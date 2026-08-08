use crate::error::Result;
use crate::text::token_reduction::{
    config::{ReductionLevel, TokenReductionConfig},
    filters::FilterPipeline,
    semantic::SemanticAnalyzer,
    simd_text::{SimdTextProcessor, chunk_text_for_parallel},
};
#[cfg(not(target_arch = "wasm32"))]
use rayon::prelude::*;
use std::sync::Arc;
use unicode_normalization::UnicodeNormalization;

use super::punctuation::PunctuationCleaner;
use super::sentence_selection::SentenceSelector;
use super::word_filtering::WordFilter;

/// Orchestrates the token-reduction pipeline: SIMD scanning, filter chain, and optional semantic clustering.
#[cfg_attr(alef, alef(skip))]
pub struct TokenReducer {
    config: Arc<TokenReductionConfig>,
    text_processor: SimdTextProcessor,
    filter_pipeline: FilterPipeline,
    semantic_analyzer: Option<SemanticAnalyzer>,
    word_filter: WordFilter,
}

impl TokenReducer {
    pub(crate) fn new(config: &TokenReductionConfig, language_hint: Option<&str>) -> Result<Self> {
        let config = Arc::new(config.clone());
        let language = language_hint
            .or(config.language_hint.as_deref())
            .unwrap_or("en")
            .to_string();

        let text_processor = SimdTextProcessor::new();
        let filter_pipeline = FilterPipeline::new(&config, &language)?;

        let semantic_analyzer = if matches!(config.level, ReductionLevel::Aggressive | ReductionLevel::Maximum) {
            Some(SemanticAnalyzer::new(&language))
        } else {
            None
        };

        let word_filter = WordFilter::new(config.preserve_important_words);

        Ok(Self {
            config,
            text_processor,
            filter_pipeline,
            semantic_analyzer,
            word_filter,
        })
    }

    pub(crate) fn reduce(&self, text: &str) -> String {
        if text.is_empty() || matches!(self.config.level, ReductionLevel::Off) {
            return text.to_string();
        }

        let nfc_string;
        let working_text = if text.is_ascii() {
            text
        } else {
            nfc_string = text.nfc().collect::<String>();
            &nfc_string
        };

        match self.config.level {
            ReductionLevel::Off => working_text.to_string(),
            ReductionLevel::Light => self.apply_light_reduction_optimized(working_text),
            ReductionLevel::Moderate => self.apply_moderate_reduction_optimized(working_text),
            ReductionLevel::Aggressive => self.apply_aggressive_reduction_optimized(working_text),
            ReductionLevel::Maximum => self.apply_maximum_reduction_optimized(working_text),
        }
    }

    #[cfg(test)]
    pub(crate) fn batch_reduce(&self, texts: &[String]) -> Vec<String> {
        if !self.config.enable_parallel || texts.len() < 2 {
            return texts.iter().map(|text| self.reduce(text)).collect();
        }

        #[cfg(not(target_arch = "wasm32"))]
        {
            texts.par_iter().map(|text| self.reduce(text.as_str())).collect()
        }
        #[cfg(target_arch = "wasm32")]
        {
            texts.iter().map(|text| self.reduce(text)).collect()
        }
    }

    fn apply_light_reduction_optimized(&self, text: &str) -> String {
        let mut result = if self.config.use_simd {
            self.text_processor.clean_punctuation(text)
        } else {
            PunctuationCleaner::clean_punctuation_optimized(text)
        };

        result = self.filter_pipeline.apply_light_filters(&result);
        result.trim().to_string()
    }

    fn apply_moderate_reduction_optimized(&self, text: &str) -> String {
        let mut result = self.apply_light_reduction_optimized(text);

        result = if self.config.enable_parallel && text.len() > 1000 {
            self.apply_parallel_moderate_reduction(&result)
        } else {
            self.filter_pipeline.apply_moderate_filters(&result)
        };

        result
    }

    fn apply_aggressive_reduction_optimized(&self, text: &str) -> String {
        let mut result = self.apply_moderate_reduction_optimized(text);

        result = self.word_filter.remove_additional_common_words(&result);
        result = SentenceSelector::apply_sentence_selection(&result);

        if let Some(ref analyzer) = self.semantic_analyzer {
            result = analyzer.apply_semantic_filtering(&result, self.config.semantic_threshold);
        }

        result
    }

    fn apply_maximum_reduction_optimized(&self, text: &str) -> String {
        let mut result = self.apply_aggressive_reduction_optimized(text);

        if let Some(ref analyzer) = self.semantic_analyzer
            && self.config.enable_semantic_clustering
        {
            result = analyzer.apply_hypernym_compression(&result, self.config.target_reduction);
        }

        result
    }

    fn apply_parallel_moderate_reduction(&self, text: &str) -> String {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let num_threads = rayon::current_num_threads();
            let chunks = chunk_text_for_parallel(text, num_threads);
            let processed_chunks: Vec<String> = chunks
                .par_iter()
                .map(|chunk| self.filter_pipeline.apply_moderate_filters(chunk))
                .collect();
            processed_chunks.join(" ")
        }
        #[cfg(target_arch = "wasm32")]
        {
            let chunks = chunk_text_for_parallel(text, 1);
            let processed_chunks: Vec<String> = chunks
                .iter()
                .map(|chunk| self.filter_pipeline.apply_moderate_filters(chunk))
                .collect();
            processed_chunks.join(" ")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_light_reduction() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Light,
            use_simd: false,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "Hello   world!!!   How are you???";
        let result = reducer.reduce(input);

        assert!(result.len() < input.len());
        assert!(!result.contains("   "));
    }

    #[test]
    fn test_moderate_reduction() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Moderate,
            use_simd: false,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, Some("en")).unwrap();
        let input = "The quick brown fox is jumping over the lazy dog";
        let result = reducer.reduce(input);

        assert!(result.len() < input.len());
        assert!(result.contains("quick"));
        assert!(result.contains("brown"));
        assert!(result.contains("fox"));
    }

    #[test]
    fn test_batch_processing() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Light,
            enable_parallel: false,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let inputs: Vec<String> = vec![
            "Hello  world!".to_string(),
            "How   are you?".to_string(),
            "Fine,  thanks!".to_string(),
        ];
        let results = reducer.batch_reduce(&inputs);

        assert_eq!(results.len(), inputs.len());
        for result in &results {
            assert!(!result.contains("  "));
        }
    }

    #[test]
    fn test_aggressive_reduction() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Aggressive,
            use_simd: false,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, Some("en")).unwrap();
        let input = "The quick brown fox is jumping over the lazy dog and running through the forest";
        let result = reducer.reduce(input);

        assert!(result.len() < input.len());
        assert!(!result.is_empty());
    }

    #[test]
    fn test_maximum_reduction() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Maximum,
            use_simd: false,
            enable_semantic_clustering: true,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, Some("en")).unwrap();
        let input = "The quick brown fox is jumping over the lazy dog and running through the forest";
        let result = reducer.reduce(input);

        assert!(result.len() < input.len());
        assert!(!result.is_empty());
    }

    #[test]
    fn test_empty_text_handling() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Moderate,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        assert_eq!(reducer.reduce(""), "");
        let result = reducer.reduce("   ");
        assert!(result == "   " || result.is_empty());
    }

    #[test]
    fn test_off_mode_preserves_text() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Off,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "Text   with    multiple   spaces!!!";
        assert_eq!(reducer.reduce(input), input);
    }

    #[test]
    fn test_parallel_batch_processing() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Light,
            enable_parallel: true,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let inputs: Vec<String> = vec![
            "First text  with spaces".to_string(),
            "Second  text with  spaces".to_string(),
            "Third   text  with spaces".to_string(),
        ];
        let results = reducer.batch_reduce(&inputs);

        assert_eq!(results.len(), inputs.len());
        for result in &results {
            assert!(!result.contains("  "));
        }
    }

    #[test]
    fn test_cjk_text_handling() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Moderate,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, Some("zh")).unwrap();
        let input = "这是中文文本测试";
        let result = reducer.reduce(input);

        assert!(!result.is_empty());
    }

    #[test]
    fn test_mixed_language_text() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Moderate,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "This is English text 这是中文 and some more English";
        let result = reducer.reduce(input);

        assert!(!result.is_empty());
        assert!(result.contains("English") || result.contains("中"));
    }

    #[test]
    fn test_unicode_normalization_ascii() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Light,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "Pure ASCII text without special characters";
        let result = reducer.reduce(input);

        assert!(result.contains("ASCII"));
    }

    #[test]
    fn test_unicode_normalization_non_ascii() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Light,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "Café naïve résumé";
        let result = reducer.reduce(input);

        assert!(result.contains("Café") || result.contains("Cafe"));
    }

    #[test]
    fn test_single_text_vs_batch() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Moderate,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let text = "The quick brown fox jumps over the lazy dog";

        let single_result = reducer.reduce(text);
        let batch_results = reducer.batch_reduce(&[text.to_string()]);

        assert_eq!(single_result, batch_results[0]);
    }

    #[test]
    fn test_important_word_preservation() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Aggressive,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "The IMPORTANT word COVID-19 and 12345 numbers should be preserved";
        let result = reducer.reduce(input);

        assert!(result.contains("IMPORTANT") || result.contains("COVID") || result.contains("12345"));
    }

    #[test]
    fn test_preserve_important_words_false_is_wired_through_to_the_word_filter() {
        // Same fixture shape as `word_filtering::tests::IMPORTANT_WORD_FIXTURE`:
        // enough unique long words to keep the primary filtering path active
        // (never falling back to "keep everything >= 3 chars"), plus a short,
        // thrice-repeated all-caps acronym that only the important-word guard
        // saves (#269).
        let input = "documentation implementation specification organization architecture \
             infrastructure optimization configuration authentication authorization synchronization \
             virtualization containerization orchestration serialization CEO CEO CEO";

        let preserving = TokenReductionConfig {
            level: ReductionLevel::Aggressive,
            preserve_important_words: true,
            ..Default::default()
        };
        let dropping = TokenReductionConfig {
            level: ReductionLevel::Aggressive,
            preserve_important_words: false,
            ..Default::default()
        };

        let preserving_result = TokenReducer::new(&preserving, Some("en")).unwrap().reduce(input);
        let dropping_result = TokenReducer::new(&dropping, Some("en")).unwrap().reduce(input);

        assert!(
            preserving_result.contains("CEO"),
            "preserve_important_words: true must keep CEO, got: {preserving_result}"
        );
        assert!(
            !dropping_result.contains("CEO"),
            "preserve_important_words: false must let CEO be removed, got: {dropping_result}"
        );
    }

    #[test]
    fn test_technical_terms_preservation() {
        let config = TokenReductionConfig {
            level: ReductionLevel::Aggressive,
            ..Default::default()
        };

        let reducer = TokenReducer::new(&config, None).unwrap();
        let input = "The implementation uses PyTorch and TensorFlow frameworks";
        let result = reducer.reduce(input);

        assert!(result.contains("PyTorch") || result.contains("TensorFlow"));
    }
}
