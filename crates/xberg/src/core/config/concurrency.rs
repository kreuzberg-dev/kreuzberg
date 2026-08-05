//! Concurrency and thread pool configuration.

use std::sync::Once;
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};

/// Controls thread usage for constrained environments.
///
/// Set `max_threads` to cap all internal thread pools (Rayon, ONNX Runtime
/// intra-op) and batch concurrency to a single limit.
///
/// # Example
///
/// ```rust
/// use xberg::core::config::ConcurrencyConfig;
///
/// let config = ConcurrencyConfig {
///     max_threads: Some(2),
/// };
/// ```
#[cfg_attr(alef, alef(skip))]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ConcurrencyConfig {
    /// Maximum number of threads for all internal thread pools.
    ///
    /// Caps Rayon global pool size, ONNX Runtime intra-op threads, and the
    /// combined document/inner-task budget for batch extraction. When `None`,
    /// system defaults are used.
    pub max_threads: Option<usize>,
}

static POOL_INIT: Once = Once::new();
static ACTIVE_THREAD_BUDGET: AtomicUsize = AtomicUsize::new(0);

/// Resolve the effective thread budget from config or auto-detection.
///
/// User-set `max_threads` takes priority. Otherwise auto-detects from `num_cpus`,
/// capped at 8 for sane defaults in serverless environments.
///
/// # Example
///
/// ```ignore
/// use xberg::core::config::ConcurrencyConfig;
/// use xberg::core::config::concurrency::resolve_thread_budget;
///
/// let config = ConcurrencyConfig { max_threads: Some(4) };
/// assert_eq!(resolve_thread_budget(Some(&config)), 4);
/// assert!(resolve_thread_budget(None) >= 1);
/// ```
pub(crate) fn resolve_thread_budget(config: Option<&ConcurrencyConfig>) -> usize {
    if let Some(n) = config.and_then(|c| c.max_threads) {
        return n.max(1);
    }
    num_cpus::get().min(8)
}

/// Internal worker/session allocation for one batch extraction.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        test,
        feature = "tokio-runtime",
        feature = "late-interaction",
        feature = "reranker",
        feature = "sparse-embeddings"
    )
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct BatchExecutionPlan {
    pub workers: usize,
    pub thread_budget: usize,
}

/// How strongly a batch is known to exercise native layout inference.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        test,
        feature = "tokio-runtime",
        feature = "late-interaction",
        feature = "reranker",
        feature = "sparse-embeddings"
    )
))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LayoutBatchWorkload {
    /// No input has layout inference configured.
    None,
    /// Layout may run for only part of the batch or through a non-PDF path.
    #[cfg(layout_detection)]
    Mixed,
    /// Every input is a PDF using layout inference for Markdown extraction.
    #[cfg(layout_detection)]
    All,
}

#[cfg(all(test, feature = "tokio-runtime", not(target_arch = "wasm32")))]
impl LayoutBatchWorkload {
    pub(crate) fn from_layout_active(layout_active: bool) -> Self {
        #[cfg(layout_detection)]
        {
            if layout_active { Self::Mixed } else { Self::None }
        }
        #[cfg(not(layout_detection))]
        {
            let _ = layout_active;
            Self::None
        }
    }
}

/// Allocate batch workers and per-worker model threads without oversubscription.
///
/// The total configured budget is divided between document workers so nested
/// per-document parallelism cannot multiply the process-wide CPU budget.
/// All-layout PDF batches use one document worker with the full thread budget.
/// RT-DETR inference does not scale enough across two half-budget sessions to
/// justify their additional resident memory. Mixed or uncertain layout batches
/// retain the previous two-worker cap, while non-layout batches use the normal
/// worker ceiling. `max_concurrent` is always a ceiling and cannot expand
/// execution beyond the total thread budget.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(
        test,
        feature = "tokio-runtime",
        feature = "late-interaction",
        feature = "reranker",
        feature = "sparse-embeddings"
    )
))]
pub(crate) fn resolve_batch_execution_plan(
    config: Option<&ConcurrencyConfig>,
    layout_workload: LayoutBatchWorkload,
    input_count: usize,
    max_concurrent: Option<usize>,
) -> BatchExecutionPlan {
    #[cfg(layout_detection)]
    const MAX_NATIVE_LAYOUT_BATCH_WORKERS: usize = 1;
    #[cfg(layout_detection)]
    const MAX_MIXED_LAYOUT_BATCH_WORKERS: usize = 2;

    let total_budget = resolve_thread_budget(config);
    let available_inputs = input_count.max(1);
    let worker_ceiling = max_concurrent
        .unwrap_or(total_budget)
        .max(1)
        .min(total_budget)
        .min(available_inputs);
    let workers = match layout_workload {
        LayoutBatchWorkload::None => worker_ceiling,
        #[cfg(layout_detection)]
        LayoutBatchWorkload::Mixed => worker_ceiling.min(MAX_MIXED_LAYOUT_BATCH_WORKERS),
        #[cfg(layout_detection)]
        LayoutBatchWorkload::All => worker_ceiling.min(MAX_NATIVE_LAYOUT_BATCH_WORKERS),
    }
    .max(1);
    let thread_budget = (total_budget / workers).max(1);

    debug_assert!(workers * thread_budget <= total_budget);
    BatchExecutionPlan { workers, thread_budget }
}

