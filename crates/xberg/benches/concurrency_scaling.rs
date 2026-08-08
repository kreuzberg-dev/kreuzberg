//! Cores-vs-throughput benchmark for batch document concurrency.
//!
//! This is the benchmark referenced by `docs-site/src/content/docs/guides/concurrency.mdx`
//! ("Layout inference: adding cores will not help"). It sweeps `ConcurrencyConfig::max_threads`
//! against two batch cohorts and measures wall-clock throughput:
//!
//! - `non_layout_batch`: plain Markdown documents, no layout inference. Batch worker count
//!   (`resolve_batch_execution_plan`'s `workers`) follows the normal ceiling, so throughput is
//!   expected to scale up as the thread budget grows.
//! - `all_layout_pdf_batch`: every input is a PDF processed with layout-driven Markdown
//!   (`use_layout_for_markdown = true`), which `classify_layout_batch` classifies as
//!   `LayoutBatchWorkload::All`. `resolve_batch_execution_plan` then pins the batch to a single
//!   document worker (`MAX_NATIVE_LAYOUT_BATCH_WORKERS = 1`) regardless of the thread budget, so
//!   throughput is expected to stay flat across the sweep.
//!
//! The contrast between the two cohorts is the result: a single-cohort run proves nothing about
//! whether the flattening is specific to layout batches or just a property of small batches.
//!
//! # Threats to validity and how this benchmark controls for them
//!
//! 1. **Content-hash cache.** `xberg`'s extraction cache is keyed on file bytes + config, so a
//!    warm cache would make every sample after the first measure a cache lookup instead of real
//!    extraction work. Every [`ExtractionConfig`] below sets `use_cache: false`.
//! 2. **First-run model loading.** The RT-DETR layout engine is held in a single-slot,
//!    shape/config-keyed cache (`xberg::layout::take_or_create_engine`); switching
//!    `max_threads` changes the per-session thread budget, which is part of that cache key
//!    (`LayoutEngine::matches_config`), so every new sweep point forces one cold ONNX Runtime
//!    session load. Criterion's own warm-up phase runs the exact closure being measured before
//!    any sample is recorded, so that cold load lands in warm-up, not in the measured samples —
//!    but `warm_up_time` is set generously (10s) specifically to make sure the cold load
//!    finishes before sampling starts, and each `(cohort, threads)` pair is registered as its
//!    own `bench_function` so the cache is never invalidated mid-measurement.
//! 3. **ORT intra-op threads vs. document-worker count.** These are two different knobs and
//!    this benchmark must not conflate them. `resolve_batch_execution_plan` derives the
//!    document-level *worker* count (a `tokio::sync::Semaphore` permit count consumed by
//!    `extract_batch`'s `JoinSet` scheduling) independently of ONNX Runtime's *intra-op* thread
//!    count (set per-session via `with_intra_threads`, see `layout::session::build_session`).
//!    Raising `max_threads` for an all-layout-PDF batch changes the latter (more intra-op
//!    threads for the one active session) without changing the former (still one worker) — that
//!    split is exactly the documented claim, so both effects must be free to move independently.
//!    The confound to guard against is a *third*, unrelated pool: `xberg`'s global Rayon pool is
//!    initialized at most once per process (`core::config::concurrency::init_thread_pools` is
//!    guarded by a `std::sync::Once`), so if this benchmark's first extraction call happened to
//!    run at a small `max_threads`, every later sweep point sharing this process would be
//!    starved of Rayon threads regardless of its own configured budget — which would understate
//!    scaling for `non_layout_batch` for reasons that have nothing to do with the documented
//!    batch-worker policy. `warm_up_rayon_pool_at_max_threads` below forces that one-time
//!    initialization to happen at the *largest* swept thread budget before any measurement
//!    begins, so no sweep point is ever bottlenecked on an undersized shared pool. This makes
//!    the reported non-layout numbers, if anything, slightly conservative (small-budget samples
//!    have access to more shared-pool capacity than a fresh process would give them) rather than
//!    inflated — it cannot manufacture the scaling effect this benchmark is trying to detect,
//!    it can only hide it. If `non_layout_batch` still shows a flat line, that is a real,
//!    interesting finding.
//!
//! # Running
//!
//! ```bash
//! cargo bench --bench concurrency_scaling --features pdf,layout-detection,ocr
//! ```
//!
//! Requires a `[[bench]]` entry in `crates/xberg/Cargo.toml`:
//!
//! ```toml
//! [[bench]]
//! name = "concurrency_scaling"
//! harness = false
//! required-features = ["pdf", "layout-detection", "ocr"]
//! ```

use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use sha2::{Digest, Sha256};
use xberg::core::config::{
    AccelerationConfig, ConcurrencyConfig, ExecutionProviderType, ExtractInput, ExtractionConfig, LayoutDetectionConfig,
};

/// Number of independent documents per batch call.
///
/// Must be at least as large as the largest swept thread budget, otherwise the
/// non-layout cohort's worker ceiling would be capped by `input_count` rather than by
/// `max_threads`, and the sweep would not actually exercise the budget it claims to.
const BATCH_SIZE: usize = 16;

const NON_LAYOUT_FIXTURE: &str = "markdown/hip_13044_b.md";
const NON_LAYOUT_FIXTURE_SHA256: &str = "750913ed94efc2d95545e4c0e56543f7e6a334478e817653ab72213f2944f89c";

