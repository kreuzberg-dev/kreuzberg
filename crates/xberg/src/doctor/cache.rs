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

/// Known xberg-owned cache modules (from `resolve_cache_dir` and
/// `cache_base.join` call sites: tessdata/ocr/paddle-ocr resolvers,
/// `GenericCache` extraction, MCP layout/embeddings warmup). Anything else in
/// the cache base — e.g. staged layout models, older versions' dirs, the
/// CoreML bundle cache — is inspected but NEVER deleted: a static allowlist
/// that lags the code must not become a deletion rule.
const KNOWN_MODULE_DIRS: &[&str] = &["tessdata", "ocr", "paddle-ocr", "extraction", "layout", "embeddings"];

pub(super) fn check_cache(_config: &ExtractionConfig) -> Vec<DoctorCheck> {
    let mut checks = cache_base_checks(&crate::cache_dir::resolve_cache_base(), cache_dir_overridden());
    checks.extend(hf_stale_check());
    checks
}

#[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
fn hf_stale_check() -> Option<DoctorCheck> {
    let stale = stale_hf_revisions();
    if stale.is_empty() {
        return None;
    }
    Some(DoctorCheck::warn(
        "cache.hf",
        format!(
            "{} stale revision(s) of xberg-pinned models in the shared HF cache (report-only, not cleaned): {}",
            stale.len(),
            stale.join(", ")
        ),
    ))
}

#[cfg(not(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection))))]
fn hf_stale_check() -> Option<DoctorCheck> {
    None
}

/// Whether the cache base comes from the `XBERG_CACHE_DIR` override rather
/// than the constructed platform default.
fn cache_dir_overridden() -> bool {
    std::env::var_os("XBERG_CACHE_DIR").is_some()
}

