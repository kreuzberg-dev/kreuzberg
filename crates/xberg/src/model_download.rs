//! Shared utilities for resolving and verifying model artifacts from Hugging Face Hub.

use std::time::Duration;

#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
use sha2::{Digest, Sha256};
#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
use std::io::{BufReader, Read};
#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
use std::path::Path;
#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
use std::path::PathBuf;

/// Default wall-clock ceiling for a single model-file download. This is a *total* deadline covering
/// the whole transfer, so it stays generous — a cold GB-scale model legitimately takes minutes — and
/// serves only as a final backstop; override with `XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS`. Fast failure
/// on a dead/blackholed network comes instead from the bounded `connect_timeout` and lowered retry
/// count on the client built by [`hf_client_builder`], not from shortening this deadline (which would
/// break legitimate slow downloads).
#[allow(dead_code)]
const DEFAULT_MODEL_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(300);

#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
static QUARANTINE_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Per-connect ceiling for HuggingFace requests. On a host that advertises an IPv6 default route but
/// blackholes IPv6 (common corporate config), a connect to an AAAA address otherwise parks in TCP
/// `SYN_SENT` until the OS SYN timeout (~75 s) with no happy-eyeballs/IPv4 race — see #1249. Bounding
/// the connect lets hf-hub's retry fail over quickly instead of burning the total deadline.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
const HF_CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Max connect/transient retry attempts for HuggingFace requests. hf-hub's default is 5, which
/// multiplies a blackholed-connect stall fivefold. Lowering it bounds *both* hf-hub retry loops —
/// the metadata `HEAD` (via hf-hub's internal, non-overridable client) and the blob `GET` — since
/// they share one `RetryConfig`. Two attempts still tolerate a single transient blip.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
const HF_MAX_RETRY_ATTEMPTS: usize = 2;

/// A DNS resolver that orders IPv4 (`A`-record) addresses ahead of IPv6 (`AAAA`) ones.
///
/// This is the IPv4 fallback for #1249: on hosts that advertise an IPv6 default route but blackhole
/// IPv6, the default resolution order can hand the connector an `AAAA` address first, which then
/// stalls in `SYN_SENT`. Returning IPv4 first lets the connector reach a dual-stack host over IPv4,
/// while still returning `AAAA` addresses afterwards so genuinely IPv6-only hosts (which resolve to
/// `AAAA` only) keep working — unlike binding an IPv4 `local_address`, which would break them.
/// Resolution runs on a blocking thread because `getaddrinfo` is synchronous; DNS itself is not the
/// blackholed path (that's the TCP connect), so this stays fast.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
#[derive(Debug, Default)]
struct Ipv4FirstResolver;

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
impl reqwest::dns::Resolve for Ipv4FirstResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            // Port 0 is a placeholder; reqwest overrides it with the request's real port. ~keep
            let mut addrs = tokio::task::spawn_blocking(move || {
                std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), 0_u16))
                    .map(|iter| iter.collect::<Vec<std::net::SocketAddr>>())
            })
            .await??;
            order_ipv4_first(&mut addrs);
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// Reorder resolved addresses so IPv4 (`A`) entries precede IPv6 (`AAAA`) ones, preserving the
/// relative order within each family. Stable so the resolver stays deterministic for a given
/// `getaddrinfo` result. See [`Ipv4FirstResolver`] for why (the #1249 IPv4 fallback).
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
fn order_ipv4_first(addrs: &mut [std::net::SocketAddr]) {
    addrs.sort_by_key(std::net::SocketAddr::is_ipv6);
}

/// Build an [`hf_hub::HFClientBuilder`] pre-configured for resilience on hostile networks: a
/// `reqwest::Client` with a bounded [`HF_CONNECT_TIMEOUT`], an IPv4-first DNS resolver
/// ([`Ipv4FirstResolver`]), and [`HF_MAX_RETRY_ATTEMPTS`] retries, injected as the transfer client.
/// Callers chain `.cache_dir(...)` / `.build_sync()` as needed.
///
/// The injected client only overrides hf-hub's main `GET` client; its internal `no_redirect_client`
/// (used for the metadata `HEAD`) is not overridable, so the lowered retry count is what bounds the
/// `HEAD` path. The HF auth token is applied per-request by hf-hub (not via client default headers),
/// so injecting our own client does not disturb `HF_TOKEN`-gated downloads. If the client fails to
/// build we fall back to hf-hub's default (unbounded) client rather than failing the download.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
pub(crate) fn hf_client_builder() -> hf_hub::HFClientBuilder {
    let builder = hf_hub::HFClientBuilder::new().retry_max_attempts(HF_MAX_RETRY_ATTEMPTS);
    match reqwest::Client::builder()
        .connect_timeout(HF_CONNECT_TIMEOUT)
        .dns_resolver(Ipv4FirstResolver)
        .build()
    {
        Ok(client) => builder.client(client),
        Err(error) => {
            tracing::warn!(
                target: "xberg::model_download",
                %error,
                "failed to build HF http client with connect timeout; using hf-hub default client"
            );
            builder
        }
    }
}

/// Resolve the model-download deadline, honoring `XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS` (seconds; a
/// value of 0 or unparseable falls back to the default).
#[allow(dead_code)]
pub(crate) fn model_download_timeout() -> Duration {
    std::env::var("XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&s| s > 0)
        .map(Duration::from_secs)
        .unwrap_or(DEFAULT_MODEL_DOWNLOAD_TIMEOUT)
}

/// Whether Hugging Face network access is explicitly disabled for this process.
///
/// Match the boolean spellings accepted by the Python Hugging Face clients while
/// honoring both the current and legacy environment variable names.
#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
pub(crate) fn hf_offline_mode() -> bool {
    ["HF_HUB_OFFLINE", "HUGGINGFACE_HUB_OFFLINE"]
        .iter()
        .filter_map(std::env::var_os)
        .any(|value| env_flag_enabled(&value))
}

#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
fn env_flag_enabled(value: &std::ffi::OsStr) -> bool {
    value
        .to_str()
        .is_some_and(|value| matches!(value.trim().to_ascii_lowercase().as_str(), "1" | "on" | "yes" | "true"))
}

#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
fn offline_cache_miss(repo_id: &str, remote_filename: &str, revision: Option<&str>) -> String {
    format!(
        "Hugging Face offline mode is enabled and '{remote_filename}' from {repo_id}@{} is not available in the local cache",
        revision.unwrap_or("main")
    )
}

/// Run a blocking model-download closure under a hard wall-clock deadline so a hung network cannot
/// block the pipeline indefinitely. The closure runs on a detached worker thread; if it does not
/// finish within `model_download_timeout()` we log a warning and return `Err`, letting the caller
/// degrade (skip the model-backed backend) rather than hang. The worker thread cannot be
/// force-killed — it stays parked on the socket until the OS tears the connection down — but it
/// holds no lock the pipeline needs, so progress resumes. `label` names the fetch in the log/error.
#[allow(dead_code)]
pub(crate) fn with_download_deadline<T, F>(label: &str, f: F) -> Result<T, String>
where
    F: FnOnce() -> Result<T, String> + Send + 'static,
    T: Send + 'static,
{
    let deadline = model_download_timeout();
    let (tx, rx) = std::sync::mpsc::sync_channel::<Result<T, String>>(1);
    std::thread::Builder::new()
        .name("xberg-model-download".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| format!("failed to spawn model-download thread: {e}"))?;
    match rx.recv_timeout(deadline) {
        Ok(result) => result,
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
            tracing::warn!(
                target: "xberg::model_download",
                label = %label,
                timeout_secs = deadline.as_secs(),
                "model download exceeded deadline (network unreachable / firewalled?); aborting so \
                 the extraction pipeline does not hang. Set XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS to adjust."
            );
            Err(format!(
                "model download '{label}' timed out after {}s (HuggingFace unreachable?)",
                deadline.as_secs()
            ))
        }
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
            Err(format!("model-download thread for '{label}' died unexpectedly"))
        }
    }
}

