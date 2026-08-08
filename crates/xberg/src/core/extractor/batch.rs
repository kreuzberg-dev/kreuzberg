//! Batch extraction operations for concurrent processing.
//!
//! This module provides parallel extraction capabilities for processing
//! multiple files or byte arrays concurrently with automatic resource management.
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use crate::core::config::BatchBytesItem;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use crate::core::config::BatchFileItem;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use crate::core::config::ExtractionConfig;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use crate::core::config::extraction::FileExtractionConfig;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use crate::types::ExtractedDocument;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use crate::{Result, XbergError};
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use std::future::Future;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use std::sync::Arc;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use std::time::Instant;

#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use super::bytes::extract_bytes;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use super::file::extract_file;
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
use super::helpers::error_extraction_result;

/// Shared batch result collection: spawns tasks via callback, collects ordered results.
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
async fn collect_batch<F, Fut>(
    count: usize,
    config: &ExtractionConfig,
    layout_active: bool,
    spawn_task: F,
) -> Result<Vec<ExtractedDocument>>
where
    F: Fn(usize, Arc<tokio::sync::Semaphore>, usize) -> Fut,
    Fut: Future<Output = (usize, Result<ExtractedDocument>, u64)> + Send + 'static,
{
    use tokio::sync::Semaphore;
    use tokio::task::JoinSet;

    if count == 0 {
        return Ok(vec![]);
    }

    crate::core::config::concurrency::init_batch_thread_pool(config.concurrency.as_ref());
    let execution_plan = crate::core::config::concurrency::resolve_batch_execution_plan(
        config.concurrency.as_ref(),
        crate::core::config::concurrency::LayoutBatchWorkload::from_layout_active(layout_active),
        count,
        config.max_concurrent_extractions,
    );
    let semaphore = Arc::new(Semaphore::new(execution_plan.workers));

    let mut tasks = JoinSet::new();

    for index in 0..count {
        let sem = Arc::clone(&semaphore);
        tasks.spawn(spawn_task(index, sem, execution_plan.thread_budget));
    }

    let mut results: Vec<Option<ExtractedDocument>> = vec![None; count];

    while let Some(task_result) = tasks.join_next().await {
        match task_result {
            Ok((index, Ok(result), _elapsed_ms)) => {
                results[index] = Some(result);
            }
            Ok((index, Err(e), elapsed_ms)) => {
                results[index] = Some(error_extraction_result(&e, Some(elapsed_ms)));
            }
            Err(join_err) => {
                return Err(XbergError::Other(format!("Task panicked: {}", join_err)));
            }
        }
    }

    Ok(results.into_iter().flatten().collect())
}

/// Run a single extraction task with semaphore gating, timing, optional timeout, and batch mode.
///
/// When `cancel_token` is provided and the timeout fires, the token is signalled so that
/// any blocking PDF operations in progress can observe the cancellation at the next
/// inter-page checkpoint and stop early.
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
async fn run_timed_extraction<F, Fut>(
    index: usize,
    semaphore: Arc<tokio::sync::Semaphore>,
    timeout_secs: Option<u64>,
    cancel_token: Option<crate::cancellation::CancellationToken>,
    extract_fn: F,
) -> (usize, Result<ExtractedDocument>, u64)
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<ExtractedDocument>>,
{
    let _permit = semaphore.acquire().await.unwrap();
    let start = Instant::now();

    let extraction_future = crate::core::batch_mode::with_batch_mode(extract_fn());

    let mut result = match timeout_secs {
        Some(secs) => match tokio::time::timeout(std::time::Duration::from_secs(secs), extraction_future).await {
            Ok(inner) => inner,
            Err(_elapsed) => {
                if let Some(ref token) = cancel_token {
                    token.cancel();
                }
                let elapsed_ms = start.elapsed().as_millis() as u64;
                Err(XbergError::Timeout {
                    elapsed_ms,
                    limit_ms: secs * 1000,
                })
            }
        },
        None => extraction_future.await,
    };

    let elapsed_ms = start.elapsed().as_millis() as u64;

    if let Ok(ref mut r) = result {
        r.metadata.extraction_duration_ms = Some(elapsed_ms);
    }

    (index, result, elapsed_ms)
}

