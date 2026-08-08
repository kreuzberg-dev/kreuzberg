//! Shared ONNX Runtime model-loading helpers.
//!
//! Consolidates the HuggingFace download + cross-process lock + tokenizer load +
//! ORT session-build machinery that would otherwise be copy-pasted across every
//! ONNX-backed capability (embeddings, reranking, sparse embeddings, late
//! interaction). New ONNX modules build on these helpers instead of vendoring
//! their own copies.
//!
//! Each fallible helper takes an [`ErrCtor`] — a module-specific error
//! constructor (e.g. [`crate::XbergError::embedding`] or
//! [`crate::XbergError::reranking`]) — so callers keep their module-tagged error
//! variant without this module needing to know which capability it serves.
//! ONNX-Runtime-missing failures are reported as [`crate::XbergError::MissingDependency`]
//! regardless of the caller.
//!
//! Since v5.0.0.

use crate::core::config::DownloadProgress;
use std::path::{Path, PathBuf};

/// A module-specific error constructor, e.g. `crate::XbergError::embedding::<String>`.
///
/// Threaded through the fallible helpers so each caller keeps its own
/// module-tagged [`crate::XbergError`] variant.
pub(crate) type ErrCtor = fn(String) -> crate::XbergError;

/// Returns installation instructions for ONNX Runtime.
pub(crate) fn onnx_runtime_install_message() -> String {
    #[cfg(all(windows, target_env = "gnu"))]
    {
        return "ONNX Runtime is not supported on Windows MinGW builds. \
        ONNX Runtime requires MSVC toolchain. \
        Please use Windows MSVC builds or disable ONNX-backed features."
            .to_string();
    }

    #[cfg(not(all(windows, target_env = "gnu")))]
    {
        "ONNX Runtime is required for this functionality. \
        Install: \
        macOS: 'brew install onnxruntime', \
        Linux (Ubuntu/Debian): 'apt install libonnxruntime libonnxruntime-dev', \
        Linux (Fedora): 'dnf install onnxruntime onnxruntime-devel', \
        Linux (Arch): 'pacman -S onnxruntime', \
        Windows (MSVC): Download from https://github.com/microsoft/onnxruntime/releases and add to PATH. \
        \
        Alternatively, set ORT_DYLIB_PATH environment variable to the ONNX Runtime library path."
            .to_string()
    }
}

/// Check if an error message looks like an ONNX Runtime missing dependency.
pub(crate) fn looks_like_ort_error(msg: &str) -> bool {
    msg.contains("onnxruntime")
        || msg.contains("ORT")
        || msg.contains("libonnxruntime")
        || msg.contains("onnxruntime.dll")
        || msg.contains("Unable to load")
        || msg.contains("library load failed")
        || msg.contains("attempting to load")
        || msg.contains("An error occurred while")
}

/// Convert a panic payload to a string message.
pub(crate) fn panic_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else {
        "Unknown panic".to_string()
    }
}

/// Map a failure message to either `MissingDependency` (when it looks like an ORT
/// load failure) or the caller's module-specific error.
fn ort_missing_or(err: ErrCtor, msg: String) -> crate::XbergError {
    if looks_like_ort_error(&msg) {
        crate::XbergError::MissingDependency(format!("ONNX Runtime - {}", onnx_runtime_install_message()))
    } else {
        err(msg)
    }
}

/// Local paths of a downloaded model's files.
///
/// `special_tokens` and `tokenizer_config` may be empty paths when the repo does
/// not ship those optional files; [`load_tokenizer`] handles the empty case.
pub(crate) struct DownloadedModel {
    pub model: PathBuf,
    pub tokenizer: PathBuf,
    pub config: PathBuf,
    pub special_tokens: PathBuf,
    pub tokenizer_config: PathBuf,
}