/// Return the process-wide lock guarding downloads of a single `(repo, file)`.
///
/// hf-hub takes a file lock on the blob it is fetching and *errors* ("Lock
/// acquisition failed") rather than waiting when a second thread races the same
/// uncached file — so two tests (or two parallel-page OCR workers) that both need
/// the same cold model can knock each other out. Serializing above hf-hub, keyed on
/// the exact file, lets the first thread populate the cache while the rest wait and
/// then get the warm copy; downloads of *different* files still run in parallel.
#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
fn download_lock(key: &str) -> std::sync::Arc<std::sync::Mutex<()>> {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};

    static LOCKS: OnceLock<Mutex<HashMap<String, Arc<Mutex<()>>>>> = OnceLock::new();
    let mut map = LOCKS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    Arc::clone(map.entry(key.to_string()).or_default())
}

/// Held advisory lock for model-cache mutations shared by all Xberg processes.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
#[derive(Debug)]
pub(crate) struct ArtifactFileLock {
    file: std::fs::File,
    path: PathBuf,
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
impl Drop for ArtifactFileLock {
    fn drop(&mut self) {
        if let Err(error) = fs2::FileExt::unlock(&self.file) {
            tracing::warn!(path = %self.path.display(), %error, "failed to release model-cache file lock");
        }
    }
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
pub(crate) fn acquire_artifact_file_lock(path: &Path) -> Result<ArtifactFileLock, String> {
    acquire_artifact_file_lock_with_timeout(path, model_download_timeout())
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
pub(crate) fn acquire_artifact_file_lock_with_timeout(
    path: &Path,
    timeout: Duration,
) -> Result<ArtifactFileLock, String> {
    const LOCK_RETRY_INTERVAL: Duration = Duration::from_millis(50);

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| {
            format!(
                "Failed to create model-cache lock directory {}: {error}",
                parent.display()
            )
        })?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|error| format!("Failed to open model-cache lock {}: {error}", path.display()))?;
    let started = std::time::Instant::now();
    loop {
        match fs2::FileExt::try_lock_exclusive(&file) {
            Ok(()) => {
                return Ok(ArtifactFileLock {
                    file,
                    path: path.to_path_buf(),
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock && started.elapsed() < timeout => {
                std::thread::sleep(LOCK_RETRY_INTERVAL.min(timeout.saturating_sub(started.elapsed())));
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                return Err(format!(
                    "Timed out after {}s waiting for model-cache lock {}",
                    timeout.as_secs_f64(),
                    path.display()
                ));
            }
            Err(error) => {
                return Err(format!(
                    "Failed to acquire model-cache lock {}: {error}",
                    path.display()
                ));
            }
        }
    }
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
fn hf_artifact_lock_path(repo_id: &str, cache_dir: Option<&Path>, expected_sha256: &str) -> Result<PathBuf, String> {
    if !is_sha256_hex(expected_sha256) {
        return Err("Cannot construct Hugging Face artifact lock for an invalid SHA-256".to_string());
    }
    Ok(cache_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(hf_hub::resolve_cache_dir)
        .join(format!("models--{}", repo_id.replace('/', "--")))
        .join(format!(".xberg-{}.lock", expected_sha256.to_ascii_lowercase())))
}

#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Build an hf-hub client using its standard cache resolution unless the caller
/// explicitly supplied an alternative Hugging Face cache root.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings",
        all(test, feature = "layout-detection")
    )
))]
#[allow(dead_code)]
fn hf_client(cache_dir: Option<&Path>) -> Result<hf_hub::HFClientSync, String> {
    let builder = hf_client_builder();
    let builder = match cache_dir {
        Some(path) => builder.cache_dir(path.to_path_buf()),
        None => builder,
    };
    builder
        .build_sync()
        .map_err(|error| format!("Failed to initialize Hugging Face Hub client: {error}"))
}

/// Resolve the effective Hugging Face cache root for in-process model cache keys.
///
/// This does not create an Xberg cache or stage files. It mirrors the root used
/// by [`hf_client`] so changing standard HF environment configuration cannot
/// accidentally reuse an engine loaded from a different snapshot cache.
#[cfg(any(
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
pub(crate) fn hf_cache_key(cache_dir: Option<&Path>) -> String {
    cache_dir
        .map(Path::to_path_buf)
        .unwrap_or_else(hf_hub::resolve_cache_dir)
        .display()
        .to_string()
}

/// Resolve an artifact strictly from an existing Hugging Face cache entry.
#[cfg(feature = "transcription")]
pub(crate) fn hf_cached_file(
    repo_id: &str,
    remote_filename: &str,
    revision: Option<&str>,
    cache_dir: Option<&Path>,
) -> Result<Option<PathBuf>, String> {
    let api = hf_client(cache_dir)?;
    hf_cached_revision_with_client(&api, repo_id, remote_filename, revision)
}

/// Resolve one model artifact through the Hugging Face cache.
///
/// With `cache_dir == None`, hf-hub owns cache discovery and follows the standard
/// `HF_HUB_CACHE` / `HUGGINGFACE_HUB_CACHE` / `HF_HOME` / XDG precedence. A
/// supplied directory is an explicit alternate *Hugging Face cache root* and
/// retains hf-hub's normal content-addressed layout; Xberg never stages a copy.
/// Cache lookup is always local-first. Both Hugging Face offline variables are
/// honored before any network request. When `expected_sha256` is supplied, a bad
/// cached entry is force-refreshed and the replacement is verified before use.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings",
        all(test, feature = "layout-detection")
    )
))]
#[allow(dead_code)]
pub(crate) fn hf_resolve_file(
    repo_id: &str,
    remote_filename: &str,
    revision: Option<&str>,
    cache_dir: Option<&Path>,
    expected_sha256: Option<&str>,
) -> Result<PathBuf, String> {
    if let Some(expected) = expected_sha256
        && !is_sha256_hex(expected)
    {
        return Err(format!("Invalid SHA-256 for {repo_id}/{remote_filename}"));
    }

    let api = hf_client(cache_dir)?;
    let cached = hf_cached_revision_with_client(&api, repo_id, remote_filename, revision)?;
    let cached_is_valid = match (&cached, expected_sha256) {
        (Some(path), Some(expected)) => verify_sha256(path, expected, remote_filename).is_ok(),
        (Some(_), None) => true,
        (None, _) => false,
    };
    if cached_is_valid {
        return cached.ok_or_else(|| "validated Hugging Face cache entry disappeared".to_string());
    }
    if hf_offline_mode() {
        if cached.is_some() {
            return Err(format!(
                "Hugging Face offline mode is enabled and cached artifact '{remote_filename}' from {repo_id}@{} failed checksum verification; network repair is disabled",
                revision.unwrap_or("main")
            ));
        }
        return Err(offline_cache_miss(repo_id, remote_filename, revision));
    }

    let label = format!("{repo_id}/{remote_filename}@{}", revision.unwrap_or("main"));
    let file_lock = download_lock(&label);
    let repo = repo_id.to_string();
    let filename = remote_filename.to_string();
    let revision = revision.map(str::to_string);
    let expected = expected_sha256.map(str::to_string);
    let artifact_file_lock = expected_sha256
        .map(|sha256| hf_artifact_lock_path(repo_id, cache_dir, sha256))
        .transpose()?;
    let force_download = cached.is_some();
    with_download_deadline(&label, move || {
        let _guard = file_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _file_guard = artifact_file_lock
            .as_deref()
            .map(acquire_artifact_file_lock)
            .transpose()?;
        let current = hf_cached_revision_with_client(&api, &repo, &filename, revision.as_deref())?;
        if let Some(path) = current.as_deref() {
            match expected.as_deref() {
                Some(sha) if verify_sha256(path, sha, &filename).is_ok() => return Ok(path.to_path_buf()),
                None => return Ok(path.to_path_buf()),
                Some(_) => {}
            }
        }

        let (owner, name) = hf_hub::split_id(&repo);
        let repository = api.model(owner, name);
        let force = force_download || current.is_some();
        #[cfg(windows)]
        let quarantined = match (current.as_deref(), expected.as_deref()) {
            (Some(path), Some(sha)) if force => quarantine_hf_cache_entry(path, None, sha, &filename)?,
            _ => Vec::new(),
        };

        let result = match (revision.as_deref(), force) {
            (Some(revision), true) => repository
                .download_file()
                .filename(filename.clone())
                .revision(revision.to_string())
                .force_download(true)
                .send(),
            (Some(revision), false) => repository
                .download_file()
                .filename(filename.clone())
                .revision(revision.to_string())
                .send(),
            (None, true) => repository
                .download_file()
                .filename(filename.clone())
                .force_download(true)
                .send(),
            (None, false) => repository.download_file().filename(filename.clone()).send(),
        };
        let refreshed = result
            .map_err(|error| format!("Failed to download '{filename}' from {repo}: {error}"))
            .and_then(|path| {
                if let Some(sha) = expected.as_deref() {
                    verify_cached_artifact(&path, None, sha, &filename)?;
                }
                Ok(path)
            });

        #[cfg(windows)]
        match refreshed {
            Ok(path) => {
                remove_quarantined_entries(&quarantined);
                Ok(path)
            }
            Err(error) => {
                restore_quarantined_entries(&quarantined, None, expected.as_deref().unwrap_or_default(), &filename)
                    .map_err(|restore| {
                        format!("{error}; additionally failed to restore corrupt cache entry: {restore}")
                    })?;
                Err(error)
            }
        }
        #[cfg(not(windows))]
        refreshed
    })
}