/// Resolve a per-file config against a base config. Returns owned config.
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
fn resolve_config(base: &ExtractionConfig, file_config: &Option<FileExtractionConfig>) -> ExtractionConfig {
    match file_config {
        Some(fc) => base.with_file_overrides(fc),
        None => base.clone(),
    }
}

#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
fn apply_batch_thread_budget(config: &mut ExtractionConfig, thread_budget: usize) {
    if crate::core::config::concurrency::resolve_thread_budget(config.concurrency.as_ref()) != thread_budget {
        config.concurrency = Some(crate::core::config::ConcurrencyConfig {
            max_threads: Some(thread_budget),
        });
    }
}

/// Extract content from multiple files concurrently.
///
/// This function processes multiple files in parallel, automatically managing
/// concurrency to prevent resource exhaustion. The concurrency limit can be
/// configured via `ExtractionConfig::max_concurrent_extractions` or defaults
/// to `(num_cpus * 1.5).ceil()`.
///
/// Each file can optionally specify a [`FileExtractionConfig`] that overrides specific
/// fields from the batch-level `config`. Pass `None` for a file to use the batch defaults.
/// Batch-level settings like `max_concurrent_extractions` and `use_cache` are always
/// taken from the batch-level `config`.
///
/// # Arguments
///
/// * `items` - Vector of `BatchFileItem` structs, each containing a path and optional
///   per-file configuration overrides.
/// * `config` - Batch-level extraction configuration (provides defaults and batch settings)
///
/// # Returns
///
/// A vector of `ExtractedDocument` in the same order as the input items.
///
/// # Errors
///
/// Individual file errors are captured in the result metadata. System errors
/// (IO, RuntimeError equivalents) will bubble up and fail the entire batch.
///
/// # Examples
///
/// This function and [`BatchFileItem`] are crate-internal; the public entry point that
/// reaches them is [`crate::extract_batch`] with URI inputs.
///
/// Simple usage with no per-file overrides:
///
/// ```rust,no_run
/// use xberg::{ExtractInput, ExtractionConfig, extract_batch};
///
/// # async fn example() -> xberg::Result<()> {
/// let config = ExtractionConfig::default();
/// let inputs = vec![
///     ExtractInput::from_uri("doc1.pdf"),
///     ExtractInput::from_uri("doc2.pdf"),
/// ];
/// let output = extract_batch(inputs, &config).await?;
/// println!("Processed {} files", output.results.len());
/// # Ok(())
/// # }
/// ```
///
/// Per-file configuration overrides:
///
/// ```rust,no_run
/// use xberg::{ExtractInput, ExtractionConfig, FileExtractionConfig, extract_batch};
///
/// # async fn example() -> xberg::Result<()> {
/// let config = ExtractionConfig::default();
/// let mut scan = ExtractInput::from_uri("scan.pdf");
/// scan.config = Some(FileExtractionConfig { force_ocr: Some(true), ..Default::default() });
/// let inputs = vec![scan, ExtractInput::from_uri("notes.txt")];
/// let output = extract_batch(inputs, &config).await?;
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
#[cfg_attr(feature = "otel", tracing::instrument(
    skip(config, items),
    fields(
        { crate::telemetry::conventions::OPERATION } = crate::telemetry::conventions::operations::BATCH_EXTRACT,
        { crate::telemetry::conventions::BATCH_SIZE } = items.len(),
    )
))]
pub(crate) async fn batch_extract_files(
    items: Vec<BatchFileItem>,
    config: &ExtractionConfig,
) -> Result<Vec<ExtractedDocument>> {
    let config_arc = Arc::new(config.clone());
    let items_arc = Arc::new(items);
    let count = items_arc.len();
    #[cfg(layout_detection)]
    let layout_active = config.layout.is_some()
        || items_arc.iter().any(|item| {
            item.config
                .as_ref()
                .is_some_and(|item_config| item_config.layout.is_some())
        });
    #[cfg(not(layout_detection))]
    let layout_active = false;

    collect_batch(count, config, layout_active, |index, sem, thread_budget| {
        let cfg = Arc::clone(&config_arc);
        let items = Arc::clone(&items_arc);
        async move {
            let item = &items[index];
            let mut resolved = resolve_config(&cfg, &item.config);
            apply_batch_thread_budget(&mut resolved, thread_budget);
            let timeout = resolved.extraction_timeout_secs;
            let cancel_token = resolved.cancel_token.clone();
            run_timed_extraction(index, sem, timeout, cancel_token, || {
                let path = item.path.clone();
                async move { extract_file(&path, None, &resolved).await }
            })
            .await
        }
    })
    .await
}