/// Download a model's files from HuggingFace and return their local paths.
///
/// `additional_files` are sibling files that must accompany `model_file` (e.g. a
/// `model.onnx.data` weight blob). They are downloaded into the same cache
/// directory; their paths are not returned because ONNX Runtime locates them by
/// sibling-name relative to `model_file` at load time.
///
/// Serializes concurrent first-time downloads across processes via a blocking
/// cross-process advisory lock, and self-heals stale `.lock`/`.part` files.
///
/// `manifest` is the module's checked-in `presets.sha256sum` (compiled in via
/// `include_str!`). Every downloaded file whose repo-relative path appears in the
/// manifest is verified against its pinned SHA-256 and the download fails on a
/// mismatch (fail-closed against a tampered/rolled-back mirror). Files absent from
/// the manifest — `Custom` repos, which ship no manifest — are downloaded without
/// verification, preserving the existing behaviour for user-supplied models. Pass
/// `None` to skip verification entirely.
///
/// `progress` carries the capability config's `show_download_progress` setting down to the
/// Hugging Face client, so every file this helper fetches honours it (#279).
#[allow(clippy::too_many_arguments)]
pub(crate) fn download_model_files(
    repo_name: &str,
    model_file: &str,
    additional_files: &[String],
    revision: Option<&str>,
    cache_directory: Option<&Path>,
    progress: DownloadProgress,
    manifest: Option<&str>,
    err: ErrCtor,
) -> crate::Result<DownloadedModel> {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        download_model_files_inner(
            repo_name,
            model_file,
            additional_files,
            revision,
            cache_directory,
            progress,
            manifest,
            err,
        )
    })) {
        Ok(result) => result,
        Err(payload) => {
            let panic_msg = panic_to_string(payload);
            Err(ort_missing_or(err, format!("Model download panicked: {panic_msg}")))
        }
    }
}

/// Fetch a companion file (tokenizer/config/…) trying the model's own directory
/// first, then the repo root.
///
/// Consolidated repos (e.g. `xberg-io/reranker-models`) co-locate every file for
/// a model under a `<name>/` subdir, so `<model_dir>/tokenizer.json` is correct.
/// Standard HF repos keep the model in `onnx/` but the tokenizer at the root, so
/// the root fallback covers those (and arbitrary `Custom` repos). Runs each
/// candidate under the download watchdog.
///
/// Returns the local cache path plus the repo-relative path that actually
/// resolved, so the caller can look that path up in the sha256 manifest.
fn fetch_companion(
    repo_name: &str,
    model_dir: Option<&str>,
    file_name: &str,
    revision: Option<&str>,
    cache_directory: Option<&Path>,
    progress: DownloadProgress,
    manifest: &[(String, String)],
) -> Result<(PathBuf, String), String> {
    let candidates: Vec<String> = match model_dir {
        Some(dir) if !dir.is_empty() => vec![format!("{dir}/{file_name}"), file_name.to_string()],
        _ => vec![file_name.to_string()],
    };
    let mut last_err = String::new();
    for candidate in candidates {
        let expected = match manifest_checksum(manifest, &candidate) {
            Ok(expected) => expected,
            Err(error) => {
                last_err = error;
                continue;
            }
        };
        match crate::model_download::hf_resolve_file_with_progress(
            repo_name,
            &candidate,
            revision,
            cache_directory,
            expected,
            progress,
        ) {
            Ok(path) => return Ok((path, candidate)),
            Err(e) => last_err = e,
        }
    }
    Err(last_err)
}

/// Fetch optional tokenizer metadata without weakening a preset manifest.
/// Missing, unpinned metadata is allowed; once a preset pins either candidate,
/// every resolution or integrity error is fatal.
fn fetch_optional_companion(
    repo_name: &str,
    model_dir: Option<&str>,
    file_name: &str,
    revision: Option<&str>,
    cache_directory: Option<&Path>,
    progress: DownloadProgress,
    manifest: &[(String, String)],
) -> Result<(PathBuf, String), String> {
    let nested_path = model_dir
        .filter(|dir| !dir.is_empty())
        .map(|dir| format!("{dir}/{file_name}"));
    let is_pinned = manifest
        .iter()
        .any(|(path, _)| path == file_name || nested_path.as_ref().is_some_and(|nested| path == nested));

    match fetch_companion(
        repo_name,
        model_dir,
        file_name,
        revision,
        cache_directory,
        progress,
        manifest,
    ) {
        Ok(resolved) => Ok(resolved),
        Err(error) if is_pinned => Err(error),
        Err(_) => Ok((PathBuf::new(), String::new())),
    }
}