/// Download a file from a HuggingFace Hub repository.
///
/// Uses `hf-hub`'s built-in caching so repeated calls for the same file are fast.
/// Concurrent calls for the same file serialize (see [`download_lock`]) so a cold
/// cache is populated once instead of racing hf-hub's blob lock.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl"
    )
))]
#[allow(dead_code)]
pub(crate) fn hf_download(repo_id: &str, remote_filename: &str) -> Result<PathBuf, String> {
    hf_download_at_revision(repo_id, remote_filename, None)
}

/// Resolve a pinned model artifact from the standard Hugging Face cache, falling
/// back to the network only on a cache miss.
#[cfg(all(not(target_arch = "wasm32"), layout_detection))]
pub(crate) fn hf_download_revision(repo_id: &str, remote_filename: &str, revision: &str) -> Result<PathBuf, String> {
    hf_download_at_revision(repo_id, remote_filename, Some(revision))
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
#[allow(dead_code)]
fn hf_download_at_revision(repo_id: &str, remote_filename: &str, revision: Option<&str>) -> Result<PathBuf, String> {
    tracing::info!(
        repo = repo_id,
        filename = remote_filename,
        revision,
        "Resolving via hf-hub"
    );

    let label = format!("{repo_id}/{remote_filename}@{}", revision.unwrap_or("main"));
    let api = hf_client_builder()
        .build_sync()
        .map_err(|e| format!("Failed to initialize HuggingFace Hub API: {e}"))?;
    if let Some(path) = hf_cached_revision_with_client(&api, repo_id, remote_filename, revision)? {
        return Ok(path);
    }
    if hf_offline_mode() {
        return Err(offline_cache_miss(repo_id, remote_filename, revision));
    }

    let file_lock = download_lock(&label);
    let filename = remote_filename.to_string();
    let repo_id = repo_id.to_string();
    let revision = revision.map(str::to_string);

    with_download_deadline(&label, move || {
        // Keep the per-artifact lock in the worker. If the caller's deadline
        // expires, the detached transfer continues to own the lock and a retry
        // cannot race hf-hub's in-flight cache publication. ~keep
        let _guard = file_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(path) = hf_cached_revision_with_client(&api, &repo_id, &filename, revision.as_deref())? {
            tracing::debug!(repo = repo_id, filename, revision, "Using standard Hugging Face cache");
            return Ok(path);
        }

        let (owner, name) = hf_hub::split_id(&repo_id);
        let repository = api.model(owner, name);
        let request = repository.download_file().filename(filename.clone());
        let result = match revision {
            Some(revision) => request.revision(revision).send(),
            None => request.send(),
        };
        result.map_err(|e| format!("Failed to download '{filename}' from {repo_id}: {e}"))
    })
}

/// Force a network refresh of a pinned artifact in the standard Hugging Face cache.
#[cfg(all(not(target_arch = "wasm32"), layout_detection))]
pub(crate) fn hf_force_download_revision(
    repo_id: &str,
    remote_filename: &str,
    revision: &str,
    expected_size: u64,
    expected_sha256: &str,
    label: &str,
) -> Result<PathBuf, String> {
    if hf_offline_mode() {
        return Err(format!(
            "Hugging Face offline mode is enabled; cannot refresh '{remote_filename}' from {repo_id}@{revision}"
        ));
    }
    let artifact_key = format!("{repo_id}/{remote_filename}@{revision}");
    let file_lock = download_lock(&artifact_key);
    let artifact_file_lock = hf_artifact_lock_path(repo_id, None, expected_sha256)?;
    let filename = remote_filename.to_string();
    let repo_id = repo_id.to_string();
    let revision = revision.to_string();
    let expected_sha256 = expected_sha256.to_string();
    let model_label = label.to_string();
    with_download_deadline(&artifact_key, move || {
        let _guard = file_lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let _file_guard = acquire_artifact_file_lock(&artifact_file_lock)?;
        let api = hf_client_builder()
            .build_sync()
            .map_err(|e| format!("Failed to initialize HuggingFace Hub API: {e}"))?;

        // Another caller may have repaired the entry while this caller waited.
        // Revalidate under the same artifact lock before forcing any network I/O. ~keep
        let cached = hf_cached_revision_with_client(&api, &repo_id, &filename, Some(&revision))?;
        if let Some(path) = verified_cached_path(cached.as_deref(), expected_size, &expected_sha256, &model_label) {
            return Ok(path);
        }

        #[cfg(windows)]
        let quarantined = match cached.as_deref() {
            Some(path) => quarantine_hf_cache_entry(path, Some(expected_size), &expected_sha256, &model_label)?,
            None => Vec::new(),
        };

        let (owner, name) = hf_hub::split_id(&repo_id);
        let refreshed = api
            .model(owner, name)
            .download_file()
            .filename(filename.clone())
            .revision(revision.clone())
            .force_download(true)
            .send()
            .map_err(|error| format!("Failed to refresh '{filename}' from {repo_id}@{revision}: {error}"))
            .and_then(|path| {
                verify_cached_artifact(&path, Some(expected_size), &expected_sha256, &model_label)
                    .map(|()| path)
                    .map_err(|error| format!("Refreshed artifact failed verification: {error}"))
            });

        #[cfg(windows)]
        match refreshed {
            Ok(path) => {
                remove_quarantined_entries(&quarantined);
                Ok(path)
            }
            Err(error) => {
                if let Ok(Some(peer_path)) = hf_cached_revision_with_client(&api, &repo_id, &filename, Some(&revision))
                    && verify_cached_artifact(&peer_path, Some(expected_size), &expected_sha256, &model_label).is_ok()
                {
                    remove_quarantined_entries(&quarantined);
                    return Ok(peer_path);
                }
                restore_quarantined_entries(&quarantined, Some(expected_size), &expected_sha256, &model_label)
                    .map_err(|restore| {
                        format!("{error}; additionally failed to restore corrupt cache entry: {restore}")
                    })?;
                Err(error)
            }
        }
        #[cfg(not(windows))]
        refreshed
    })
}

#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
fn verify_cached_artifact(
    path: &Path,
    expected_size: Option<u64>,
    expected_sha256: &str,
    label: &str,
) -> Result<(), String> {
    let actual_size = std::fs::metadata(path)
        .map_err(|error| format!("Failed to inspect cached {label}: {error}"))?
        .len();
    if let Some(expected_size) = expected_size
        && actual_size != expected_size
    {
        return Err(format!(
            "Size mismatch for {label}: expected {expected_size} bytes, got {actual_size}"
        ));
    }
    verify_sha256(path, expected_sha256, label)
}

#[cfg(all(not(target_arch = "wasm32"), layout_detection))]
fn verified_cached_path(
    path: Option<&Path>,
    expected_size: u64,
    expected_sha256: &str,
    label: &str,
) -> Option<PathBuf> {
    path.filter(|path| verify_cached_artifact(path, Some(expected_size), expected_sha256, label).is_ok())
        .map(Path::to_path_buf)
}

#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
#[derive(Debug)]
struct QuarantinedEntry {
    original: PathBuf,
    quarantine: PathBuf,
}

/// Move a corrupt snapshot entry and its backing blob aside before hf-hub refreshes it.
///
/// hf-hub's standard cache normally exposes a snapshot symlink into `blobs/`. Windows
/// cannot rename a replacement over an existing file, so both names must be absent.
/// The caller holds the process-local artifact lock. Cross-process repair remains
/// coordinated only by hf-hub's own blob lock.
#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
fn quarantine_hf_cache_entry(
    path: &Path,
    expected_size: Option<u64>,
    expected_sha256: &str,
    label: &str,
) -> Result<Vec<QuarantinedEntry>, String> {
    let canonical = std::fs::canonicalize(path).ok();
    let mut originals = vec![path.to_path_buf()];
    if let Some(blob) = canonical
        && blob != path
        && hf_blob_belongs_to_snapshot(path, &blob)
    {
        originals.push(blob);
    }
    if let Some(blob) = deterministic_hf_blob_path(path, expected_sha256)
        && !originals.contains(&blob)
        && blob.exists()
    {
        originals.push(blob);
    }

    let mut moved = Vec::with_capacity(originals.len());
    for original in originals {
        if !original.exists() && std::fs::symlink_metadata(&original).is_err() {
            continue;
        }
        let suffix = QUARANTINE_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let file_name = original.file_name().and_then(|name| name.to_str()).unwrap_or("model");
        let quarantine =
            original.with_file_name(format!(".{file_name}.xberg-corrupt.{}.{}", std::process::id(), suffix));
        if let Err(error) = std::fs::rename(&original, &quarantine) {
            if verify_cached_artifact(&original, expected_size, expected_sha256, label).is_ok() {
                remove_quarantined_entries(&moved);
                return Ok(Vec::new());
            }
            let restore_error = restore_quarantined_entries(&moved, expected_size, expected_sha256, label).err();
            return Err(match restore_error {
                Some(restore) => format!(
                    "Failed to quarantine corrupt cache entry {}: {error}; rollback also failed: {restore}",
                    original.display()
                ),
                None => format!(
                    "Failed to quarantine corrupt cache entry {}: {error}",
                    original.display()
                ),
            });
        }
        moved.push(QuarantinedEntry { original, quarantine });
    }
    Ok(moved)
}

#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
fn deterministic_hf_blob_path(snapshot: &Path, expected_sha256: &str) -> Option<PathBuf> {
    if !is_sha256_hex(expected_sha256) {
        return None;
    }
    let repo_root = snapshot
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "snapshots"))?
        .parent()?;
    let repo_root = std::fs::canonicalize(repo_root).ok()?;
    let blob = repo_root.join("blobs").join(expected_sha256.to_ascii_lowercase());
    blob.starts_with(repo_root.join("blobs")).then_some(blob)
}

