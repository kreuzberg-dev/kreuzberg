//! Cache hygiene checks: xberg-owned cache directories and stale model
//! revisions in the shared Hugging Face cache.
//!
//! SHA verification of the models the current config actually uses happens in
//! the backend probes (`is_rtdetr_cached` and the paddle cache check both
//! verify checksums), so this module only covers what probes do not: stray
//! files in xberg-owned dirs and outdated revisions in the HF cache.
//!
//! The HF cache is shared with other tools, so stale revisions found there are
//! report-only and never deleted. Stray files in xberg-owned dirs are the only
//! entries `clean_obsolete` removes.

use super::DoctorCheck;
use crate::core::config::ExtractionConfig;
use std::path::{Path, PathBuf};

/// Known xberg-owned cache modules (from `resolve_cache_dir` call sites).
/// Anything else in the cache base — e.g. staged layout models, older
/// versions' dirs, the CoreML bundle cache — is inspected but NEVER deleted:
/// a static allowlist that lags the code must not become a deletion rule.
const KNOWN_MODULE_DIRS: &[&str] = &["tessdata", "ocr", "paddle-ocr"];

pub(super) fn check_cache(_config: &ExtractionConfig) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();

    let base = crate::cache_dir::resolve_cache_base();
    let stray = stray_files(&base);
    if stray.is_empty() {
        checks.push(DoctorCheck::pass(
            "cache.xberg",
            "no stray files in xberg cache directories",
        ));
    } else {
        checks.push(DoctorCheck::pass(
            "cache.xberg",
            format!(
                "{} stray file(s) in xberg cache root (run `xberg doctor --clean` to remove): {}",
                stray.len(),
                stray
                    .iter()
                    .take(5)
                    .map(|p| p.display().to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }

    let unknown = unknown_dirs(&base);
    if !unknown.is_empty() {
        checks.push(DoctorCheck::pass(
            "cache.xberg.dirs",
            format!(
                "unknown cache entries (inspected only, never auto-cleaned): {}",
                unknown.join(", ")
            ),
        ));
    }

    #[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
    {
        let stale = stale_hf_revisions();
        if !stale.is_empty() {
            checks.push(DoctorCheck::pass(
                "cache.hf",
                format!(
                    "{} stale revision(s) of xberg-pinned models in the shared HF cache (report-only, not cleaned): {}",
                    stale.len(),
                    stale.join(", ")
                ),
            ));
        }
    }

    checks
}

/// Result of [`clean_obsolete`]: how many stray files were removed and how
/// many removals failed (e.g. permissions).
#[derive(Debug, Clone, Copy, Default)]
#[cfg_attr(alef, alef(skip))]
pub struct CleanOutcome {
    /// Files successfully deleted.
    pub removed: usize,
    /// Files that could not be deleted.
    pub failed: usize,
}

/// Remove the stray files reported by [`check_cache`]. Only regular files
/// directly in the xberg cache base that [`stray_files`] classified as stray
/// are touched; see there for the safety guards.
///
/// Excluded from the polyglot binding surface: exposed for the CLI's
/// `xberg doctor --clean`.
#[cfg_attr(alef, alef(skip))]
pub fn clean_obsolete() -> CleanOutcome {
    clean_stray_files(&crate::cache_dir::resolve_cache_base())
}

fn clean_stray_files(base: &Path) -> CleanOutcome {
    let mut outcome = CleanOutcome::default();
    for path in stray_files(base) {
        if std::fs::remove_file(path).is_ok() {
            outcome.removed += 1;
        } else {
            outcome.failed += 1;
        }
    }
    outcome
}

/// Stray regular FILES directly in the xberg cache root. These are the only
/// entries `clean_obsolete` deletes — directories and symlinks are never
/// touched.
///
/// `XBERG_CACHE_DIR` is a raw override, so a base pointed at a directory that
/// was never an xberg cache (`$HOME`, `.`) must not have its files classified
/// as stray: unless at least one known module dir exists, nothing qualifies.
fn stray_files(base: &Path) -> Vec<PathBuf> {
    let mut stray = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return stray;
    };
    if !KNOWN_MODULE_DIRS.iter().any(|dir| base.join(dir).is_dir()) {
        return stray;
    }
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            stray.push(entry.path());
        }
    }
    stray
}

/// Top-level cache entries that are not known module dirs. Report-only.
fn unknown_dirs(base: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(base) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .filter(|name| !KNOWN_MODULE_DIRS.contains(&name.as_str()))
        .collect()
}

/// Revisions of xberg-pinned HF model repos present in the shared cache that do
/// not match the currently pinned revision(s). Report-only: the cache is shared.
#[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
fn stale_hf_revisions() -> Vec<String> {
    let mut stale = Vec::new();
    let hf_cache = hf_hub::resolve_cache_dir();
    #[cfg(feature = "paddle-ocr")]
    collect_stale_revisions(
        &mut stale,
        &hf_cache,
        "models--xberg-io--paddleocr-onnx-models",
        &[crate::paddle_ocr::ModelManager::pinned_revision()],
    );
    #[cfg(layout_detection)]
    collect_stale_revisions(
        &mut stale,
        &hf_cache,
        "models--xberg-io--layout-models",
        &crate::layout::model_manager::pinned_revisions(),
    );
    stale
}

#[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
fn collect_stale_revisions(stale: &mut Vec<String>, hf_cache: &Path, repo_dir: &str, pinned: &[&str]) {
    let Ok(revisions) = std::fs::read_dir(hf_cache.join(repo_dir).join("snapshots")) else {
        return;
    };
    for revision in revisions.flatten() {
        let name = revision.file_name().to_string_lossy().into_owned();
        if !pinned.contains(&name.as_str()) {
            stale.push(format!("{repo_dir}@{name}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xberg_like_cache() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("tessdata")).unwrap();
        dir
    }

    #[test]
    fn stray_files_only_regular_files_in_module_dir_base() {
        let dir = xberg_like_cache();
        std::fs::write(dir.path().join("stale.tmp"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("some-dir")).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("stale.tmp"), dir.path().join("link.tmp")).unwrap();

        let stray = stray_files(dir.path());
        assert_eq!(stray, vec![dir.path().join("stale.tmp")]);
    }

    #[test]
    fn stray_files_empty_when_base_is_not_an_xberg_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious.txt"), b"keep").unwrap();
        assert!(stray_files(dir.path()).is_empty());
    }

    #[test]
    fn clean_removes_stray_files_and_keeps_everything_else() {
        let dir = xberg_like_cache();
        std::fs::write(dir.path().join("stale.tmp"), b"x").unwrap();
        std::fs::write(dir.path().join("tessdata").join("eng.traineddata"), b"x").unwrap();

        let outcome = clean_stray_files(dir.path());
        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.failed, 0);
        assert!(!dir.path().join("stale.tmp").exists());
        assert!(dir.path().join("tessdata").join("eng.traineddata").exists());
    }

    #[test]
    fn clean_deletes_nothing_outside_an_xberg_cache() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("precious.txt"), b"keep").unwrap();

        let outcome = clean_stray_files(dir.path());
        assert_eq!(outcome.removed, 0);
        assert!(dir.path().join("precious.txt").exists());
    }
}