/// Return a pinned checksum for a preset artifact. Custom repositories have no
/// manifest and remain caller-managed; a non-empty preset manifest must list
/// every artifact Xberg resolves.
fn manifest_checksum<'a>(manifest: &'a [(String, String)], repo_path: &str) -> Result<Option<&'a str>, String> {
    match manifest.iter().find(|(path, _)| path == repo_path) {
        Some((_, sha256)) => Ok(Some(sha256.as_str())),
        None if manifest.is_empty() => Ok(None),
        None => Err(format!("SHA-256 manifest does not list {repo_path}")),
    }
}

/// Verify a downloaded file against the module's sha256 manifest.
///
/// When a preset manifest is present, `repo_path` must be listed and the file at
/// `local` must hash to the pinned value. Custom repos use an empty manifest and
/// remain caller-managed. An empty `repo_path` (an optional companion that was
/// not downloaded) is a no-op.
fn verify_downloaded(manifest: &[(String, String)], repo_path: &str, local: &Path, err: ErrCtor) -> crate::Result<()> {
    if repo_path.is_empty() {
        return Ok(());
    }
    if let Some((_, sha256)) = manifest.iter().find(|(path, _)| path == repo_path) {
        crate::model_download::verify_sha256(local, sha256, repo_path).map_err(err)?;
    } else if !manifest.is_empty() {
        return Err(err(format!("SHA-256 manifest does not list {repo_path}")));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn download_model_files_inner(
    repo_name: &str,
    model_file: &str,
    additional_files: &[String],
    revision: Option<&str>,
    cache_directory: Option<&Path>,
    progress: DownloadProgress,
    manifest: Option<&str>,
    err: ErrCtor,
) -> crate::Result<DownloadedModel> {
    let manifest: Vec<(String, String)> = match manifest {
        Some(content) => crate::model_download::parse_sha256_manifest(content)
            .map_err(|e| err(format!("Invalid sha256 manifest for {repo_name}: {e}")))?,
        None => Vec::new(),
    };

    let model_sha = manifest_checksum(&manifest, model_file).map_err(err)?;
    let model = crate::model_download::hf_resolve_file_with_progress(
        repo_name,
        model_file,
        revision,
        cache_directory,
        model_sha,
        progress,
    )
    .map_err(|e| err(format!("Failed to resolve {model_file} from {repo_name}: {e}")))?;
    verify_downloaded(&manifest, model_file, &model, err)?;

    for sibling in additional_files {
        let sibling_sha = manifest_checksum(&manifest, sibling).map_err(err)?;
        let sib_path = crate::model_download::hf_resolve_file_with_progress(
            repo_name,
            sibling,
            revision,
            cache_directory,
            sibling_sha,
            progress,
        )
        .map_err(|e| {
            err(format!(
                "Failed to resolve sibling file {sibling} from {repo_name}: {e}"
            ))
        })?;
        verify_downloaded(&manifest, sibling, &sib_path, err)?;
    }

    let model_dir = Path::new(model_file)
        .parent()
        .and_then(|p| p.to_str())
        .filter(|s| !s.is_empty());

    let (tokenizer, tokenizer_rel) = fetch_companion(
        repo_name,
        model_dir,
        "tokenizer.json",
        revision,
        cache_directory,
        progress,
        &manifest,
    )
    .map_err(|e| err(format!("Failed to download tokenizer.json: {e}")))?;
    verify_downloaded(&manifest, &tokenizer_rel, &tokenizer, err)?;

    let (config, config_rel) = fetch_companion(
        repo_name,
        model_dir,
        "config.json",
        revision,
        cache_directory,
        progress,
        &manifest,
    )
    .map_err(|e| err(format!("Failed to download config.json: {e}")))?;
    verify_downloaded(&manifest, &config_rel, &config, err)?;

    let (special_tokens, special_tokens_rel) = fetch_optional_companion(
        repo_name,
        model_dir,
        "special_tokens_map.json",
        revision,
        cache_directory,
        progress,
        &manifest,
    )
    .map_err(|e| err(format!("Failed to download special_tokens_map.json: {e}")))?;
    verify_downloaded(&manifest, &special_tokens_rel, &special_tokens, err)?;

    let (tokenizer_config, tokenizer_config_rel) = fetch_optional_companion(
        repo_name,
        model_dir,
        "tokenizer_config.json",
        revision,
        cache_directory,
        progress,
        &manifest,
    )
    .map_err(|e| err(format!("Failed to download tokenizer_config.json: {e}")))?;
    verify_downloaded(&manifest, &tokenizer_config_rel, &tokenizer_config, err)?;

    Ok(DownloadedModel {
        model,
        tokenizer,
        config,
        special_tokens,
        tokenizer_config,
    })
}

/// Load and configure a tokenizer with `BatchLongest` padding and truncation.
///
/// Reads `pad_token_id` from `config.json` and `model_max_length`/`pad_token`
/// from `tokenizer_config.json` (both optional, sensible defaults applied), then
/// merges any special tokens declared in `special_tokens_map.json`. `max_length`
/// is capped at the model's declared maximum.
pub(crate) fn load_tokenizer(
    files: &DownloadedModel,
    max_length: usize,
    err: ErrCtor,
) -> crate::Result<tokenizers::Tokenizer> {
    use tokenizers::{AddedToken, PaddingParams, PaddingStrategy, TruncationParams};

    let config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&files.config).map_err(|e| err(format!("Failed to read config.json: {e}")))?,
    )
    .map_err(|e| err(format!("Failed to parse config.json: {e}")))?;

    let tokenizer_config: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&files.tokenizer_config)
            .map_err(|e| err(format!("Failed to read tokenizer_config.json: {e}")))?,
    )
    .map_err(|e| err(format!("Failed to parse tokenizer_config.json: {e}")))?;

    let mut tokenizer = tokenizers::Tokenizer::from_file(&files.tokenizer)
        .map_err(|e| err(format!("Failed to load tokenizer: {e}")))?;

    let model_max_length = tokenizer_config["model_max_length"].as_f64().unwrap_or(512.0) as usize;
    let max_length = max_length.min(model_max_length);
    let pad_id = config["pad_token_id"].as_u64().unwrap_or(0) as u32;
    let pad_token = tokenizer_config["pad_token"].as_str().unwrap_or("[PAD]").to_string();

    tokenizer
        .with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            pad_token,
            pad_id,
            ..Default::default()
        }))
        .with_truncation(Some(TruncationParams {
            max_length,
            ..Default::default()
        }))
        .map_err(|e| err(format!("Failed to configure tokenizer: {e}")))?;

    if let Ok(special_tokens_data) = std::fs::read(&files.special_tokens)
        && let Ok(serde_json::Value::Object(map)) = serde_json::from_slice(&special_tokens_data)
    {
        for (_, value) in &map {
            if let Some(content) = value.as_str() {
                let _ = tokenizer.add_special_tokens([AddedToken {
                    content: content.to_string(),
                    special: true,
                    ..Default::default()
                }]);
            } else if value.is_object()
                && let (Some(content), Some(single_word), Some(lstrip), Some(rstrip), Some(normalized)) = (
                    value["content"].as_str(),
                    value["single_word"].as_bool(),
                    value["lstrip"].as_bool(),
                    value["rstrip"].as_bool(),
                    value["normalized"].as_bool(),
                )
            {
                let _ = tokenizer.add_special_tokens([AddedToken {
                    content: content.to_string(),
                    special: true,
                    single_word,
                    lstrip,
                    rstrip,
                    normalized,
                }]);
            }
        }
    }

    Ok(tokenizer)
}