#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
fn hf_blob_belongs_to_snapshot(snapshot: &Path, blob: &Path) -> bool {
    snapshot
        .ancestors()
        .find(|ancestor| ancestor.file_name().is_some_and(|name| name == "snapshots"))
        .and_then(Path::parent)
        .is_some_and(|repo_root| {
            let repo_root = std::fs::canonicalize(repo_root).unwrap_or_else(|_| repo_root.to_path_buf());
            blob.starts_with(repo_root.join("blobs"))
        })
}

#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
fn restore_quarantined_entries(
    entries: &[QuarantinedEntry],
    expected_size: Option<u64>,
    expected_sha256: &str,
    label: &str,
) -> Result<(), String> {
    // hf-hub does not honor Xberg's advisory lock. If it has installed either
    // the expected snapshot or blob since quarantine began, its publication is
    // authoritative and the old corrupt entries must not be restored over it. ~keep
    if entries
        .iter()
        .any(|entry| verify_cached_artifact(&entry.original, expected_size, expected_sha256, label).is_ok())
    {
        remove_quarantined_entries(entries);
        return Ok(());
    }

    let conflicting: Vec<_> = entries
        .iter()
        .filter(|entry| std::fs::symlink_metadata(&entry.original).is_ok())
        .map(|entry| entry.original.display().to_string())
        .collect();
    if !conflicting.is_empty() {
        return Err(format!(
            "refusing to replace cache entries created by another Hugging Face client: {}",
            conflicting.join(", ")
        ));
    }

    let mut failures = Vec::new();
    for entry in entries.iter().rev() {
        if std::fs::symlink_metadata(&entry.original).is_ok() {
            if verify_cached_artifact(&entry.original, expected_size, expected_sha256, label).is_ok() {
                if let Err(error) = std::fs::remove_file(&entry.quarantine) {
                    failures.push(format!(
                        "remove stale quarantine {}: {error}",
                        entry.quarantine.display()
                    ));
                }
            } else {
                failures.push(format!(
                    "refusing to replace cache entry {} created by another Hugging Face client",
                    entry.original.display()
                ));
            }
            continue;
        }
        if let Err(error) = std::fs::rename(&entry.quarantine, &entry.original) {
            failures.push(format!(
                "restore {} to {}: {error}",
                entry.quarantine.display(),
                entry.original.display()
            ));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

#[cfg(all(
    any(windows, test),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
fn remove_quarantined_entries(entries: &[QuarantinedEntry]) {
    let failures: Vec<String> = entries
        .iter()
        .filter_map(|entry| {
            std::fs::remove_file(&entry.quarantine)
                .err()
                .map(|error| format!("remove {}: {error}", entry.quarantine.display()))
        })
        .collect();
    if !failures.is_empty() {
        tracing::warn!(
            failures = %failures.join("; "),
            "refreshed Hugging Face artifact is valid, but stale quarantine entries could not be removed"
        );
    }
}

/// Resolve a pinned artifact strictly from the standard Hugging Face cache.
#[cfg(all(not(target_arch = "wasm32"), layout_detection))]
pub(crate) fn hf_cached_revision(
    repo_id: &str,
    remote_filename: &str,
    revision: &str,
) -> Result<Option<PathBuf>, String> {
    let api = hf_client_builder()
        .build_sync()
        .map_err(|e| format!("Failed to initialize HuggingFace Hub API: {e}"))?;
    hf_cached_revision_with_client(&api, repo_id, remote_filename, Some(revision))
}

#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
fn hf_cached_revision_with_client(
    api: &hf_hub::HFClientSync,
    repo_id: &str,
    remote_filename: &str,
    revision: Option<&str>,
) -> Result<Option<PathBuf>, String> {
    let (owner, name) = hf_hub::split_id(repo_id);
    let repository = api.model(owner, name);
    let request = repository
        .download_file()
        .filename(remote_filename)
        .local_files_only(true);
    let result = match revision {
        Some(revision) => request.revision(revision).send(),
        None => request.send(),
    };
    match result {
        Ok(path) => Ok(Some(path)),
        Err(hf_hub::HFError::LocalEntryNotFound { .. } | hf_hub::HFError::EntryNotFound { .. }) => Ok(None),
        Err(error) => Err(format!(
            "Failed to inspect Hugging Face cache for '{remote_filename}' from {repo_id}: {error}"
        )),
    }
}

/// Parse a `sha256sum`-format manifest into ordered `(path, sha256)` pairs.
///
/// Skips blank lines and `#` comments; each remaining line must be
/// `<64-hex-sha256>  <path>`. Leading `./` is stripped from paths and checksums are
/// lowercased. Returns the pairs in file order (may be empty if the content is all
/// comments — callers that require at least one entry check that themselves).
///
/// Shared by every checksum-manifest consumer (GLiNER model checksums, Candle VLM-OCR
/// weight staging) so the format and validation live in one place.
#[cfg(any(
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
pub(crate) fn parse_sha256_manifest(content: &str) -> Result<Vec<(String, String)>, String> {
    let mut entries = Vec::new();
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut parts = trimmed.split_whitespace();
        let checksum = parts
            .next()
            .ok_or_else(|| format!("Invalid checksum line {}: missing checksum", index + 1))?;
        let path = parts
            .next()
            .ok_or_else(|| format!("Invalid checksum line {}: missing path", index + 1))?;
        if checksum.len() != 64 || !checksum.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid checksum line {}: checksum must be SHA256 hex",
                index + 1
            ));
        }
        entries.push((path.trim_start_matches("./").to_string(), checksum.to_ascii_lowercase()));
    }
    Ok(entries)
}

