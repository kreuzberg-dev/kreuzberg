//! Keyword extraction module.
//!
//! Provides unified keyword extraction interface supporting multiple algorithms:
//! - YAKE (Yet Another Keyword Extractor) - statistical approach
//! - RAKE (Rapid Automatic Keyword Extraction) - co-occurrence based
//!
//! # Feature Flags
//!
//! - `keywords-yake`: Enable YAKE algorithm
//! - `keywords-rake`: Enable RAKE algorithm
//! - `keywords`: Enable both algorithms (default in `full` feature)
//!
//! # Examples
//!
//! ```ignore
//! # use xberg::keywords::{extract_keywords, KeywordConfig};
//! let text = "Rust is a systems programming language focused on safety and performance.";
//!
//! // Use default algorithm (YAKE if available)
//! let config = KeywordConfig::default();
//! let keywords = extract_keywords(text, &config).unwrap();
//!
//! for keyword in keywords {
//!     println!("{}: {:.3}", keyword.text, keyword.score);
//! }
//! ```
//!
//! ```rust,no_run
//! # #[cfg(feature = "keywords-rake")]
//! # {
//! # use xberg::keywords::{extract_keywords, KeywordAlgorithm, KeywordConfig};
//! // Use RAKE algorithm explicitly
//! let text = "Machine learning models require large datasets.";
//! let config = KeywordConfig {
//!     algorithm: KeywordAlgorithm::Rake,
//!     max_keywords: 5,
//!     min_score: 0.3,
//!     ..Default::default()
//! };
//!
//! let keywords = extract_keywords(text, &config).unwrap();
//! # }
//! ```

use crate::Result;
use crate::plugins::registry::get_post_processor_registry;
use once_cell::sync::OnceCell;
use std::sync::Arc;

pub mod config;
pub mod processor;
pub mod types;

#[cfg(feature = "keywords-yake")]
mod yake;

#[cfg(feature = "keywords-rake")]
mod rake;

pub use config::KeywordConfig;
pub use processor::KeywordExtractor;

#[cfg(feature = "keywords-rake")]
pub use config::RakeParams;

#[cfg(feature = "keywords-yake")]
pub use config::YakeParams;
pub use types::{Keyword, KeywordAlgorithm};

/// Extract keywords from text using the specified algorithm.
///
/// This is the unified entry point for keyword extraction. The algorithm
/// used is determined by `config.algorithm`.
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
/// Returns an error if:
/// - The specified algorithm feature is not enabled
/// - Keyword extraction fails
///
/// # Examples
///
/// ```rust,no_run
/// # use xberg::keywords::{extract_keywords, KeywordConfig};
/// let text = "Document intelligence with Rust provides memory safety.";
/// let config = KeywordConfig {
///     max_keywords: 10,
///     language: Some("en".to_string()),
///     ..Default::default()
/// };
///
/// let keywords = extract_keywords(text, &config)?;
///
/// for keyword in keywords {
///     println!("{}: {:.3}", keyword.text, keyword.score);
/// }
/// # Ok::<(), xberg::XbergError>(())
/// ```
pub fn extract_keywords(text: &str, config: &KeywordConfig) -> Result<Vec<Keyword>> {
    match config.algorithm {
        #[cfg(feature = "keywords-yake")]
        KeywordAlgorithm::Yake => yake::extract_keywords_yake(text, config),

        #[cfg(feature = "keywords-rake")]
        KeywordAlgorithm::Rake => rake::extract_keywords_rake(text, config),

        #[cfg(not(any(feature = "keywords-yake", feature = "keywords-rake")))]
        _ => Err(crate::XbergError::Other(
            "No keyword extraction algorithm feature enabled".to_string(),
        )),
    }
}

/// One-time initialization guard for the keyword extraction processor registry.
///
/// Set to `()` once registration succeeds. If registration fails the cell remains
/// empty, allowing the next call to retry.
static PROCESSOR_INITIALIZED: OnceCell<()> = OnceCell::new();

/// Ensure the keyword processor is registered.
///
/// This function is called automatically when needed.
/// It's safe to call multiple times - registration only happens once, unless
/// the global post-processor registry was cleared (e.g. by test teardown, see
/// #317), in which case the keyword processor is re-registered.
///
/// The `OnceCell` alone is not enough: `get_or_try_init` never re-runs its
/// closure once the cell is filled, so anything that later wipes the global
/// post-processor registry (such as
/// `plugins::registry::test_support::PostProcessorRegistryGuard`, whose
/// `acquire`/`drop` both call `clear_post_processors`) would otherwise leave
/// the keyword processor permanently missing for the rest of the process.
/// Checking the registry directly and re-registering when the entry is gone
/// makes this self-healing, mirroring `extractors::ensure_initialized`.
pub(crate) fn ensure_initialized() -> Result<()> {
    PROCESSOR_INITIALIZED.get_or_try_init(register_keyword_processor)?;

    let registry = get_post_processor_registry();
    let already_registered = registry
        .read()
        .list()
        .iter()
        .any(|name| name == processor::KEYWORD_PROCESSOR_NAME);

    if !already_registered {
        register_keyword_processor()?;
    }

    Ok(())
}

