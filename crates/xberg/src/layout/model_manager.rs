//! Model downloading and caching for layout detection.
//!
//! Downloads ONNX models from HuggingFace Hub and caches them locally.
//! Uses shared download/checksum utilities from [`crate::model_download`].

use std::fs;
use std::path::{Path, PathBuf};

use crate::layout::error::LayoutError;
use crate::model_download;

/// Monotonic counter giving each `atomic_publish` call a unique temp filename so concurrent
/// publishes of the same model never collide on the staging path.
static PUBLISH_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

struct StagedFile {
    path: PathBuf,
}

impl StagedFile {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn publish_lock(path: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut locks = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(locks.entry(path.to_path_buf()).or_default())
}

#[cfg(any(windows, test))]
fn finalize_verified_publish(dst: &Path, backup: &Path, sha256: &str, label: &str) -> Result<(), String> {
    model_download::verify_sha256(dst, sha256, label)?;
    if backup.exists()
        && let Err(error) = fs::remove_file(backup)
    {
        tracing::warn!(
            path = %backup.display(),
            destination = %dst.display(),
            %error,
            "published model is valid, but stale rollback backup could not be removed"
        );
    }
    Ok(())
}

#[cfg(any(windows, test))]
fn publish_with_rollback(tmp: &Path, dst: &Path, backup: &Path, sha256: &str, label: &str) -> Result<(), String> {
    let had_destination = dst.exists();
    if had_destination && let Err(error) = fs::rename(dst, backup) {
        let cleanup = fs::remove_file(tmp).err();
        return Err(match cleanup {
            Some(cleanup) => format!(
                "Failed to prepare cached model replacement at {}: {error}; failed to clean staged file {}: {cleanup}",
                dst.display(),
                tmp.display()
            ),
            None => format!(
                "Failed to prepare cached model replacement at {}: {error}",
                dst.display()
            ),
        });
    }

    match fs::rename(tmp, dst) {
        Ok(()) => finalize_verified_publish(dst, backup, sha256, label),
        Err(error) => {
            let cleanup_error = fs::remove_file(tmp).err();
            if model_download::verify_sha256(dst, sha256, label).is_ok() {
                finalize_verified_publish(dst, backup, sha256, label)?;
                return Ok(());
            }
            let mut rollback_error = None;
            if had_destination {
                if dst.exists()
                    && let Err(remove_error) = fs::remove_file(dst)
                {
                    rollback_error = Some(format!("remove failed destination {}: {remove_error}", dst.display()));
                }
                if rollback_error.is_none()
                    && let Err(restore_error) = fs::rename(backup, dst)
                {
                    rollback_error = Some(format!(
                        "restore {} to {}: {restore_error}",
                        backup.display(),
                        dst.display()
                    ));
                }
            }
            let mut message = format!("Failed to publish model to {}: {error}", dst.display());
            if let Some(cleanup) = cleanup_error {
                message.push_str(&format!("; failed to clean staged file {}: {cleanup}", tmp.display()));
            }
            if let Some(rollback) = rollback_error {
                message.push_str(&format!("; rollback failed: {rollback}"));
            }
            Err(message)
        }
    }
}

/// Publish `src` to `dst` without exposing a partial file.
///
/// Copies `src` into a per-call temp file inside `dst_dir` (same filesystem as `dst`),
/// re-verifies the staged copy's checksum, then renames it into place. Concurrent callers —
/// e.g. GLM-OCR paired mode rasterizing pages in parallel, each constructing its own
/// [`LayoutModelManager`] — stage to private temps and serialize publication by destination.
/// Unix replacement is atomic; Windows uses a rollback backup because `std::fs::rename`
/// cannot replace an existing file there. Neither path can publish a torn copy.
/// A bounded advisory file lock extends that serialization across Xberg processes;
/// every waiter re-verifies the destination after acquiring it so a peer's valid
/// publication short-circuits its own write.
fn atomic_publish(src: &Path, dst: &Path, dst_dir: &Path, sha256: &str, label: &str) -> Result<(), String> {
    atomic_publish_with_lock_timeout(src, dst, dst_dir, sha256, label, None)
}

fn atomic_publish_with_lock_timeout(
    src: &Path,
    dst: &Path,
    dst_dir: &Path,
    sha256: &str,
    label: &str,
    lock_timeout: Option<std::time::Duration>,
) -> Result<(), String> {
    let stem = dst.file_name().and_then(|n| n.to_str()).unwrap_or("model");
    let lock = publish_lock(dst);
    let _guard = lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let file_lock_path = dst_dir.join(format!(".{stem}.xberg-publish.lock"));
    let _file_guard = match lock_timeout {
        Some(timeout) => model_download::acquire_artifact_file_lock_with_timeout(&file_lock_path, timeout)?,
        None => model_download::acquire_artifact_file_lock(&file_lock_path)?,
    };
    if model_download::verify_sha256(dst, sha256, label).is_ok() {
        return Ok(());
    }

    let tmp = StagedFile::new(dst_dir.join(format!(
        ".{stem}.{}.{}.tmp",
        std::process::id(),
        PUBLISH_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
    )));
    fs::copy(src, tmp.path()).map_err(|e| format!("Failed to stage model at {}: {e}", tmp.path().display()))?;
    model_download::verify_sha256(tmp.path(), sha256, label)?;

    // std::fs::rename replaces an existing file on Unix but not on Windows.
    // Serialize by destination and preserve the old file as a rollback backup
    // until the complete staged copy has been published. ~keep
    #[cfg(windows)]
    {
        let backup = tmp.path().with_extension("backup");
        publish_with_rollback(tmp.path(), dst, &backup, sha256, label)
    }

    #[cfg(not(windows))]
    fs::rename(tmp.path(), dst).map_err(|e| format!("Failed to publish model to {}: {e}", dst.display()))
}

#[cfg(feature = "paddle-ocr")]
use crate::paddle_ocr::ModelManifestEntry;

#[cfg(not(feature = "paddle-ocr"))]
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModelManifestEntry {
    pub relative_path: String,
    pub sha256: String,
    pub size_bytes: u64,
    pub source_url: String,
}

/// Model definition for a layout model.
#[derive(Debug, Clone)]
struct ModelDefinition {
    model_type: &'static str,
    hf_repo_id: &'static str,
    hf_revision: &'static str,
    remote_filename: &'static str,
    local_filename: &'static str,
    sha256_checksum: &'static str,
    size_bytes: u64,
}

fn hf_cache_relative_path(definition: &ModelDefinition) -> String {
    let repo = definition.hf_repo_id.replace('/', "--");
    format!(
        "models--{repo}/snapshots/{}/{}",
        definition.hf_revision, definition.remote_filename
    )
}

const MODELS: &[ModelDefinition] = &[
    ModelDefinition {
        model_type: "rtdetr",
        hf_repo_id: "xberg-io/layout-models",
        hf_revision: "c6bf493e2f7b0b9a29a5870da9880c14e20ff0a3",
        remote_filename: "rtdetr/model.onnx",
        local_filename: "model.onnx",
        sha256_checksum: "3bf2fb0ee6df87435b7ae47f0f3930ec3dc97ec56fd824acc6d57bc7a6b89ef2",
        size_bytes: 169_089_059,
    },
    ModelDefinition {
        model_type: "tatr",
        hf_repo_id: "xberg-io/layout-models",
        hf_revision: "c6bf493e2f7b0b9a29a5870da9880c14e20ff0a3",
        remote_filename: "tatr/model.onnx",
        local_filename: "tatr.onnx",
        sha256_checksum: "c11f4033da75e9c4d41c403ef356e89caa0a37a7d111b55461e7d5ba856bb6b6",
        size_bytes: 30_158_413,
    },
    ModelDefinition {
        model_type: "slanet_wired",
        hf_repo_id: "xberg-io/paddleocr-onnx-models",
        hf_revision: "bfaf0b492cfc1dee0c73245fc5860bfdcf2c3443",
        remote_filename: "v2/table/SLANeXt_wired.onnx",
        local_filename: "slanet_wired.onnx",
        sha256_checksum: "64990fa026a7e2e2c2d4ad2c810bc9c6992da76d5f91b54771dfc900927ca3d0",
        size_bytes: 365_355_622,
    },
    ModelDefinition {
        model_type: "slanet_wireless",
        hf_repo_id: "xberg-io/paddleocr-onnx-models",
        hf_revision: "bfaf0b492cfc1dee0c73245fc5860bfdcf2c3443",
        remote_filename: "v2/table/SLANeXt_wireless.onnx",
        local_filename: "slanet_wireless.onnx",
        sha256_checksum: "b29ae2b4fe0ff8bbf7efd73fda0951227eb1abaedcaa046ad016191c779b7766",
        size_bytes: 365_355_622,
    },
    ModelDefinition {
        model_type: "slanet_plus",
        hf_repo_id: "xberg-io/paddleocr-onnx-models",
        hf_revision: "bfaf0b492cfc1dee0c73245fc5860bfdcf2c3443",
        remote_filename: "v2/table/SLANet_plus.onnx",
        local_filename: "slanet_plus.onnx",
        sha256_checksum: "e48a401a4ebcddd47fe3822427db24d867a557324f58e438692f588bbe9231de",
        size_bytes: 7_781_309,
    },
    ModelDefinition {
        model_type: "table_classifier",
        hf_repo_id: "xberg-io/paddleocr-onnx-models",
        hf_revision: "bfaf0b492cfc1dee0c73245fc5860bfdcf2c3443",
        remote_filename: "v2/classifiers/PP-LCNet_x1_0_table_cls.onnx",
        local_filename: "table_cls.onnx",
        sha256_checksum: "f02bf087e924dadfb109e3b7887d7d56dc961b80e08c64cacf1030f97345b3c3",
        size_bytes: 6_775_213,
    },
    ModelDefinition {
        model_type: "pp_doclayout_v3",
        hf_repo_id: "xberg-io/layout-models",
        hf_revision: "c6bf493e2f7b0b9a29a5870da9880c14e20ff0a3",
        remote_filename: "pp_doclayout_v3/model.onnx",
        local_filename: "pp_doclayout_v3.onnx",
        sha256_checksum: "93d1197e55f1c9cb6720275a89684e7ea61cd5830008a837d8c51b19d47926c1",
        size_bytes: 131_731_131,
    },
];

/// All Hub revisions the runtime pins across its model set (deduplicated).
pub(crate) fn pinned_revisions() -> Vec<&'static str> {
    let mut revisions: Vec<&'static str> = MODELS.iter().map(|model| model.hf_revision).collect();
    revisions.sort_unstable();
    revisions.dedup();
    revisions
}