/// Verify the SHA256 checksum of a file using streaming reads.
///
/// Streams the file in 64 KiB chunks to avoid loading large model files (100MB+) entirely
/// into memory. Returns `Ok(())` if the checksum matches or is empty (skip verification).
#[cfg(any(
    feature = "paddle-ocr",
    layout_detection,
    auto_rotate,
    feature = "ner-onnx",
    feature = "candle-paddleocr-vl",
    feature = "transcription",
    feature = "chunking-tokenizers",
    feature = "onnx-runtime",
    all(feature = "static-embeddings", not(target_arch = "wasm32"))
))]
pub(crate) fn verify_sha256(path: &Path, expected: &str, label: &str) -> Result<(), String> {
    if expected.is_empty() {
        return Ok(());
    }

    let file = std::fs::File::open(path).map_err(|e| format!("Failed to open file for checksum: {e}"))?;
    let mut reader = BufReader::with_capacity(64 * 1024, file);
    let mut hasher = Sha256::new();

    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| format!("Failed to read file for checksum: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }

    let hash_hex = hex::encode(hasher.finalize());

    if hash_hex != expected {
        return Err(format!(
            "Checksum mismatch for {label}: expected {expected}, got {hash_hex}"
        ));
    }

    tracing::debug!(label, "Checksum verified");
    Ok(())
}

#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    any(
        feature = "paddle-ocr",
        layout_detection,
        auto_rotate,
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "transcription",
        feature = "chunking-tokenizers",
        feature = "onnx-runtime",
        feature = "static-embeddings"
    )
))]
mod hf_cache_tests {
    use super::*;

    fn sha256(payload: &[u8]) -> String {
        use sha2::{Digest, Sha256};
        hex::encode(Sha256::digest(payload))
    }

    fn run_env_child(test_name: &str, cache: &Path, offline_variable: Option<&str>) {
        let mut command = std::process::Command::new(std::env::current_exe().unwrap());
        command
            .arg("--exact")
            .arg(test_name)
            .arg("--ignored")
            .arg("--nocapture")
            .env("XBERG_HF_CACHE_TEST_ROOT", cache)
            .env("HF_HUB_CACHE", cache)
            .env_remove("HF_HUB_OFFLINE")
            .env_remove("HUGGINGFACE_HUB_OFFLINE");
        if let Some(variable) = offline_variable {
            command.env(variable, "1");
        }
        let status = command.status().expect("launch isolated Hugging Face environment test");
        assert!(status.success(), "isolated test {test_name} failed with {status}");
    }

    #[test]
    fn pinned_revision_resolves_from_standard_cache_without_network() {
        let cache = tempfile::TempDir::new().unwrap();
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let cached_file = cache
            .path()
            .join("models--xberg-io--layout-models")
            .join("snapshots")
            .join(revision)
            .join("rtdetr/model.onnx");
        std::fs::create_dir_all(cached_file.parent().unwrap()).unwrap();
        std::fs::write(&cached_file, b"cached model").unwrap();

        let api = hf_hub::HFClientBuilder::new()
            .endpoint("http://127.0.0.1:1")
            .cache_dir(cache.path())
            .build_sync()
            .unwrap();
        let resolved =
            hf_cached_revision_with_client(&api, "xberg-io/layout-models", "rtdetr/model.onnx", Some(revision))
                .unwrap();

        assert_eq!(resolved.as_deref(), Some(cached_file.as_path()));
    }

    #[test]
    fn shared_resolver_returns_verified_snapshot_path_from_custom_hf_root() {
        let cache = tempfile::TempDir::new().unwrap();
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let payload = b"verified cached model";
        let cached_file = cache
            .path()
            .join("models--xberg-io--layout-models")
            .join("snapshots")
            .join(revision)
            .join("rtdetr/model.onnx");
        std::fs::create_dir_all(cached_file.parent().unwrap()).unwrap();
        std::fs::write(&cached_file, payload).unwrap();

        let resolved = hf_resolve_file(
            "xberg-io/layout-models",
            "rtdetr/model.onnx",
            Some(revision),
            Some(cache.path()),
            Some(&sha256(payload)),
        )
        .unwrap();

        assert_eq!(resolved, cached_file);
    }