/// Build an ORT session for `model_path` with the standard xberg configuration:
/// `GraphOptimizationLevel::All`, an intra-op thread budget resolved from the
/// concurrency config, a single inter-op thread, and the execution provider
/// selected by [`crate::ort_discovery::apply_execution_providers`].
///
/// The build runs inside `catch_unwind` because ORT can panic on a missing or
/// incompatible native library; such failures map to
/// [`crate::XbergError::MissingDependency`].
pub(crate) fn build_session(
    model_path: &Path,
    accel: Option<&crate::core::config::acceleration::AccelerationConfig>,
    err: ErrCtor,
) -> crate::Result<ort::session::Session> {
    let thread_budget = crate::core::config::concurrency::resolve_thread_budget(None);

    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut builder = ort::session::Session::builder()?;
        builder = builder
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::All)
            .map_err(|e| ort::Error::new(e.message()))?;
        builder = builder
            .with_intra_threads(thread_budget)
            .map_err(|e| ort::Error::new(e.message()))?;
        builder = builder
            .with_inter_threads(1)
            .map_err(|e| ort::Error::new(e.message()))?;
        builder = crate::ort_discovery::apply_execution_providers(builder, accel)?;
        builder.commit_from_file(model_path)
    }))
    .map_err(|payload| {
        ort_missing_or(
            err,
            format!("ONNX Runtime initialization panicked: {}", panic_to_string(payload)),
        )
    })?
    .map_err(|e| ort_missing_or(err, format!("Failed to create ONNX session: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn embed_err(msg: String) -> crate::XbergError {
        crate::XbergError::embedding(msg)
    }

    #[test]
    fn looks_like_ort_error_detects_keywords() {
        assert!(looks_like_ort_error("failed to load libonnxruntime.so"));
        assert!(looks_like_ort_error("An error occurred while loading the model"));
        assert!(!looks_like_ort_error("some unrelated parsing failure"));
    }

    #[test]
    fn panic_to_string_handles_str_and_string_and_other() {
        assert_eq!(panic_to_string(Box::new("boom")), "boom");
        assert_eq!(panic_to_string(Box::new(String::from("kaboom"))), "kaboom");
        assert_eq!(panic_to_string(Box::new(42_u8)), "Unknown panic");
    }

    #[test]
    fn ort_missing_or_maps_ort_errors_to_missing_dependency() {
        let e = ort_missing_or(embed_err, "libonnxruntime not found".to_string());
        assert!(matches!(e, crate::XbergError::MissingDependency(_)));
        let e = ort_missing_or(embed_err, "generic failure".to_string());
        assert!(matches!(e, crate::XbergError::Embedding { .. }));
    }

    #[test]
    fn verify_downloaded_errors_on_checksum_mismatch() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("model.onnx");
        std::fs::write(&file, b"tampered bytes").unwrap();
        let manifest = vec![("name/model.onnx".to_string(), "0".repeat(64))];
        let result = verify_downloaded(&manifest, "name/model.onnx", &file, embed_err);
        assert!(result.is_err(), "tampered file must fail checksum verification");
    }

    #[test]
    fn verify_downloaded_passes_on_checksum_match() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("model.onnx");
        std::fs::write(&file, b"pinned content").unwrap();
        let digest = "28f10de8a12ace2df7c733d697168479b5707cdb2a21df8561cabda49473e3c1";
        let manifest = vec![("name/model.onnx".to_string(), digest.to_string())];
        verify_downloaded(&manifest, "name/model.onnx", &file, embed_err)
            .expect("matching file must pass verification");
    }

    #[test]
    fn verify_downloaded_rejects_unlisted_preset_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("model.onnx");
        std::fs::write(&file, b"anything").unwrap();
        let manifest = vec![("other/model.onnx".to_string(), "0".repeat(64))];
        let result = verify_downloaded(&manifest, "name/model.onnx", &file, embed_err);
        assert!(result.is_err(), "unlisted preset artifacts must fail closed");
        verify_downloaded(&manifest, "", &file, embed_err).expect("empty path is a no-op");
    }

    #[test]
    fn verify_downloaded_allows_unlisted_custom_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let file = tmp.path().join("model.onnx");
        std::fs::write(&file, b"caller-managed").unwrap();
        verify_downloaded(&[], "model.onnx", &file, embed_err).expect("custom repos have no built-in manifest");
    }
}