fn verify_model_file(path: &Path, expected_size: u64, expected_sha256: &str, label: &str) -> Result<(), String> {
    let actual_size = fs::metadata(path)
        .map_err(|error| format!("Failed to inspect cached {label} model: {error}"))?
        .len();
    if actual_size != expected_size {
        return Err(format!(
            "Size mismatch for {label}: expected {expected_size} bytes, got {actual_size}"
        ));
    }
    model_download::verify_sha256(path, expected_sha256, label)
}

fn resolve_verified_hf_model(definition: &ModelDefinition) -> Result<PathBuf, LayoutError> {
    let resolve = || {
        model_download::hf_download_revision(
            definition.hf_repo_id,
            definition.remote_filename,
            definition.hf_revision,
        )
        .map_err(LayoutError::ModelDownload)
    };

    let cached_path = resolve()?;
    if verify_model_file(
        &cached_path,
        definition.size_bytes,
        definition.sha256_checksum,
        definition.model_type,
    )
    .is_ok()
    {
        return Ok(cached_path);
    }

    tracing::warn!(
        model_type = definition.model_type,
        path = %cached_path.display(),
        "Cached Hugging Face model failed integrity validation; forcing refresh"
    );
    let refreshed = model_download::hf_force_download_revision(
        definition.hf_repo_id,
        definition.remote_filename,
        definition.hf_revision,
        definition.size_bytes,
        definition.sha256_checksum,
        definition.model_type,
    )
    .map_err(LayoutError::ModelDownload)?;
    verify_model_file(
        &refreshed,
        definition.size_bytes,
        definition.sha256_checksum,
        definition.model_type,
    )
    .map_err(LayoutError::ModelDownload)?;
    Ok(refreshed)
}