const LAYOUT_PDF_FIXTURE: &str = "pdf/681693.pdf";
const LAYOUT_PDF_FIXTURE_SHA256: &str = "133592c082044b981bb96947ccf8141c66d279375023a823fa3e681c3c8b17e5";

fn test_documents_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../test_documents")
}

fn load_pinned_fixture(relative_path: &str, expected_sha256: &str) -> Vec<u8> {
    let path = test_documents_root().join(relative_path);
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|error| panic!("failed to read pinned fixture {}: {error}", path.display()));
    let actual_sha256 = hex::encode(Sha256::digest(&bytes));
    assert_eq!(
        actual_sha256,
        expected_sha256,
        "pinned fixture changed: {}",
        path.display()
    );
    bytes
}

/// Thread-budget sweep: 1, 2, 4, and the host's cap (matching the documented
/// `min(cpu_cores, 8)` default ceiling), deduplicated and sorted ascending.
fn thread_budget_sweep() -> Vec<usize> {
    let host_cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    let default_cap = host_cores.min(8);
    let mut sweep: Vec<usize> = [1_usize, 2, 4, default_cap]
        .into_iter()
        .filter(|&threads| threads <= host_cores)
        .collect();
    sweep.sort_unstable();
    sweep.dedup();
    sweep
}

fn tokio_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(
            std::thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1)
                .max(8),
        )
        .enable_all()
        .build()
        .expect("failed to build the Tokio runtime backing the concurrency-scaling benchmark")
}

fn extraction_config(max_threads: usize, layout_active: bool) -> ExtractionConfig {
    ExtractionConfig {
        use_cache: false,
        // Isolate document-worker/thread-budget scaling from Tesseract OCR latency: neither
        // fixture is a scanned document, but this keeps the claim under test (layout-model
        // inference speed) from being diluted by an unrelated OCR path.
        disable_ocr: true,
        concurrency: Some(ConcurrencyConfig {
            max_threads: Some(max_threads),
        }),
        layout: layout_active.then(|| LayoutDetectionConfig {
            acceleration: Some(AccelerationConfig {
                provider: ExecutionProviderType::Cpu,
                ..AccelerationConfig::default()
            }),
            ..LayoutDetectionConfig::default()
        }),
        use_layout_for_markdown: layout_active,
        ..ExtractionConfig::default()
    }
}

fn non_layout_inputs(fixture: &[u8]) -> Vec<ExtractInput> {
    (0..BATCH_SIZE)
        .map(|_| ExtractInput::from_bytes(fixture.to_vec(), "text/markdown", None))
        .collect()
}

/// All-layout-PDF batch. Fewer documents than `BATCH_SIZE` since
/// `LayoutBatchWorkload::All` pins this batch to a single worker regardless of size —
/// growing it would only lengthen the benchmark, not exercise a different code path.
const LAYOUT_BATCH_SIZE: usize = 3;

fn all_layout_pdf_inputs(fixture: &[u8]) -> Vec<ExtractInput> {
    (0..LAYOUT_BATCH_SIZE)
        .map(|_| ExtractInput::from_bytes(fixture.to_vec(), "application/pdf", None))
        .collect()
}

/// Force the one-time global Rayon pool initialization (see the module-level doc comment,
/// point 3) to happen at the largest swept thread budget, before any measurement begins.
fn warm_up_rayon_pool_at_max_threads(runtime: &tokio::runtime::Runtime, non_layout_fixture: &[u8], max_threads: usize) {
    let config = extraction_config(max_threads, false);
    let inputs = non_layout_inputs(non_layout_fixture);
    runtime
        .block_on(xberg::extract_batch(inputs, &config))
        .expect("Rayon pool warm-up batch failed");
}

fn bench_concurrency_scaling(criterion: &mut Criterion) {
    let non_layout_fixture = load_pinned_fixture(NON_LAYOUT_FIXTURE, NON_LAYOUT_FIXTURE_SHA256);
    let layout_pdf_fixture = load_pinned_fixture(LAYOUT_PDF_FIXTURE, LAYOUT_PDF_FIXTURE_SHA256);
    let sweep = thread_budget_sweep();
    let runtime = tokio_runtime();

    let max_swept_threads = *sweep.last().expect("thread_budget_sweep must be non-empty");
    warm_up_rayon_pool_at_max_threads(&runtime, &non_layout_fixture, max_swept_threads);

    let mut group = criterion.benchmark_group("concurrency_scaling");
    for &threads in &sweep {
        let non_layout_config = extraction_config(threads, false);
        group.bench_function(format!("non_layout_batch/threads_{threads}"), |bencher| {
            bencher.iter(|| {
                let inputs = black_box(non_layout_inputs(&non_layout_fixture));
                let result = runtime
                    .block_on(xberg::extract_batch(inputs, &non_layout_config))
                    .expect("non-layout batch extraction failed");
                black_box(result)
            })
        });

        let layout_config = extraction_config(threads, true);
        group.bench_function(format!("all_layout_pdf_batch/threads_{threads}"), |bencher| {
            bencher.iter(|| {
                let inputs = black_box(all_layout_pdf_inputs(&layout_pdf_fixture));
                let result = runtime
                    .block_on(xberg::extract_batch(inputs, &layout_config))
                    .expect("all-layout-PDF batch extraction failed");
                black_box(result)
            })
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(10)
        .warm_up_time(Duration::from_secs(10))
        .measurement_time(Duration::from_secs(30));
    targets = bench_concurrency_scaling
}
criterion_main!(benches);