/// Resolve concurrency for model-level batches outside document extraction.
#[cfg(all(
    not(target_arch = "wasm32"),
    any(feature = "late-interaction", feature = "reranker", feature = "sparse-embeddings")
))]
pub(crate) fn resolve_batch_concurrency(config: Option<&ConcurrencyConfig>, model_threads_active: bool) -> usize {
    let budget = resolve_thread_budget(config);
    if !model_threads_active {
        return budget;
    }
    let cores = num_cpus::get().max(1);
    (cores / budget).max(1).min(budget)
}

/// Initialize the global Rayon thread pool with the given budget.
///
/// Safe to call multiple times — only the first call takes effect (subsequent
/// calls are silently ignored).
///
/// # Example
///
/// ```ignore
/// use xberg::core::config::concurrency::init_thread_pools;
///
/// init_thread_pools(4);
/// init_thread_pools(2); // no-op: pool already initialized
/// ```
pub(crate) fn init_thread_pools(budget: usize) {
    POOL_INIT.call_once(|| {
        ACTIVE_THREAD_BUDGET.store(budget.max(1), Ordering::Relaxed);
        #[cfg(not(target_arch = "wasm32"))]
        if let Err(_err) = rayon::ThreadPoolBuilder::new().num_threads(budget).build_global() {
            tracing::debug!(
                budget,
                "global rayon pool already initialized; reusing the existing pool \
                 (xberg thread budget not applied)"
            );
        }
        #[cfg(target_arch = "wasm32")]
        let _ = budget;
    });
}

/// Return the process thread budget selected when the shared pools were initialized.
///
/// Model backends with private pools use this value to obey the same extraction
/// budget. Before initialization, this resolves to the standard automatic limit.
#[cfg(sceptre_ocr)]
pub(crate) fn active_thread_budget() -> usize {
    match ACTIVE_THREAD_BUDGET.load(Ordering::Relaxed) {
        0 => resolve_thread_budget(None),
        budget => budget,
    }
}