fn ensure_staged_model<F>(
    model_file: &Path,
    model_dir: &Path,
    expected_size: u64,
    expected_sha256: &str,
    label: &str,
    fetch: F,
) -> Result<PathBuf, LayoutError>
where
    F: FnOnce() -> Result<PathBuf, LayoutError>,
{
    if verify_model_file(model_file, expected_size, expected_sha256, label).is_ok() {
        tracing::debug!(model_type = label, "Verified layout model found in explicit cache");
        return Ok(model_file.to_path_buf());
    }
    if model_file.exists() {
        tracing::warn!(
            model_type = label,
            path = %model_file.display(),
            "Explicitly cached layout model failed integrity validation; replacing it"
        );
    }

    fs::create_dir_all(model_dir).map_err(|error| {
        LayoutError::ModelDownload(format!("Failed to create cache dir {}: {error}", model_dir.display()))
    })?;
    let source = fetch()?;
    verify_model_file(&source, expected_size, expected_sha256, label).map_err(LayoutError::ModelDownload)?;
    atomic_publish(&source, model_file, model_dir, expected_sha256, label).map_err(LayoutError::ModelDownload)?;
    verify_model_file(model_file, expected_size, expected_sha256, label).map_err(LayoutError::ModelDownload)?;
    Ok(model_file.to_path_buf())
}