    #[test]
    fn shared_resolver_uses_standard_hf_hub_cache_environment() {
        let cache = tempfile::TempDir::new().unwrap();
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let payload = b"standard environment cache";
        let cached_file = cache
            .path()
            .join("models--owner--repo")
            .join("snapshots")
            .join(revision)
            .join("model.onnx");
        std::fs::create_dir_all(cached_file.parent().unwrap()).unwrap();
        std::fs::write(&cached_file, payload).unwrap();

        run_env_child(
            "model_download::hf_cache_tests::standard_hf_hub_cache_environment_child",
            cache.path(),
            None,
        );
    }

    #[test]
    #[ignore = "run in an isolated subprocess by shared_resolver_uses_standard_hf_hub_cache_environment"]
    fn standard_hf_hub_cache_environment_child() {
        let cache = PathBuf::from(std::env::var_os("XBERG_HF_CACHE_TEST_ROOT").expect("test cache root"));
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let payload = b"standard environment cache";
        let cached_file = cache
            .join("models--owner--repo")
            .join("snapshots")
            .join(revision)
            .join("model.onnx");

        let resolved =
            hf_resolve_file("owner/repo", "model.onnx", Some(revision), None, Some(&sha256(payload))).unwrap();

        assert_eq!(resolved, cached_file);
        #[cfg(any(
            feature = "onnx-runtime",
            all(feature = "static-embeddings", not(target_arch = "wasm32"))
        ))]
        assert_eq!(hf_cache_key(None), cache.display().to_string());
    }

    #[test]
    fn offline_cache_miss_and_corruption_never_refresh_from_network() {
        let cache = tempfile::TempDir::new().unwrap();
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let cached_file = cache
            .path()
            .join("models--owner--repo")
            .join("snapshots")
            .join(revision)
            .join("corrupt.onnx");
        std::fs::create_dir_all(cached_file.parent().unwrap()).unwrap();
        std::fs::write(&cached_file, b"corrupt").unwrap();

        for variable in ["HF_HUB_OFFLINE", "HUGGINGFACE_HUB_OFFLINE"] {
            run_env_child(
                "model_download::hf_cache_tests::offline_cache_miss_and_corruption_child",
                cache.path(),
                Some(variable),
            );
        }
    }

    #[test]
    #[ignore = "run in isolated subprocesses by offline_cache_miss_and_corruption_never_refresh_from_network"]
    fn offline_cache_miss_and_corruption_child() {
        let cache = PathBuf::from(std::env::var_os("XBERG_HF_CACHE_TEST_ROOT").expect("test cache root"));
        let revision = "0123456789abcdef0123456789abcdef01234567";
        let cached_file = cache
            .join("models--owner--repo")
            .join("snapshots")
            .join(revision)
            .join("corrupt.onnx");

        let missing = hf_resolve_file("owner/repo", "missing.onnx", Some(revision), Some(&cache), None).unwrap_err();
        let corrupt = hf_resolve_file(
            "owner/repo",
            "corrupt.onnx",
            Some(revision),
            Some(&cache),
            Some(&sha256(b"expected")),
        )
        .unwrap_err();

        assert!(missing.contains("offline mode"), "{missing}");
        assert!(corrupt.contains("offline mode"), "{corrupt}");
        assert!(corrupt.contains("failed checksum verification"), "{corrupt}");
        assert_eq!(std::fs::read(cached_file).unwrap(), b"corrupt");
    }

    #[test]
    fn shared_resolver_rejects_invalid_checksum_before_cache_or_network_access() {
        let error =
            hf_resolve_file("owner/repo", "model.onnx", Some("revision"), None, Some("not-a-sha256")).unwrap_err();

        assert!(error.contains("Invalid SHA-256"));
    }

    #[test]
    fn offline_flag_parser_accepts_only_explicit_truthy_values() {
        for value in ["1", "true", "TRUE", " yes ", "On"] {
            assert!(env_flag_enabled(std::ffi::OsStr::new(value)), "{value}");
        }
        for value in ["", "0", "false", "off", "anything"] {
            assert!(!env_flag_enabled(std::ffi::OsStr::new(value)), "{value}");
        }
    }

    #[test]
    fn offline_cache_miss_identifies_pinned_artifact() {
        let error = offline_cache_miss("owner/repo", "model.onnx", Some("abc123"));
        assert!(error.contains("offline mode"));
        assert!(error.contains("owner/repo@abc123"));
        assert!(error.contains("model.onnx"));
    }

    #[cfg(layout_detection)]
    #[test]
    fn verified_cached_path_accepts_a_concurrently_repaired_artifact() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("model.onnx");
        let payload = b"repaired model";
        std::fs::write(&path, payload).unwrap();

        let resolved = verified_cached_path(Some(&path), payload.len() as u64, &sha256(payload), "test-model");

        assert_eq!(resolved.as_deref(), Some(path.as_path()));
    }

    #[test]
    fn quarantine_restore_round_trip_preserves_regular_cache_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("model.onnx");
        let payload = b"corrupt model";
        let expected_sha = sha256(payload);
        std::fs::write(&path, payload).unwrap();

        let quarantined =
            quarantine_hf_cache_entry(&path, Some(payload.len() as u64), &expected_sha, "test-model").unwrap();
        assert!(!path.exists());
        assert_eq!(quarantined.len(), 1);

        restore_quarantined_entries(&quarantined, Some(payload.len() as u64), &expected_sha, "test-model").unwrap();
        assert_eq!(std::fs::read(path).unwrap(), payload);
    }

    #[cfg(unix)]
    #[test]
    fn quarantine_restore_round_trip_preserves_snapshot_symlink_and_blob() {
        let dir = tempfile::TempDir::new().unwrap();
        let expected_sha = sha256(b"corrupt blob");
        let blob = dir.path().join("blobs").join(&expected_sha);
        let snapshot = dir.path().join("snapshots/revision/model.onnx");
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&blob, b"corrupt blob").unwrap();
        std::os::unix::fs::symlink(format!("../../blobs/{expected_sha}"), &snapshot).unwrap();

        let quarantined = quarantine_hf_cache_entry(
            &snapshot,
            Some(b"corrupt blob".len() as u64),
            &expected_sha,
            "test-model",
        )
        .unwrap();
        assert_eq!(quarantined.len(), 2);
        assert!(!snapshot.exists());
        assert!(!blob.exists());

        restore_quarantined_entries(
            &quarantined,
            Some(b"corrupt blob".len() as u64),
            &expected_sha,
            "test-model",
        )
        .unwrap();
        assert_eq!(std::fs::read(&snapshot).unwrap(), b"corrupt blob");
        assert_eq!(std::fs::read(blob).unwrap(), b"corrupt blob");
    }

    #[test]
    fn quarantine_restore_handles_windows_copy_snapshot_and_sha_named_blob() {
        let dir = tempfile::TempDir::new().unwrap();
        let payload = b"corrupt copied model";
        let expected_sha = sha256(payload);
        let blob = dir.path().join("blobs").join(&expected_sha);
        let snapshot = dir.path().join("snapshots/revision/model.onnx");
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&blob, payload).unwrap();
        std::fs::write(&snapshot, payload).unwrap();

        let quarantined =
            quarantine_hf_cache_entry(&snapshot, Some(payload.len() as u64), &expected_sha, "test-model").unwrap();

        assert_eq!(quarantined.len(), 2);
        assert!(!snapshot.exists());
        assert!(!blob.exists());
        restore_quarantined_entries(&quarantined, Some(payload.len() as u64), &expected_sha, "test-model").unwrap();
        assert_eq!(std::fs::read(snapshot).unwrap(), payload);
        assert_eq!(std::fs::read(blob).unwrap(), payload);
    }

    #[test]
    fn rollback_preserves_valid_peer_publication() {
        let dir = tempfile::TempDir::new().unwrap();
        let valid = b"new-data";
        let expected_sha = sha256(valid);
        let blob = dir.path().join("blobs").join(&expected_sha);
        let snapshot = dir.path().join("snapshots/revision/model.onnx");
        std::fs::create_dir_all(blob.parent().unwrap()).unwrap();
        std::fs::create_dir_all(snapshot.parent().unwrap()).unwrap();
        std::fs::write(&blob, b"bad-data").unwrap();
        std::fs::write(&snapshot, b"bad-data").unwrap();

        let quarantined =
            quarantine_hf_cache_entry(&snapshot, Some(valid.len() as u64), &expected_sha, "test-model").unwrap();
        std::fs::write(&blob, valid).unwrap();
        std::fs::write(&snapshot, valid).unwrap();

        restore_quarantined_entries(&quarantined, Some(valid.len() as u64), &expected_sha, "test-model").unwrap();

        assert_eq!(std::fs::read(&snapshot).unwrap(), valid);
        assert_eq!(std::fs::read(&blob).unwrap(), valid);
        assert!(
            quarantined.iter().all(|entry| !entry.quarantine.exists()),
            "obsolete corrupt quarantine files must be removed"
        );
    }

    #[test]
    fn rollback_refuses_to_remove_unknown_invalid_peer_entry() {
        let dir = tempfile::TempDir::new().unwrap();
        let valid = b"new-data";
        let path = dir.path().join("model.onnx");
        std::fs::write(&path, b"old-data").unwrap();
        let quarantined =
            quarantine_hf_cache_entry(&path, Some(valid.len() as u64), &sha256(valid), "test-model").unwrap();
        std::fs::write(&path, b"peer-bad").unwrap();

        let error = restore_quarantined_entries(&quarantined, Some(valid.len() as u64), &sha256(valid), "test-model")
            .unwrap_err();

        assert!(error.contains("refusing to replace cache entries"), "{error}");
        assert_eq!(std::fs::read(&path).unwrap(), b"peer-bad");
        assert!(quarantined[0].quarantine.exists());
    }

    #[test]
    fn successful_quarantine_cleanup_removes_backup() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("model.onnx");
        std::fs::write(&path, b"corrupt model").unwrap();
        let quarantined = quarantine_hf_cache_entry(
            &path,
            Some(b"corrupt model".len() as u64),
            &sha256(b"corrupt model"),
            "test-model",
        )
        .unwrap();
        let backup = quarantined[0].quarantine.clone();

        remove_quarantined_entries(&quarantined);

        assert!(!backup.exists());
    }

    #[test]
    fn artifact_file_lock_is_bounded_and_released() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("artifact.lock");
        let first = acquire_artifact_file_lock_with_timeout(&path, Duration::from_secs(1)).unwrap();

        let error = acquire_artifact_file_lock_with_timeout(&path, Duration::from_millis(100)).unwrap_err();
        assert!(error.contains("Timed out"), "{error}");

        drop(first);
        acquire_artifact_file_lock_with_timeout(&path, Duration::from_secs(1)).unwrap();
    }
}

