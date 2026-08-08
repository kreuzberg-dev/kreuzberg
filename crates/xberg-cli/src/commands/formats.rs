//! Format reporting for `xberg formats`.
//!
//! [`xberg::list_supported_formats`] returns the core's *static* format catalogue: it is built
//! from an ungated `FORMATS` table in `xberg::core::mime`, so it reports the same ~119 formats no
//! matter which cargo features were compiled into the binary. Every extractor, by contrast, is
//! registered behind a `#[cfg(feature = ...)]` gate in `xberg::extractors::register_default_extractors`.
//!
//! The two therefore disagree for any build that is not the full feature set, and the catalogue
//! advertises formats that `extract` then rejects with `UnsupportedFormat`. Observed on a
//! `--no-default-features` build of this CLI: `xberg formats` listed `docx`, `pdf` and `xlsx`,
//! while `xberg extract` on each of them failed with
//! `Unsupported format: application/vnd.openxmlformats-officedocument.wordprocessingml.document`.
//! The same mismatch is what GH#1387 reports against the .NET binding for `xlsx`.
//!
//! This module resolves the catalogue against the extractor registry, which is populated from
//! those `#[cfg(feature = ...)]` gates, so the reported list is derived from what the running
//! binary actually compiled in. Reading the registry rather than restating the gates in a
//! CLI-side table means the two cannot drift apart as extractors are added or re-gated.
//!
//! Related: GH#1387 reports the same over-reporting through the .NET binding, which reads the
//! same static catalogue. Fixing it there means routing that binding through the registry too.

use anyhow::{Context, Result};
use xberg::{SupportedFormat, list_supported_formats};

/// Return the formats this binary can actually extract.
///
/// The static catalogue is filtered down to entries whose MIME type resolves to a registered
/// extractor, so a format is reported only when the feature providing its extractor was compiled
/// in. Wildcard registrations (for example `image/*`) resolve through the registry's own
/// fallback, so families are handled without special-casing here.
///
/// The returned entries keep the catalogue's ordering (sorted by extension) and its
/// [`SupportedFormat`] shape, so the `--format json` and `--format toon` output shapes are
/// unchanged — only over-reported rows disappear.
pub fn compiled_in_formats() -> Result<Vec<SupportedFormat>> {
    // The registry is filled lazily on first extraction, so on a cold process it is empty and
    // every lookup below would miss — reporting no formats at all.
    xberg::extractors::ensure_initialized()
        .context("Failed to initialize the extractor registry while listing supported formats")?;

    let registry = xberg::plugins::registry::get_document_extractor_registry();
    let registered = registry.read();

    Ok(list_supported_formats()
        .into_iter()
        .filter(|format| registered.get(&format.mime_type).is_ok())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extensions() -> Vec<String> {
        compiled_in_formats()
            .expect("listing compiled-in formats must succeed")
            .into_iter()
            .map(|format| format.extension)
            .collect()
    }

    fn contains(extensions: &[String], extension: &str) -> bool {
        extensions.iter().any(|candidate| candidate == extension)
    }

    /// Formats backed by an unconditionally registered extractor must always be reported.
    #[test]
    fn compiled_in_formats_always_reports_ungated_formats() {
        let extensions = extensions();

        for extension in ["txt", "md", "csv", "json"] {
            assert!(
                contains(&extensions, extension),
                "'{extension}' has an ungated extractor and must always be reported; got {extensions:?}"
            );
        }
    }

    /// The reported list must track the feature set: formats whose extractor was compiled out
    /// must disappear rather than being advertised and then failing at extraction time.
    #[test]
    fn compiled_in_formats_tracks_the_compiled_feature_set() {
        let extensions = extensions();

        if cfg!(feature = "pdf-surface") {
            assert!(
                contains(&extensions, "pdf"),
                "the `pdf-surface` feature is enabled, so 'pdf' must be reported; got {extensions:?}"
            );
        } else {
            assert!(
                !contains(&extensions, "pdf"),
                "the `pdf-surface` feature is disabled, so 'pdf' must not be advertised; got {extensions:?}"
            );
        }

        // `formats-no-heic` is what pulls `xberg/excel` and `xberg/office` in; `pdf-surface`
        // alone does not, which makes these two the discriminator between the feature sets.
        for extension in ["docx", "xlsx"] {
            if cfg!(feature = "formats-no-heic") {
                assert!(
                    contains(&extensions, extension),
                    "the `formats-no-heic` feature is enabled, so '{extension}' must be reported; got {extensions:?}"
                );
            } else {
                assert!(
                    !contains(&extensions, extension),
                    "the `formats-no-heic` feature is disabled, so '{extension}' must not be advertised; \
                     this is the GH#1387 mismatch; got {extensions:?}"
                );
            }
        }
    }

    /// Guards the direction of the fix: the reported list is the static catalogue minus the
    /// formats with no compiled-in extractor, and it is strictly smaller whenever the format
    /// features are absent.
    #[test]
    fn compiled_in_formats_is_a_subset_of_the_static_catalogue() {
        let compiled_in = compiled_in_formats().expect("listing compiled-in formats must succeed");
        let catalogue = list_supported_formats();

        assert!(
            compiled_in.len() <= catalogue.len(),
            "compiled-in formats ({}) cannot exceed the static catalogue ({})",
            compiled_in.len(),
            catalogue.len()
        );

        if !cfg!(feature = "formats-no-heic") {
            assert!(
                compiled_in.len() < catalogue.len(),
                "without the format features the catalogue ({}) must over-report relative to the \
                 compiled-in list ({}) — equal lengths mean the filter did nothing",
                catalogue.len(),
                compiled_in.len()
            );
        }
    }
}