/// Manages layout model downloading, caching, and path resolution.
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone)]
pub struct LayoutModelManager {
    cache_dir: PathBuf,
    stage_in_xberg_cache: bool,
}

impl LayoutModelManager {
    /// Creates a new model manager.
    ///
    /// If `cache_dir` is None, uses the default cache directory:
    /// the standard Hugging Face cache is used directly, without duplicating
    /// model files in Xberg's cache. Passing an explicit directory preserves
    /// the standalone Xberg cache layout for callers that need it.
    pub fn new(cache_dir: Option<PathBuf>) -> Self {
        match cache_dir {
            Some(cache_dir) => Self {
                cache_dir,
                stage_in_xberg_cache: true,
            },
            None => Self {
                cache_dir: hf_hub::resolve_cache_dir(),
                stage_in_xberg_cache: false,
            },
        }
    }

    /// Ensure the RT-DETR model (Docling Heron) exists locally, downloading if needed.
    pub fn ensure_rtdetr_model(&self) -> Result<PathBuf, LayoutError> {
        self.ensure_model("rtdetr")
    }

    fn ensure_model(&self, model_type: &str) -> Result<PathBuf, LayoutError> {
        let definition = MODELS
            .iter()
            .find(|m| m.model_type == model_type)
            .ok_or_else(|| LayoutError::ModelDownload(format!("Unknown model type: {model_type}")))?;

        if !self.stage_in_xberg_cache {
            return resolve_verified_hf_model(definition);
        }

        let model_dir = self.cache_dir.join(model_type);
        let model_file = model_dir.join(definition.local_filename);
        ensure_staged_model(
            &model_file,
            &model_dir,
            definition.size_bytes,
            definition.sha256_checksum,
            model_type,
            || resolve_verified_hf_model(definition),
        )
    }

    /// Check if the RT-DETR model is cached.
    pub fn is_rtdetr_cached(&self) -> bool {
        self.is_model_cached("rtdetr")
    }

    /// Ensure the TATR table structure recognition model exists locally, downloading if needed.
    pub fn ensure_tatr_model(&self) -> Result<PathBuf, LayoutError> {
        self.ensure_model("tatr")
    }

    /// Check if the TATR model is cached.
    pub fn is_tatr_cached(&self) -> bool {
        self.is_model_cached("tatr")
    }

    /// Ensure a SLANeXT table structure model exists locally, downloading if needed.
    ///
    /// `variant` must be one of: `"slanet_wired"`, `"slanet_wireless"`, `"slanet_plus"`.
    pub fn ensure_slanet_model(&self, variant: &str) -> Result<PathBuf, LayoutError> {
        self.ensure_model(variant)
    }

    /// Ensure the table classifier model exists locally, downloading if needed.
    pub fn ensure_table_classifier(&self) -> Result<PathBuf, LayoutError> {
        self.ensure_model("table_classifier")
    }

    /// Ensure the PP-DocLayout-V3 layout detection model exists locally, downloading if needed.
    pub fn ensure_pp_doclayout_v3_model(&self) -> Result<PathBuf, LayoutError> {
        self.ensure_model("pp_doclayout_v3")
    }

    /// Check if the PP-DocLayout-V3 model is cached.
    pub fn is_pp_doclayout_v3_cached(&self) -> bool {
        self.is_model_cached("pp_doclayout_v3")
    }

