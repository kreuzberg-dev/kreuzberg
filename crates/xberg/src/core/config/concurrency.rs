//! Concurrency and thread pool configuration.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Once, OnceLock};

use serde::{Deserialize, Serialize};

/// Controls thread usage for constrained environments.
///
/// Set `max_threads` to cap all internal thread pools (Rayon, ONNX Runtime
/// intra-op) and batch concurrency to a single limit.
///
/// # Default budget when `max_threads` is unset
///
/// Without an explicit `max_threads`, the effective budget is
/// `min(detected_cpu_cores, 8)` — a deliberate ceiling chosen for
/// serverless/shared-tenant defaults, not a full-host auto-scale. On a host
/// with more than 8 cores this means the extra cores go **unused** unless one
/// of the following applies:
///
/// - `max_threads` is set explicitly above 8 (the only way to exceed the
///   ceiling on a bare-metal or VM host with no CPU quota).
/// - The process runs under a Linux cgroup CPU quota (containers, Kubernetes
///   `resources.limits.cpu`); in that case the quota itself is used as the
///   ceiling instead of the hardcoded 8, since the quota already reflects a
///   deliberately-configured resource limit.
///
/// When neither applies and the host has more than 8 cores, a single
/// `WARN`-level log is emitted the first time the budget is resolved,
/// naming the detected core count and the applied cap, so the ceiling is
/// discoverable without reading source.
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
    /// the effective budget is `min(detected_cpu_cores, 8)` unless a Linux
    /// cgroup CPU quota is present, in which case the quota is used as the
    /// ceiling instead. On hosts with more than 8 cores and no cgroup quota,
    /// set `max_threads` explicitly to use the additional cores — the
    /// default will not scale past 8 on its own.
    pub max_threads: Option<usize>,
}

static POOL_INIT: Once = Once::new();
static ACTIVE_THREAD_BUDGET: AtomicUsize = AtomicUsize::new(0);

/// Ceiling applied to the auto-detected thread budget when `max_threads` is
/// unset and no tighter resource limit (e.g. a cgroup CPU quota) is found.
///
/// This is a deliberate serverless/shared-tenant default, not a scaling
/// limit of the underlying pipelines — see [`ConcurrencyConfig::max_threads`].
const DEFAULT_THREAD_CAP: usize = 8;

/// Guards the one-time startup warning for the default-cap fallback so it
/// fires at most once per process, not once per extraction.
static DEFAULT_CAP_WARNED: AtomicBool = AtomicBool::new(false);

/// Emit the "unused cores" warning at most once per `already_warned` guard.
///
/// The guard is a parameter rather than a direct read of [`DEFAULT_CAP_WARNED`]
/// so that the once-per-process property can be tested against a guard the test
/// owns. Asserting on the global is not merely racy but unfixably so: any test
/// in the binary that runs a real extraction reaches `resolve_thread_budget`
/// and trips the global first on a host with more than [`DEFAULT_THREAD_CAP`]
/// cores, and `#[serial]` cannot help because it only excludes other `#[serial]`
/// tests. That is the same defect class as #215.
fn warn_default_thread_cap_once(already_warned: &AtomicBool, host_cpus: usize) {
    if already_warned
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        tracing::warn!(
            host_cpus,
            thread_cap = DEFAULT_THREAD_CAP,
            "detected {host_cpus} CPU cores but no `max_threads` is configured and no cgroup CPU \
             quota was found; capping the thread budget at {DEFAULT_THREAD_CAP} \
             (min(cpu_cores, {DEFAULT_THREAD_CAP})). Set `ConcurrencyConfig::max_threads` above \
             {DEFAULT_THREAD_CAP} to use the remaining cores."
        );
    }
}

/// The cgroup CPU quota, resolved at most once per process.
///
/// `resolve_thread_budget` runs per extracted document (see
/// `core::extractor::file`), and the quota cannot change under a running
/// process, so reading `/sys/fs/cgroup/...` on every call would be two
/// syscalls per document for a value that never moves.
static CGROUP_QUOTA_CORES: OnceLock<Option<usize>> = OnceLock::new();

/// Detect an effective CPU core cap from the process's Linux cgroup CPU
/// quota, if any.
///
/// Returns `None` when no quota is configured (bare metal, most developer
/// machines, and non-Linux platforms) — callers then fall back to
/// [`DEFAULT_THREAD_CAP`]. Returns `Some(cores)` when a cgroup v2 (`cpu.max`)
/// or v1 (`cpu.cfs_quota_us` / `cpu.cfs_period_us`) quota is present and
/// finite, rounded up to the nearest whole core. Never panics: any read or
/// parse failure is treated as "no quota".
fn cgroup_cpu_quota_cores() -> Option<usize> {
    *CGROUP_QUOTA_CORES.get_or_init(read_cgroup_cpu_quota_cores)
}