/// Register the keyword extraction processor with the global registry.
///
/// This function should be called once at application startup to register
/// the keyword extraction post-processor.
///
/// **Note:** This is called automatically on first use.
/// Explicit calling is optional.
///
/// # Example
///
/// Not run as a doctest: registration is `pub(crate)` and happens automatically on
/// first use, so there is no public call for this example to make. It documents the
/// in-crate call shape.
///
/// ```ignore
/// use xberg::keywords::register_keyword_processor;
///
/// # fn main() -> xberg::Result<()> {
/// register_keyword_processor()?;
/// # Ok(())
/// # }
/// ```
#[cfg_attr(alef, alef(skip))]
pub(crate) fn register_keyword_processor() -> Result<()> {
    let registry = get_post_processor_registry();
    let mut registry = registry.write();

    registry.register(Arc::new(KeywordExtractor))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn post_processor_registry_lists(name: &str) -> bool {
        get_post_processor_registry()
            .read()
            .list()
            .iter()
            .any(|registered| registered == name)
    }

    /// #317: `ensure_initialized` registers the keyword post-processor exactly
    /// once (via a `OnceCell`), with no re-registration path. Any code that
    /// wipes the global post-processor registry after that first call — such
    /// as `plugins::registry::test_support::PostProcessorRegistryGuard`,
    /// whose `acquire`/`drop` both call `clear_post_processors` — permanently
    /// removes the keyword processor for the rest of the process, because
    /// `get_or_try_init` never re-runs the registrar once the cell is filled.
    ///
    /// This test must run in a process where `ensure_initialized` has not
    /// already fired, so the first call below is the one that populates the
    /// `OnceCell`. Run it in isolation (e.g. `--test-threads=1` targeting just
    /// this test) to observe the failure deterministically.
    #[test]
    fn keyword_processor_registration_does_not_survive_a_post_processor_registry_guard_cycle() {
        ensure_initialized().expect("first call must register the keyword processor");
        assert!(
            post_processor_registry_lists("keyword-extraction"),
            "keyword processor must be present immediately after ensure_initialized"
        );

        // Simulate an unrelated test elsewhere in the binary that acquires and
        // releases `PostProcessorRegistryGuard`. Both acquire and drop call
        // `clear_post_processors`, wiping the registry regardless of what (if
        // anything) the guarded test itself registers.
        {
            use crate::plugins::registry::test_support::PostProcessorRegistryGuard;
            let _guard = PostProcessorRegistryGuard::acquire();
        }

        // `ensure_initialized` is backed by a `OnceCell` that is already
        // filled, so this call is a no-op: it will NOT re-run
        // `register_keyword_processor`, no matter how many times it's called.
        ensure_initialized().expect("ensure_initialized must remain Ok once already initialized");

        assert!(
            post_processor_registry_lists("keyword-extraction"),
            "keyword processor must still be registered after a PostProcessorRegistryGuard cycle"
        );
    }

    #[test]
    fn test_extract_keywords_default_algorithm() {
        let text = "Rust programming language provides memory safety and performance.";
        let config = KeywordConfig::default();

        let keywords = extract_keywords(text, &config).unwrap();

        assert!(!keywords.is_empty(), "Should extract keywords");
        assert!(keywords.len() <= config.max_keywords);
    }

    #[cfg(feature = "keywords-yake")]
    #[test]
    fn test_extract_keywords_yake() {
        let text = "Natural language processing using Rust is efficient and safe.";
        let config = KeywordConfig::yake();

        let keywords = extract_keywords(text, &config).unwrap();

        assert!(!keywords.is_empty());
        assert_eq!(keywords[0].algorithm, KeywordAlgorithm::Yake);
    }

    #[cfg(feature = "keywords-rake")]
    #[test]
    fn test_extract_keywords_rake() {
        let text = "Natural language processing using Rust is efficient and safe.";
        let config = KeywordConfig::rake();

        let keywords = extract_keywords(text, &config).unwrap();

        assert!(!keywords.is_empty());
        assert_eq!(keywords[0].algorithm, KeywordAlgorithm::Rake);
    }

    #[cfg(all(feature = "keywords-yake", feature = "keywords-rake"))]
    #[test]
    fn test_compare_algorithms() {
        let text = "Machine learning and artificial intelligence are transforming technology. \
                    Deep learning models require substantial computational resources.";

        let yake_config = KeywordConfig::yake().with_max_keywords(5);
        let yake_keywords = extract_keywords(text, &yake_config).unwrap();

        let rake_config = KeywordConfig::rake().with_max_keywords(5);
        let rake_keywords = extract_keywords(text, &rake_config).unwrap();

        assert!(!yake_keywords.is_empty());
        assert!(!rake_keywords.is_empty());

        assert!(yake_keywords.iter().all(|k| k.algorithm == KeywordAlgorithm::Yake));
        assert!(rake_keywords.iter().all(|k| k.algorithm == KeywordAlgorithm::Rake));

        println!(
            "YAKE keywords: {:?}",
            yake_keywords.iter().map(|k| &k.text).collect::<Vec<_>>()
        );
        println!(
            "RAKE keywords: {:?}",
            rake_keywords.iter().map(|k| &k.text).collect::<Vec<_>>()
        );
    }
}