/// Extract content from multiple byte arrays concurrently.
///
/// This function processes multiple byte arrays in parallel, automatically managing
/// concurrency to prevent resource exhaustion. The concurrency limit can be
/// configured via `ExtractionConfig::max_concurrent_extractions` or defaults
/// to `(num_cpus * 1.5).ceil()`.
///
/// Each item can optionally specify a [`FileExtractionConfig`] that overrides specific
/// fields from the batch-level `config`. Pass `None` as the config to use
/// the batch-level defaults for that item.
///
/// # Arguments
///
/// * `items` - Vector of `BatchBytesItem` structs, each containing content bytes,
///   MIME type, and optional per-item configuration overrides.
/// * `config` - Batch-level extraction configuration
///
/// # Returns
///
/// A vector of `ExtractedDocument` in the same order as the input items.
///
/// # Examples
///
/// This function and [`BatchBytesItem`] are crate-internal; the public entry point that
/// reaches them is [`crate::extract_batch`] with bytes inputs.
///
/// Simple usage with no per-item overrides:
///
/// ```rust,no_run
/// use xberg::{ExtractInput, ExtractionConfig, extract_batch};
///
/// # async fn example() -> xberg::Result<()> {
/// let config = ExtractionConfig::default();
/// let inputs = vec![
///     ExtractInput::from_bytes(b"content 1".to_vec(), "text/plain", None),
///     ExtractInput::from_bytes(b"content 2".to_vec(), "text/plain", None),
/// ];
/// let output = extract_batch(inputs, &config).await?;
/// println!("Processed {} items", output.results.len());
/// # Ok(())
/// # }
/// ```
///
/// Per-item configuration overrides:
///
/// ```rust,no_run
/// use xberg::{ExtractInput, ExtractionConfig, FileExtractionConfig, extract_batch};
///
/// # async fn example() -> xberg::Result<()> {
/// let config = ExtractionConfig::default();
/// let mut html = ExtractInput::from_bytes(b"<html>test</html>".to_vec(), "text/html", None);
/// html.config = Some(FileExtractionConfig { force_ocr: Some(true), ..Default::default() });
/// let inputs = vec![
///     ExtractInput::from_bytes(b"content".to_vec(), "text/plain", None),
///     html,
/// ];
/// let output = extract_batch(inputs, &config).await?;
/// # Ok(())
/// # }
/// ```
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
#[cfg_attr(feature = "otel", tracing::instrument(
    skip(config, items),
    fields(
        { crate::telemetry::conventions::OPERATION } = crate::telemetry::conventions::operations::BATCH_EXTRACT,
        { crate::telemetry::conventions::BATCH_SIZE } = items.len(),
    )
))]
pub(crate) async fn batch_extract_bytes(
    items: Vec<BatchBytesItem>,
    config: &ExtractionConfig,
) -> Result<Vec<ExtractedDocument>> {
    let config_arc = Arc::new(config.clone());
    let count = items.len();
    #[cfg(layout_detection)]
    let layout_active = config.layout.is_some()
        || items.iter().any(|item| {
            item.config
                .as_ref()
                .is_some_and(|item_config| item_config.layout.is_some())
        });
    #[cfg(not(layout_detection))]
    let layout_active = false;

    type BytesSlot = parking_lot::Mutex<Option<BatchBytesItem>>;
    let slots: Arc<Vec<BytesSlot>> = Arc::new(
        items
            .into_iter()
            .map(|item| parking_lot::Mutex::new(Some(item)))
            .collect(),
    );

    collect_batch(count, config, layout_active, |index, sem, thread_budget| {
        let cfg = Arc::clone(&config_arc);
        let slots = Arc::clone(&slots);
        async move {
            let item = slots[index].lock().take().expect("batch item already consumed");
            let mut resolved = resolve_config(&cfg, &item.config);
            apply_batch_thread_budget(&mut resolved, thread_budget);
            let timeout = resolved.extraction_timeout_secs;
            let cancel_token = resolved.cancel_token.clone();
            run_timed_extraction(index, sem, timeout, cancel_token, || async move {
                extract_bytes(&item.content, &item.mime_type, &resolved).await
            })
            .await
        }
    })
    .await
}