/// Central registry of every vendored, checked-in SHA-256 manifest across model
/// families, paired with a short family name.
///
/// Each family's manifest const is only reachable when its module is compiled, so
/// every entry is pushed under its own `#[cfg]` matching that module's feature gate.
/// Returns an empty `Vec` when no relevant feature is enabled — callers must not
/// assume non-empty. Used by the coverage test below and available for future
/// tooling (e.g. `xberg cache manifest`) that wants a single source of truth for
/// "which families are checksum-pinned right now".
#[cfg_attr(not(test), allow(dead_code))]
#[allow(unused_mut)]
// Every push below is individually `#[cfg]`-gated on its family's feature, so the
// entries cannot be expressed as a single `vec![]` literal. ~keep
#[allow(clippy::vec_init_then_push)]
pub(crate) fn vendored_model_manifests() -> Vec<(&'static str, &'static str)> {
    let mut manifests = Vec::new();

    // The `embeddings` module (and therefore its manifest const) is only compiled
    // under `embedding-presets`; the const itself further requires `embeddings`,
    // non-wasm `static-embeddings`, or `test`. ~keep
    #[cfg(all(
        feature = "embedding-presets",
        any(
            feature = "embeddings",
            all(feature = "static-embeddings", not(target_arch = "wasm32")),
            test
        )
    ))]
    manifests.push(("embeddings", crate::embeddings::EMBEDDING_SHA256_MANIFEST));

    #[cfg(all(
        any(feature = "sparse-embedding-presets", feature = "sparse-embeddings"),
        any(feature = "sparse-embeddings", test)
    ))]
    manifests.push((
        "sparse_embeddings",
        crate::sparse_embeddings::SPARSE_EMBEDDING_SHA256_MANIFEST,
    ));

    #[cfg(all(
        any(feature = "late-interaction-presets", feature = "late-interaction"),
        any(feature = "late-interaction", test)
    ))]
    manifests.push((
        "late_interaction",
        crate::late_interaction::LATE_INTERACTION_SHA256_MANIFEST,
    ));

    #[cfg(all(
        any(feature = "reranker-presets", feature = "reranker"),
        any(feature = "reranker", test)
    ))]
    manifests.push(("reranking", crate::reranking::RERANKER_SHA256_MANIFEST));

    #[cfg(feature = "ner-onnx")]
    manifests.push(("gliner", crate::text::ner::gline::GLINER_SHA256_MANIFEST));

    #[cfg(feature = "candle-paddleocr-vl")]
    manifests.push((
        "paddleocr-vl",
        crate::candle_ocr::model_stager::PADDLEOCR_VL_16_SHA256_MANIFEST,
    ));

    manifests
}

/// Tests for the always-compiled download watchdog. Deliberately network-free: they exercise the
/// deadline machinery with plain closures so the guard's behavior is provable in CI without any
/// HuggingFace connectivity.
#[cfg(test)]
mod download_deadline_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn with_download_deadline_returns_ok_for_fast_closure() {
        let result = with_download_deadline("fast", || Ok::<i32, String>(42));
        assert_eq!(result, Ok(42), "fast closure must return its Ok value verbatim");
    }

    #[test]
    fn deadline_reads_env_override_and_aborts_a_hung_closure() {
        #[allow(unsafe_code)]
        unsafe {
            std::env::set_var("XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS", "1");
        }
        assert_eq!(
            model_download_timeout(),
            Duration::from_secs(1),
            "explicit override must win"
        );

        let started = Instant::now();
        let result = with_download_deadline("hung", || {
            std::thread::sleep(Duration::from_secs(10));
            Ok::<(), String>(())
        });
        let elapsed = started.elapsed();
        #[allow(unsafe_code)]
        unsafe {
            std::env::remove_var("XBERG_MODEL_DOWNLOAD_TIMEOUT_SECS");
        }

        let err = result.expect_err("a closure that outlives the deadline must return Err");
        assert!(err.contains("timed out"), "error must mention the timeout, got: {err}");
        assert!(
            elapsed < Duration::from_secs(3),
            "guard must fire near the 1s deadline, not wait out the 10s sleep (took {elapsed:?})"
        );
    }
}