    /// Get the cache directory path.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    fn is_model_cached(&self, model_type: &str) -> bool {
        let Some(definition) = MODELS.iter().find(|model| model.model_type == model_type) else {
            return false;
        };
        if self.stage_in_xberg_cache {
            let path = self.cache_dir.join(model_type).join(definition.local_filename);
            return verify_model_file(
                &path,
                definition.size_bytes,
                definition.sha256_checksum,
                definition.model_type,
            )
            .is_ok();
        }
        model_download::hf_cached_revision(
            definition.hf_repo_id,
            definition.remote_filename,
            definition.hf_revision,
        )
        .ok()
        .flatten()
        .is_some_and(|path| {
            verify_model_file(
                &path,
                definition.size_bytes,
                definition.sha256_checksum,
                definition.model_type,
            )
            .is_ok()
        })
    }

    /// Returns the manifest of all layout model files with checksums and sizes.
    ///
    /// Paths are relative to the standard Hugging Face cache root returned by
    /// [`Self::cache_dir`] on a default manager. Explicit-cache managers stage
    /// the same artifacts under their documented standalone layout instead.
    pub fn manifest() -> Vec<ModelManifestEntry> {
        MODELS
            .iter()
            .map(|model| ModelManifestEntry {
                relative_path: hf_cache_relative_path(model),
                sha256: model.sha256_checksum.to_string(),
                size_bytes: model.size_bytes,
                source_url: format!(
                    "https://huggingface.co/{}/resolve/{}/{}",
                    model.hf_repo_id, model.hf_revision, model.remote_filename
                ),
            })
            .collect()
    }

    /// Ensures the default layout models (RT-DETR + TATR) are downloaded and cached.
    ///
    /// This downloads only the core models needed for basic layout detection and table
    /// structure recognition. Use [`Self::ensure_all_models`] to also download the larger
    /// SLANeXT variants (~730MB).
    pub fn ensure_default_models(&self) -> Result<(), LayoutError> {
        self.ensure_model("rtdetr")?;
        self.ensure_model("tatr")?;
        tracing::info!("Default layout models (rtdetr, tatr) ready");
        Ok(())
    }

    /// Ensures all layout models are downloaded and cached.
    ///
    /// Downloads RT-DETR, TATR, and all SLANeXT table structure variants (~730MB).
    /// For a lighter download that omits SLANeXT, use [`Self::ensure_default_models`].
    pub fn ensure_all_models(&self) -> Result<(), LayoutError> {
        for model in MODELS {
            self.ensure_model(model.model_type)?;
        }
        tracing::info!("All layout models ready");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn atomic_publish_yields_complete_file_under_concurrency() {
        use std::sync::Arc;

        let dir = TempDir::new().unwrap();
        let dst_dir = dir.path().to_path_buf();
        let src = dst_dir.join("source.bin");
        let payload: Vec<u8> = (0..1024 * 1024).map(|i| (i % 251) as u8).collect();
        fs::write(&src, &payload).unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(&payload);
            hex::encode(hasher.finalize())
        };

        let src = Arc::new(src);
        let dst = Arc::new(dst_dir.join("published.onnx"));
        let dst_dir = Arc::new(dst_dir);
        let sha = Arc::new(sha);

        let handles: Vec<_> = (0..16)
            .map(|_| {
                let (src, dst, dst_dir, sha) = (src.clone(), dst.clone(), dst_dir.clone(), sha.clone());
                std::thread::spawn(move || {
                    atomic_publish(&src, &dst, &dst_dir, &sha, "test-model").expect("publish must succeed");
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }

        assert_eq!(fs::read(&*dst).unwrap(), payload, "published file must equal source");

        let leftovers: Vec<_> = fs::read_dir(&*dst_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "no .tmp staging files should remain, found {leftovers:?}"
        );
    }

    #[test]
    fn atomic_publish_rejects_checksum_mismatch_and_leaves_no_partial() {
        let dir = TempDir::new().unwrap();
        let dst_dir = dir.path().to_path_buf();
        let src = dst_dir.join("source.bin");
        fs::write(&src, b"some model bytes").unwrap();
        let dst = dst_dir.join("published.onnx");

        let err = atomic_publish(&src, &dst, &dst_dir, &"0".repeat(64), "test-model")
            .expect_err("mismatched checksum must fail");
        assert!(err.contains("Checksum mismatch"), "unexpected error: {err}");
        assert!(!dst.exists(), "no file may be published on checksum failure");

        let leftovers: Vec<_> = fs::read_dir(&dst_dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "staging temp must be cleaned up on failure, found {leftovers:?}"
        );
    }

    #[test]
    fn atomic_publish_lock_timeout_leaves_no_staged_file() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("published.onnx");
        let payload = b"complete model";
        fs::write(&src, payload).unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(payload))
        };
        let lock_path = dir.path().join(".published.onnx.xberg-publish.lock");
        let _held = model_download::acquire_artifact_file_lock(&lock_path).unwrap();

        let error = atomic_publish_with_lock_timeout(
            &src,
            &dst,
            dir.path(),
            &sha,
            "test-model",
            Some(std::time::Duration::from_millis(50)),
        )
        .unwrap_err();

        assert!(error.contains("Timed out"), "unexpected error: {error}");
        let staged: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(staged.is_empty(), "lock failure leaked staging files: {staged:?}");
        assert!(!dst.exists());
    }

    #[test]
    fn atomic_publish_replaces_corrupt_destination() {
        let dir = TempDir::new().unwrap();
        let src = dir.path().join("source.bin");
        let dst = dir.path().join("published.onnx");
        let payload = b"complete verified model bytes";
        fs::write(&src, payload).unwrap();
        fs::write(&dst, b"corrupt").unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(payload))
        };

        atomic_publish(&src, &dst, dir.path(), &sha, "test-model").unwrap();

        assert_eq!(fs::read(dst).unwrap(), payload);
    }

    #[test]
    fn rollback_publish_replaces_destination_and_cleans_backup() {
        let dir = TempDir::new().unwrap();
        let tmp = dir.path().join("staged.tmp");
        let dst = dir.path().join("model.onnx");
        let backup = dir.path().join("model.backup");
        fs::write(&tmp, b"new model").unwrap();
        fs::write(&dst, b"old model").unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(b"new model"))
        };

        publish_with_rollback(&tmp, &dst, &backup, &sha, "test-model").unwrap();

        assert_eq!(fs::read(dst).unwrap(), b"new model");
        assert!(!backup.exists());
    }

    #[test]
    fn rollback_publish_restores_destination_when_publish_fails() {
        let dir = TempDir::new().unwrap();
        let missing_tmp = dir.path().join("missing.tmp");
        let dst = dir.path().join("model.onnx");
        let backup = dir.path().join("model.backup");
        fs::write(&dst, b"old model").unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(b"new model"))
        };

        let error = publish_with_rollback(&missing_tmp, &dst, &backup, &sha, "test-model").unwrap_err();

        assert!(error.contains("Failed to publish model"));
        assert_eq!(fs::read(dst).unwrap(), b"old model");
        assert!(!backup.exists());
    }

    #[test]
    fn valid_publish_survives_backup_cleanup_failure() {
        let dir = TempDir::new().unwrap();
        let dst = dir.path().join("model.onnx");
        let backup = dir.path().join("model.backup");
        let payload = b"valid model";
        fs::write(&dst, payload).unwrap();
        fs::create_dir(&backup).unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(payload))
        };

        finalize_verified_publish(&dst, &backup, &sha, "test-model").unwrap();

        assert_eq!(fs::read(dst).unwrap(), payload);
        assert!(backup.is_dir(), "unremovable backup is retained for later cleanup");
    }

    #[test]
    fn test_layout_model_manager_creation() {
        let temp_dir = TempDir::new().unwrap();
        let manager = LayoutModelManager::new(Some(temp_dir.path().to_path_buf()));
        assert_eq!(manager.cache_dir(), temp_dir.path());
    }

    #[test]
    fn default_manager_reports_standard_hugging_face_cache() {
        let manager = LayoutModelManager::new(None);
        assert_eq!(manager.cache_dir(), hf_hub::resolve_cache_dir());
        assert!(!manager.stage_in_xberg_cache);
    }

    #[test]
    fn test_is_rtdetr_cached_empty() {
        let temp_dir = TempDir::new().unwrap();
        let manager = LayoutModelManager::new(Some(temp_dir.path().to_path_buf()));
        assert!(!manager.is_rtdetr_cached());
    }

    #[test]
    fn test_is_rtdetr_cached_rejects_truncated_file() {
        let temp_dir = TempDir::new().unwrap();
        let manager = LayoutModelManager::new(Some(temp_dir.path().to_path_buf()));

        let dir = temp_dir.path().join("rtdetr");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("model.onnx"), "fake").unwrap();

        assert!(!manager.is_rtdetr_cached());
    }

    #[test]
    fn test_is_tatr_cached_empty() {
        let temp_dir = TempDir::new().unwrap();
        let manager = LayoutModelManager::new(Some(temp_dir.path().to_path_buf()));
        assert!(!manager.is_tatr_cached());
    }

    #[test]
    fn test_is_tatr_cached_rejects_truncated_file() {
        let temp_dir = TempDir::new().unwrap();
        let manager = LayoutModelManager::new(Some(temp_dir.path().to_path_buf()));

        let dir = temp_dir.path().join("tatr");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("tatr.onnx"), "fake").unwrap();

        assert!(!manager.is_tatr_cached());
    }

    #[test]
    fn test_manifest_returns_all_layout_models() {
        let entries = LayoutModelManager::manifest();
        assert_eq!(entries.len(), 7);

        let paths: Vec<&str> = entries.iter().map(|e| e.relative_path.as_str()).collect();
        assert!(paths.iter().all(|path| path.starts_with("models--")));
        assert!(paths.iter().all(|path| path.contains("/snapshots/")));
        assert!(paths.iter().any(|path| path.ends_with("/rtdetr/model.onnx")));
        assert!(paths.iter().any(|path| path.ends_with("/tatr/model.onnx")));
        assert!(paths.iter().any(|path| path.ends_with("/v2/table/SLANeXt_wired.onnx")));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("/v2/table/SLANeXt_wireless.onnx"))
        );
        assert!(paths.iter().any(|path| path.ends_with("/v2/table/SLANet_plus.onnx")));
        assert!(
            paths
                .iter()
                .any(|path| path.ends_with("/v2/classifiers/PP-LCNet_x1_0_table_cls.onnx"))
        );
        assert!(paths.iter().any(|path| path.ends_with("/pp_doclayout_v3/model.onnx")));
    }

    #[test]
    fn test_manifest_entries_have_valid_fields() {
        let entries = LayoutModelManager::manifest();

        for entry in &entries {
            assert!(
                !entry.sha256.is_empty(),
                "SHA256 should not be empty for {}",
                entry.relative_path
            );
            assert!(
                entry.size_bytes > 0,
                "Size should be non-zero for {}",
                entry.relative_path
            );
            assert!(
                entry.source_url.starts_with("https://huggingface.co/"),
                "Source URL should be a HuggingFace URL"
            );
            assert!(
                !entry.source_url.contains("/resolve/main/"),
                "Source URL must pin a revision"
            );
            assert!(entry.relative_path.starts_with("models--"));
            assert!(entry.relative_path.contains("/snapshots/"));
        }
    }

    #[test]
    fn corrupt_explicit_cache_is_replaced_from_verified_source() {
        let temp_dir = TempDir::new().unwrap();
        let model_dir = temp_dir.path().join("model");
        let model_file = model_dir.join("model.onnx");
        let source = temp_dir.path().join("source.onnx");
        let payload = b"complete verified model bytes";
        fs::create_dir_all(&model_dir).unwrap();
        fs::write(&model_file, b"truncated").unwrap();
        fs::write(&source, payload).unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(payload))
        };

        let resolved = ensure_staged_model(
            &model_file,
            &model_dir,
            payload.len() as u64,
            &sha,
            "test-model",
            || Ok(source),
        )
        .unwrap();

        assert_eq!(resolved, model_file);
        assert_eq!(fs::read(resolved).unwrap(), payload);
    }

    #[test]
    fn verified_explicit_cache_skips_fetch() {
        let temp_dir = TempDir::new().unwrap();
        let model_dir = temp_dir.path().join("model");
        let model_file = model_dir.join("model.onnx");
        let payload = b"complete verified model bytes";
        fs::create_dir_all(&model_dir).unwrap();
        fs::write(&model_file, payload).unwrap();
        let sha = {
            use sha2::{Digest, Sha256};
            hex::encode(Sha256::digest(payload))
        };

        let resolved = ensure_staged_model(
            &model_file,
            &model_dir,
            payload.len() as u64,
            &sha,
            "test-model",
            || panic!("verified staged model must not fetch"),
        )
        .unwrap();

        assert_eq!(resolved, model_file);
    }
}