#[cfg(target_os = "linux")]
fn read_cgroup_cpu_quota_cores() -> Option<usize> {
    cgroup_v2_quota_cores().or_else(cgroup_v1_quota_cores)
}

#[cfg(not(target_os = "linux"))]
fn read_cgroup_cpu_quota_cores() -> Option<usize> {
    None
}

#[cfg(target_os = "linux")]
fn cgroup_v2_quota_cores() -> Option<usize> {
    let contents = std::fs::read_to_string("/sys/fs/cgroup/cpu.max").ok()?;
    let mut fields = contents.split_whitespace();
    let quota_field = fields.next()?;
    let period_field = fields.next()?;
    if quota_field == "max" {
        return None;
    }
    quota_period_to_cores(quota_field.parse().ok()?, period_field.parse().ok()?)
}

#[cfg(target_os = "linux")]
fn cgroup_v1_quota_cores() -> Option<usize> {
    let quota: f64 = std::fs::read_to_string("/sys/fs/cgroup/cpu/cpu.cfs_quota_us")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    let period: f64 = std::fs::read_to_string("/sys/fs/cgroup/cpu/cpu.cfs_period_us")
        .ok()?
        .trim()
        .parse()
        .ok()?;
    quota_period_to_cores(quota, period)
}

/// `cpu.cfs_quota_us` of `-1` (v1) and a bare `max` period (v2, handled by the
/// caller) both mean "unlimited" and must resolve to `None`, not a huge cap.
#[cfg(target_os = "linux")]
fn quota_period_to_cores(quota: f64, period: f64) -> Option<usize> {
    if quota <= 0.0 || period <= 0.0 {
        return None;
    }
    Some((quota / period).ceil().max(1.0) as usize)
}

/// Resolve the effective thread budget from config or auto-detection.
///
/// User-set `max_threads` takes priority. Otherwise auto-detects from
/// `num_cpus`, preferring a detected Linux cgroup CPU quota as the ceiling
/// when present, and falling back to [`DEFAULT_THREAD_CAP`] otherwise. See
/// the [`ConcurrencyConfig`] docs for the full default-budget explanation.
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
    resolve_thread_budget_inner(config, num_cpus::get(), cgroup_cpu_quota_cores())
}

/// Pure core of [`resolve_thread_budget`], parameterized on the host CPU
/// count and any detected cgroup quota so tests can exercise every branch
/// deterministically regardless of the machine actually running the tests.
fn resolve_thread_budget_inner(
    config: Option<&ConcurrencyConfig>,
    host_cpus: usize,
    quota_cores: Option<usize>,
) -> usize {
    resolve_thread_budget_with_guard(config, host_cpus, quota_cores, &DEFAULT_CAP_WARNED)
}