/// Tests for the connect-timeout-hardened HF client builder (#1249). Network-free: `build_sync`
/// only constructs the reqwest client + tokio handle, so a successful build proves the injected
/// `connect_timeout` client path compiles and constructs on this platform.
#[cfg(all(
    test,
    not(target_arch = "wasm32"),
    any(
        feature = "candle-paddleocr-vl",
        feature = "paddle-ocr",
        auto_rotate,
        layout_detection,
        feature = "transcription",
        feature = "onnx-runtime",
        feature = "ner-onnx",
        feature = "static-embeddings"
    )
))]
mod hf_client_builder_tests {
    use super::*;

    #[test]
    fn hf_client_builder_builds_a_working_client() {
        let client = hf_client_builder().build_sync();
        assert!(
            client.is_ok(),
            "builder with injected connect-timeout client must construct offline: {:?}",
            client.err()
        );
    }

    #[test]
    fn order_ipv4_first_puts_ipv4_before_ipv6_and_is_stable() {
        use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, SocketAddrV4, SocketAddrV6};
        let v6a = SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 0, 0, 0));
        let v4a = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(1, 1, 1, 1), 0));
        let v6b = SocketAddr::V6(SocketAddrV6::new(
            Ipv6Addr::new(0x2606, 0x4700, 0, 0, 0, 0, 0, 1),
            0,
            0,
            0,
        ));
        let v4b = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::new(2, 2, 2, 2), 0));

        // Interleaved AAAA-first input: default getaddrinfo order can hand IPv6 out first. ~keep
        let mut addrs = vec![v6a, v4a, v6b, v4b];
        order_ipv4_first(&mut addrs);

        assert_eq!(
            addrs,
            vec![v4a, v4b, v6a, v6b],
            "IPv4 addresses must precede IPv6, preserving intra-family order"
        );
        assert!(
            !addrs[0].is_ipv6(),
            "the first address offered to the connector must be IPv4"
        );
    }
}

#[cfg(all(
    test,
    any(feature = "paddle-ocr", layout_detection, auto_rotate, feature = "ner-onnx")
))]
mod tests {
    use super::*;

    #[cfg(any(feature = "ner-onnx", feature = "candle-paddleocr-vl"))]
    #[test]
    fn parse_sha256_manifest_reads_entries_and_normalizes() {
        let entries = parse_sha256_manifest(
            "# comment\n\
             AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA  ./config.json\n\
             bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  tokenizer.json\n",
        )
        .expect("valid manifest");
        assert_eq!(entries[0], ("config.json".to_string(), "a".repeat(64)));
        assert_eq!(entries[1].0, "tokenizer.json");
        assert!(parse_sha256_manifest("# only comments\n").unwrap().is_empty());
    }

    #[cfg(any(feature = "ner-onnx", feature = "candle-paddleocr-vl"))]
    #[test]
    fn parse_sha256_manifest_rejects_malformed_lines() {
        assert!(
            parse_sha256_manifest("not-a-sha256  config.json").is_err(),
            "invalid hash"
        );
        assert!(parse_sha256_manifest(&"a".repeat(64)).is_err(), "missing path");
    }

    #[test]
    fn download_lock_is_stable_per_key_and_distinct_across_keys() {
        let a1 = download_lock("xberg-io/layout-models/rtdetr/model.onnx");
        let a2 = download_lock("xberg-io/layout-models/rtdetr/model.onnx");
        let b = download_lock("xberg-io/layout-models/tatr/model.onnx");

        assert!(std::sync::Arc::ptr_eq(&a1, &a2), "same key must share one lock");
        assert!(!std::sync::Arc::ptr_eq(&a1, &b), "distinct keys must not share a lock");
    }

    #[test]
    fn download_lock_serializes_same_key_across_threads() {
        use std::sync::Arc;
        use std::sync::atomic::{AtomicUsize, Ordering};

        let in_flight = Arc::new(AtomicUsize::new(0));
        let max_seen = Arc::new(AtomicUsize::new(0));

        let handles: Vec<_> = (0..8)
            .map(|_| {
                let (in_flight, max_seen) = (in_flight.clone(), max_seen.clone());
                std::thread::spawn(move || {
                    let lock = download_lock("same/key/file.bin");
                    let _g = lock.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                    let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                    max_seen.fetch_max(now, Ordering::SeqCst);
                    std::thread::yield_now();
                    in_flight.fetch_sub(1, Ordering::SeqCst);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(
            max_seen.load(Ordering::SeqCst),
            1,
            "same-key critical sections must not overlap"
        );
    }
}

/// Central coverage test for [`vendored_model_manifests`], covering every model
/// family from one place instead of duplicating the well-formedness checks per
/// family. `parse_sha256_manifest` is only reachable under the feature union below
/// (matching every family that can populate the registry), so the assertion body is
/// gated the same way. Under a build with none of those features the registry itself
/// still compiles and returns an empty `Vec` (see the always-on smoke test below).
#[cfg(all(
    test,
    any(
        feature = "ner-onnx",
        feature = "candle-paddleocr-vl",
        feature = "onnx-runtime",
        all(feature = "static-embeddings", not(target_arch = "wasm32"))
    )
))]
mod vendored_manifest_tests {
    use super::*;

    /// Fail-closed guarantee across every model family: whichever vendored checksum
    /// manifests are reachable under the current feature set must each parse cleanly,
    /// declare at least one entry, have no duplicate paths, and use well-formed
    /// SHA-256 hex.
    #[test]
    fn every_vendored_manifest_is_well_formed_and_deduplicated() {
        let manifests = vendored_model_manifests();

        for (family, content) in &manifests {
            let entries = parse_sha256_manifest(content)
                .unwrap_or_else(|error| panic!("[{family}] manifest failed to parse: {error}"));
            assert!(!entries.is_empty(), "[{family}] manifest declares no entries");

            let mut seen_paths = std::collections::HashSet::new();
            for (path, checksum) in &entries {
                assert!(
                    seen_paths.insert(path.as_str()),
                    "[{family}] duplicate path in manifest: {path}"
                );
                assert_eq!(
                    checksum.len(),
                    64,
                    "[{family}] checksum for {path} is not 64 hex chars: {checksum}"
                );
                assert!(
                    checksum
                        .bytes()
                        .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
                    "[{family}] checksum for {path} is not lowercase hex: {checksum}"
                );
            }
        }

        eprintln!(
            "vendored_model_manifests: checked {} famil{} ({})",
            manifests.len(),
            if manifests.len() == 1 { "y" } else { "ies" },
            manifests.iter().map(|(name, _)| *name).collect::<Vec<_>>().join(", ")
        );
    }
}

/// Always-on smoke test: [`vendored_model_manifests`] itself must compile and run
/// under any feature combination, including none, and must not panic when the
/// registry comes back empty.
#[cfg(test)]
mod vendored_manifest_registry_tests {
    use super::*;

    #[test]
    fn registry_never_panics_even_when_empty() {
        let manifests = vendored_model_manifests();
        assert!(manifests.len() <= 6, "registry declares more families than expected");
    }
}