/// Checks over the xberg cache base. The constructed default base is
/// xberg-owned by definition; an `XBERG_CACHE_DIR` override is a raw user
/// pointer with no way to verify ownership (any name-based heuristic can
/// collide with real user directories), so an overridden base is never
/// inspected: its file names must not leak into a report designed for public
/// bug reports, and `--clean` must not delete in it.
fn cache_base_checks(base: &Path, overridden: bool) -> Vec<DoctorCheck> {
    let mut checks = Vec::new();
    if overridden {
        checks.push(DoctorCheck::skip(
            "cache.xberg",
            format!(
                "XBERG_CACHE_DIR override in effect ({}); ownership cannot be verified, so stray-file inspection and --clean are disabled",
                base.display()
            ),
        ));
        return checks;
    }
    if base.exists() && std::fs::read_dir(base).is_err() {
        // An absent base is a fresh install; an unreadable one must not
        // masquerade as a clean bill of health. ~keep
        checks.push(DoctorCheck::warn(
            "cache.xberg",
            format!(
                "cache base {} exists but is not readable; stray-file and unknown-dir checks skipped",
                base.display()
            ),
        ));
    } else {
        let stray = stray_files(base);
        if stray.is_empty() {
            checks.push(DoctorCheck::pass(
                "cache.xberg",
                "no stray files in xberg cache directories",
            ));
        } else {
            checks.push(DoctorCheck::warn(
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

        let unknown = unknown_dirs(base);
        if !unknown.is_empty() {
            checks.push(DoctorCheck::warn(
                "cache.xberg.dirs",
                format!(
                    "unknown cache entries (inspected only, never auto-cleaned): {}",
                    unknown.join(", ")
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
/// directly in the xberg cache base are touched — directories and symlinks
/// never; see [`stray_files`]. Returns `None` without touching anything when
/// `XBERG_CACHE_DIR` is set: ownership of an overridden base cannot be
/// verified, so it is never cleaned.
///
/// Excluded from the polyglot binding surface: exposed for the CLI's
/// `xberg doctor --clean`.
#[cfg_attr(alef, alef(skip))]
pub fn clean_obsolete() -> Option<CleanOutcome> {
    if cache_dir_overridden() {
        return None;
    }
    Some(clean_stray_files(&crate::cache_dir::resolve_cache_base()))
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
/// touched. Callers gate on provenance ([`cache_base_checks`] /
/// [`clean_obsolete`]): this scanner must only ever see the constructed
/// default base.
fn stray_files(base: &Path) -> Vec<PathBuf> {
    let mut stray = Vec::new();
    let Ok(entries) = std::fs::read_dir(base) else {
        return stray;
    };
    for entry in entries.flatten() {
        if entry.file_type().is_ok_and(|t| t.is_file()) {
            stray.push(entry.path());
        }
    }
    stray
}

/// Top-level cache entries that are not known module dirs. Report-only, and
/// provenance-gated like [`stray_files`].
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
///
/// Pinned revisions are grouped per repository: layout pins models in both
/// `layout-models` and `paddleocr-onnx-models`, so a revision valid for one
/// repo must not be accepted for the other.
#[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
fn stale_hf_revisions() -> Vec<String> {
    let mut pinned: std::collections::BTreeMap<String, Vec<&'static str>> = std::collections::BTreeMap::new();
    #[cfg(feature = "paddle-ocr")]
    pinned
        .entry(hf_repo_dir("xberg-io/paddleocr-onnx-models"))
        .or_default()
        .push(crate::paddle_ocr::ModelManager::pinned_revision());
    #[cfg(layout_detection)]
    for (repo_id, revision) in crate::layout::model_manager::pinned_repo_revisions() {
        pinned.entry(hf_repo_dir(repo_id)).or_default().push(revision);
    }

    let hf_cache = hf_hub::resolve_cache_dir();
    let mut stale = Vec::new();
    for (repo_dir, revisions) in &pinned {
        collect_stale_revisions(&mut stale, &hf_cache, repo_dir, revisions);
    }
    stale
}

/// The hf-hub on-disk directory name for a Hub repo id.
#[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
fn hf_repo_dir(repo_id: &str) -> String {
    format!("models--{}", repo_id.replace('/', "--"))
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

    fn cache_with(entries: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for entry in entries {
            std::fs::create_dir(dir.path().join(entry)).unwrap();
        }
        dir
    }

    #[test]
    fn stray_files_only_regular_files() {
        let dir = cache_with(&["tessdata", "some-dir"]);
        std::fs::write(dir.path().join("stale.tmp"), b"x").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink(dir.path().join("stale.tmp"), dir.path().join("link.tmp")).unwrap();

        let stray = stray_files(dir.path());
        assert_eq!(stray, vec![dir.path().join("stale.tmp")]);
    }

    #[test]
    fn unknown_dirs_reports_only_non_module_dirs() {
        let dir = cache_with(&["tessdata", "layout-staging"]);
        assert_eq!(unknown_dirs(dir.path()), vec!["layout-staging".to_string()]);
    }

    #[test]
    fn every_runtime_module_dir_is_recognized() {
        let dir = cache_with(KNOWN_MODULE_DIRS);
        assert!(
            unknown_dirs(dir.path()).is_empty(),
            "no runtime module dir may be reported as unknown"
        );
    }

    #[test]
    fn overridden_base_is_never_inspected() {
        // XBERG_CACHE_DIR pointed at a real user directory: nothing may be
        // listed, whatever it contains.
        let dir = cache_with(&["tessdata", "embeddings", "Documents"]);
        std::fs::write(dir.path().join("precious.txt"), b"keep").unwrap();

        let checks = cache_base_checks(dir.path(), true);
        assert_eq!(checks.len(), 1);
        assert_eq!(checks[0].status, super::super::ProbeStatus::Skip);
        assert!(!checks[0].message.contains("precious"), "no file names may leak");
    }

    #[test]
    fn default_base_is_inspected() {
        let dir = cache_with(&["extraction"]);
        std::fs::write(dir.path().join("stale.tmp"), b"x").unwrap();

        let checks = cache_base_checks(dir.path(), false);
        let stray = checks.iter().find(|c| c.name == "cache.xberg").unwrap();
        assert_eq!(stray.status, super::super::ProbeStatus::Warn);
        assert!(stray.message.contains("stale.tmp"));
        assert!(
            !checks.iter().any(|c| c.name == "cache.xberg.dirs"),
            "extraction is a known module"
        );
    }

    #[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
    #[test]
    fn stale_revisions_are_matched_per_repository() {
        let hf = tempfile::tempdir().unwrap();
        for (repo, revision) in [("models--org--alpha", "aaa"), ("models--org--beta", "bbb")] {
            std::fs::create_dir_all(hf.path().join(repo).join("snapshots").join(revision)).unwrap();
        }

        let mut stale = Vec::new();
        // "bbb" is pinned only for beta, so alpha@aaa is stale even though
        // alpha@aaa would match alpha's own pin.
        collect_stale_revisions(&mut stale, hf.path(), "models--org--alpha", &["bbb"]);
        collect_stale_revisions(&mut stale, hf.path(), "models--org--beta", &["bbb"]);
        assert_eq!(stale, vec!["models--org--alpha@aaa".to_string()]);
    }

    #[cfg(all(not(target_arch = "wasm32"), any(feature = "paddle-ocr", layout_detection)))]
    #[test]
    fn hf_repo_dir_follows_hub_layout() {
        assert_eq!(hf_repo_dir("xberg-io/layout-models"), "models--xberg-io--layout-models");
    }

    #[test]
    fn clean_removes_stray_files_and_keeps_everything_else() {
        let dir = cache_with(&["tessdata"]);
        std::fs::write(dir.path().join("stale.tmp"), b"x").unwrap();
        std::fs::write(dir.path().join("tessdata").join("eng.traineddata"), b"x").unwrap();

        let outcome = clean_stray_files(dir.path());
        assert_eq!(outcome.removed, 1);
        assert_eq!(outcome.failed, 0);
        assert!(!dir.path().join("stale.tmp").exists());
        assert!(dir.path().join("tessdata").join("eng.traineddata").exists());
    }
}