/// As [`resolve_thread_budget_inner`], but against a caller-supplied
/// "already warned" guard — see [`warn_default_thread_cap_once`] for why the
/// warning tests cannot share the process-global one.
fn resolve_thread_budget_with_guard(
    config: Option<&ConcurrencyConfig>,
    host_cpus: usize,
    quota_cores: Option<usize>,
    already_warned: &AtomicBool,
) -> usize {
    if let Some(n) = config.and_then(|c| c.max_threads) {
        return n.max(1);
    }
    match quota_cores {
        // `clamp` rather than `min().max()`: both bounds are known non-zero here
        // (`cgroup_cpu_quota_cores` floors its result at 1), so it cannot panic.
        Some(quota_cores) => host_cpus.clamp(1, quota_cores.max(1)),
        None => {
            if host_cpus > DEFAULT_THREAD_CAP {
                warn_default_thread_cap_once(already_warned, host_cpus);
            }
            host_cpus.clamp(1, DEFAULT_THREAD_CAP)
        }
    }
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
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::layer::SubscriberExt as _;
    use tracing_subscriber::{EnvFilter, Layer};

    use super::*;

    /// A tracing `Layer` that records the level of every emitted event.
    #[derive(Clone, Default)]
    struct EventCapture {
        levels: Arc<Mutex<Vec<tracing::Level>>>,
    }

    impl<S> Layer<S> for EventCapture
    where
        S: tracing::Subscriber,
    {
        fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
            self.levels.lock().unwrap().push(*event.metadata().level());
        }
    }

    fn warn_event_count(capture: &EventCapture) -> usize {
        capture
            .levels
            .lock()
            .unwrap()
            .iter()
            .filter(|level| **level == tracing::Level::WARN)
            .count()
    }

    #[test]
    fn test_resolve_thread_budget_none() {
        let budget = resolve_thread_budget(None);
        assert!(budget >= 1);
        assert!(budget <= 8);
    }

    // -- Pure-function pinning: resolve_thread_budget_inner --------------------
    //
    // These exercise resolve_thread_budget_inner directly with injected
    // host_cpus/quota_cores so the assertions are deterministic regardless of
    // the machine actually running the test suite (unlike resolve_thread_budget,
    // which reads real num_cpus/cgroup state).

    #[test]
    fn test_inner_pins_default_cap_when_no_quota_and_no_max_threads() {
        assert_eq!(resolve_thread_budget_inner(None, 1, None), 1);
        assert_eq!(resolve_thread_budget_inner(None, 4, None), 4);
        assert_eq!(resolve_thread_budget_inner(None, 8, None), 8);
        assert_eq!(resolve_thread_budget_inner(None, 16, None), 8);
        assert_eq!(resolve_thread_budget_inner(None, 64, None), 8);
    }

    #[test]
    fn test_inner_explicit_max_threads_wins_over_host_cpus_and_quota() {
        let config = ConcurrencyConfig { max_threads: Some(20) };
        assert_eq!(resolve_thread_budget_inner(Some(&config), 4, Some(2)), 20);
        assert_eq!(resolve_thread_budget_inner(Some(&config), 64, None), 20);
    }

    #[test]
    fn test_inner_explicit_max_threads_of_zero_clamps_to_one() {
        let config = ConcurrencyConfig { max_threads: Some(0) };
        assert_eq!(resolve_thread_budget_inner(Some(&config), 16, None), 1);
    }

    #[test]
    fn test_inner_cgroup_quota_above_default_cap_is_not_clamped_to_eight() {
        // This is the #1392 fix: a real cgroup quota larger than the
        // hardcoded serverless default must be honoured, not silently
        // clamped to 8 the way the unset/no-quota path is.
        assert_eq!(resolve_thread_budget_inner(None, 64, Some(24)), 24);
        assert_eq!(resolve_thread_budget_inner(None, 64, Some(9)), 9);
    }

    #[test]
    fn test_inner_cgroup_quota_below_default_cap_is_used_as_is() {
        assert_eq!(resolve_thread_budget_inner(None, 64, Some(3)), 3);
    }

    #[test]
    fn test_inner_cgroup_quota_never_exceeds_host_cpus() {
        assert_eq!(resolve_thread_budget_inner(None, 4, Some(16)), 4);
    }

    // -- One-time warning ---------------------------------------------------
    //
    // `#[serial]`: DEFAULT_CAP_WARNED is a process-global static with no
    // injectable variant, so concurrent tests resetting/reading it would race
    // each other — same reasoning as the other process-global-static tests in
    // this crate (see core::pipeline::mod::tests for the established pattern).

    #[test]
    #[serial_test::serial]
    fn test_default_cap_warning_fires_exactly_once_when_cores_exceed_cap_and_unset() {
        let already_warned = AtomicBool::new(false);
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("warn"))
            .with(capture.clone());

        tracing::subscriber::with_default(subscriber, || {
            for _ in 0..5 {
                assert_eq!(resolve_thread_budget_with_guard(None, 16, None, &already_warned), 8);
            }
        });

        assert_eq!(
            warn_event_count(&capture),
            1,
            "expected exactly one WARN event across repeated calls, got {:?}",
            capture.levels.lock().unwrap()
        );
    }

    #[test]
    #[serial_test::serial]
    fn test_default_cap_warning_does_not_fire_when_max_threads_is_set() {
        let already_warned = AtomicBool::new(false);
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("warn"))
            .with(capture.clone());

        tracing::subscriber::with_default(subscriber, || {
            let config = ConcurrencyConfig { max_threads: Some(4) };
            resolve_thread_budget_with_guard(Some(&config), 16, None, &already_warned)
        });

        assert_eq!(warn_event_count(&capture), 0);
    }

    #[test]
    #[serial_test::serial]
    fn test_default_cap_warning_does_not_fire_when_cores_at_or_below_default_cap() {
        let already_warned = AtomicBool::new(false);
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("warn"))
            .with(capture.clone());

        tracing::subscriber::with_default(subscriber, || {
            resolve_thread_budget_with_guard(None, 8, None, &already_warned);
            resolve_thread_budget_with_guard(None, 1, None, &already_warned);
        });

        assert_eq!(warn_event_count(&capture), 0);
    }

    #[test]
    #[serial_test::serial]
    fn test_default_cap_warning_does_not_fire_when_cgroup_quota_present() {
        let already_warned = AtomicBool::new(false);
        let capture = EventCapture::default();
        let subscriber = tracing_subscriber::registry()
            .with(EnvFilter::new("warn"))
            .with(capture.clone());

        tracing::subscriber::with_default(subscriber, || {
            resolve_thread_budget_with_guard(None, 16, Some(12), &already_warned);
        });

        assert_eq!(warn_event_count(&capture), 0);
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