/// Initialize process-wide CPU pools from the total batch budget.
///
/// Batch workers receive a divided per-document budget, but Rayon is global and
/// immutable after first initialization. It must therefore be initialized before
/// any worker observes its smaller share.
#[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
pub(crate) fn init_batch_thread_pool(config: Option<&ConcurrencyConfig>) -> usize {
    let total_budget = resolve_thread_budget(config);
    init_thread_pools(total_budget);
    total_budget
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_thread_budget_none() {
        let budget = resolve_thread_budget(None);
        assert!(budget >= 1);
        assert!(budget <= 8);
    }

    #[test]
    fn test_resolve_thread_budget_with_config() {
        let config = ConcurrencyConfig { max_threads: Some(4) };
        assert_eq!(resolve_thread_budget(Some(&config)), 4);
    }

    #[test]
    fn test_resolve_thread_budget_clamps_to_one() {
        let config = ConcurrencyConfig { max_threads: Some(0) };
        assert_eq!(resolve_thread_budget(Some(&config)), 1);
    }

    #[test]
    fn test_resolve_thread_budget_no_max() {
        let config = ConcurrencyConfig { max_threads: None };
        let budget = resolve_thread_budget(Some(&config));
        assert!(budget >= 1);
        assert!(budget <= 8);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_batch_plan_without_layout_uses_available_budget() {
        let budget = resolve_thread_budget(None);
        assert_eq!(
            resolve_batch_execution_plan(None, LayoutBatchWorkload::None, budget, None),
            BatchExecutionPlan {
                workers: budget,
                thread_budget: 1,
            }
        );
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), layout_detection))]
    fn test_layout_batch_plan_table() {
        for budget in [1, 2, 4, 8] {
            let config = ConcurrencyConfig {
                max_threads: Some(budget),
            };
            assert_eq!(
                resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::All, 16, None),
                BatchExecutionPlan {
                    workers: 1,
                    thread_budget: budget,
                }
            );
        }
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), layout_detection))]
    fn test_mixed_layout_batch_preserves_two_worker_cap() {
        for (budget, workers, thread_budget) in [(1, 1, 1), (2, 2, 1), (4, 2, 2), (8, 2, 4)] {
            let config = ConcurrencyConfig {
                max_threads: Some(budget),
            };
            assert_eq!(
                resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::Mixed, 16, None),
                BatchExecutionPlan { workers, thread_budget }
            );
        }
    }

    #[test]
    #[cfg(all(not(target_arch = "wasm32"), layout_detection))]
    fn test_layout_batch_plan_respects_input_and_explicit_limits() {
        let config = ConcurrencyConfig { max_threads: Some(8) };
        assert_eq!(
            resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::All, 1, Some(8)),
            BatchExecutionPlan {
                workers: 1,
                thread_budget: 8,
            }
        );
        assert_eq!(
            resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::All, 8, Some(1)),
            BatchExecutionPlan {
                workers: 1,
                thread_budget: 8,
            }
        );
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_non_layout_batch_plan_divides_budget_at_explicit_worker_limit() {
        let config = ConcurrencyConfig { max_threads: Some(8) };
        let plan = resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::None, 16, Some(2));
        assert_eq!(plan.workers, 2);
        assert_eq!(plan.thread_budget, 4);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_non_layout_batch_plan_clamps_explicit_limit_to_total_budget() {
        let config = ConcurrencyConfig { max_threads: Some(2) };
        let plan = resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::None, 8, Some(6));
        assert_eq!(plan.workers, 2);
        assert_eq!(plan.thread_budget, 1);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_non_layout_batch_plan_gives_single_input_full_inner_budget() {
        let config = ConcurrencyConfig { max_threads: Some(8) };
        let plan = resolve_batch_execution_plan(Some(&config), LayoutBatchWorkload::None, 1, None);
        assert_eq!(plan.workers, 1);
        assert_eq!(plan.thread_budget, 8);
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn test_batch_plan_never_exceeds_total_budget() {
        for total_budget in 1..=8 {
            let config = ConcurrencyConfig {
                max_threads: Some(total_budget),
            };
            for input_count in 0..=12 {
                for max_concurrent in [None, Some(0), Some(1), Some(3), Some(16)] {
                    #[cfg(layout_detection)]
                    let layout_workloads = [
                        LayoutBatchWorkload::None,
                        LayoutBatchWorkload::Mixed,
                        LayoutBatchWorkload::All,
                    ];
                    #[cfg(not(layout_detection))]
                    let layout_workloads = [LayoutBatchWorkload::None];
                    for layout_workload in layout_workloads {
                        let plan =
                            resolve_batch_execution_plan(Some(&config), layout_workload, input_count, max_concurrent);
                        assert!(plan.workers * plan.thread_budget <= total_budget);
                        assert!(plan.workers <= total_budget);
                        assert!(plan.workers <= input_count.max(1));
                        if let Some(explicit) = max_concurrent {
                            assert!(plan.workers <= explicit.max(1));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn test_init_thread_pools_idempotent() {
        init_thread_pools(2);
        init_thread_pools(4);
    }

    #[test]
    #[cfg(all(feature = "tokio-runtime", not(target_arch = "wasm32")))]
    fn test_batch_thread_pool_uses_total_configured_budget() {
        let config = ConcurrencyConfig { max_threads: Some(7) };
        assert_eq!(init_batch_thread_pool(Some(&config)), 7);
    }

    #[test]
    fn test_default() {
        let config = ConcurrencyConfig::default();
        assert!(config.max_threads.is_none());
    }

    #[test]
    fn test_serde_roundtrip() {
        let json = r#"{"max_threads": 2}"#;
        let config: ConcurrencyConfig = serde_json::from_str(json).unwrap();
        assert_eq!(config.max_threads, Some(2));

        let serialized = serde_json::to_string(&config).unwrap();
        let roundtripped: ConcurrencyConfig = serde_json::from_str(&serialized).unwrap();
        assert_eq!(roundtripped.max_threads, Some(2));
    }

    #[test]
    fn test_serde_empty() {
        let json = r#"{}"#;
        let config: ConcurrencyConfig = serde_json::from_str(json).unwrap();
        assert!(config.max_threads.is_none());
    }
}
