//! Issue #332 — `telemetry::conventions::metrics` declares `CACHE_HITS`, `CACHE_MISSES`,
//! `BATCH_TOTAL` and `BATCH_DURATION_MS`, and `telemetry::metrics` declares the
//! corresponding OTel instruments, but no code path ever recorded any of them: an
//! operator who wired up an OTLP metrics exporter saw four permanently-empty metrics.
//!
//! These tests install a real `opentelemetry_sdk` `SdkMeterProvider` backed by an
//! `InMemoryMetricExporter`, drive the real public `xberg::extract` /
//! `xberg::extract_batch` API, and assert on the exact recorded counter/histogram
//! values — not on a tracing event, so a regression that keeps the tracing calls but
//! drops the metric calls (or vice versa) still fails here.
//!
//! ## Why a process-global provider, and why `#[serial]`
//!
//! The four instruments live behind `xberg::telemetry::metrics::get_metrics()`'s
//! process-global `OnceLock`, which binds permanently to whichever
//! `opentelemetry::global` meter provider is installed the *first* time anything in
//! this process calls it (see `opentelemetry::global::meter`'s own doc: "If the global
//! `MeterProvider` is changed after getting `Meter` instances from these calls, the
//! `Meter` instances returned will not reflect the change"). `test_env()` below installs
//! the test provider, guarded by a `OnceLock`, before anything else in this binary can
//! reach `get_metrics()`. Every test in this file is `#[serial]` so that one test's
//! recordings cannot land inside another test's before/after snapshot window on the
//! shared, cumulative counters.
#![cfg(feature = "otel")]

use std::sync::OnceLock;
use std::time::Duration;

use opentelemetry::global;
use opentelemetry_sdk::metrics::data::{AggregatedMetrics, MetricData, ResourceMetrics};
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use serial_test::serial;

use xberg::telemetry::conventions::metrics as names;
use xberg::{ExtractInput, ExtractionConfig};

/// Everything needed to read back what this process's global
/// `xberg::telemetry::metrics::ExtractionMetrics` instruments have recorded so far.
struct TestMeterEnv {
    provider: SdkMeterProvider,
    exporter: InMemoryMetricExporter,
}

/// Install the test `SdkMeterProvider` as the global OTel meter provider, once per
/// process, and hand back a handle to flush and read it.
///
/// Must run before the first `xberg` extraction in this binary: `get_metrics()`'s
/// `OnceLock` captures whatever provider is global at that moment and never looks again.
fn test_env() -> &'static TestMeterEnv {
    static ENV: OnceLock<TestMeterEnv> = OnceLock::new();
    ENV.get_or_init(|| {
        // A long interval keeps the reader's background thread from ever firing on its
        // own; every collection in this file is driven explicitly via `force_flush`.
        let exporter = InMemoryMetricExporter::default();
        let reader = PeriodicReader::builder(exporter.clone())
            .with_interval(Duration::from_secs(3600))
            .build();
        let provider = SdkMeterProvider::builder().with_reader(reader).build();

        global::set_meter_provider(provider.clone());

        TestMeterEnv { provider, exporter }
    })
}

/// Body text that has never been extracted on this machine before.
///
/// The extraction cache is keyed on a content hash, so the "first lookup is a miss"
/// assertion below is only true for content the cache has not already seen — and a
/// second run of this test would otherwise start from a warm entry left by the first.
/// Redirecting the cache to a temp directory would need `std::env::set_var`, which is
/// `unsafe` in edition 2024 and denied workspace-wide (`unsafe_code = "deny"`), so
/// uniqueness is established through the content itself instead.
fn unique_body() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after the unix epoch")
        .as_nanos();
    format!("issue 332 telemetry metrics fixture, run {nanos}")
}

/// Install the test meter provider and hand back the meter environment.
/// Idempotent — safe to call at the top of every test.
fn setup() -> &'static TestMeterEnv {
    test_env()
}

/// Sum of every `u64` counter data point currently recorded for `metric_name`. None of
/// the metrics asserted on in this file carry attributes, so this is always the single
/// combined total regardless of how many distinct data points the SDK produced.
fn read_u64_sum(resource_metrics: &[ResourceMetrics], metric_name: &str) -> u64 {
    resource_metrics
        .iter()
        .flat_map(|resource| resource.scope_metrics())
        .flat_map(|scope| scope.metrics())
        .filter(|metric| metric.name() == metric_name)
        .filter_map(|metric| match metric.data() {
            AggregatedMetrics::U64(MetricData::Sum(sum)) => {
                Some(sum.data_points().map(|point| point.value()).sum::<u64>())
            }
            _ => None,
        })
        .sum()
}

/// Total number of histogram observations (`count`, summed across data points)
/// currently recorded for `metric_name`.
fn read_f64_histogram_count(resource_metrics: &[ResourceMetrics], metric_name: &str) -> u64 {
    resource_metrics
        .iter()
        .flat_map(|resource| resource.scope_metrics())
        .flat_map(|scope| scope.metrics())
        .filter(|metric| metric.name() == metric_name)
        .filter_map(|metric| match metric.data() {
            AggregatedMetrics::F64(MetricData::Histogram(histogram)) => {
                Some(histogram.data_points().map(|point| point.count()).sum::<u64>())
            }
            _ => None,
        })
        .sum()
}

/// Cumulative values of every metric this file asserts on, as of the moment this is
/// called — obtained via a fresh, synchronous force-flush of the test provider.
struct Snapshot {
    cache_hits: u64,
    cache_misses: u64,
    batch_total: u64,
    batch_duration_updates: u64,
}

fn snapshot(env: &TestMeterEnv) -> Snapshot {
    // `reset()` only clears the exporter's own already-exported buffer; it does not
    // reset the SDK's cumulative aggregation, so `force_flush` below still reports the
    // running totals-to-date (the temporality is `Cumulative`, the SDK default).
    env.exporter.reset();
    env.provider
        .force_flush()
        .expect("force_flush must succeed against the in-memory exporter");
    let collected = env
        .exporter
        .get_finished_metrics()
        .expect("the in-memory exporter's internal lock must not be poisoned");

    Snapshot {
        cache_hits: read_u64_sum(&collected, names::CACHE_HITS),
        cache_misses: read_u64_sum(&collected, names::CACHE_MISSES),
        batch_total: read_u64_sum(&collected, names::BATCH_TOTAL),
        batch_duration_updates: read_f64_histogram_count(&collected, names::BATCH_DURATION_MS),
    }
}

fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("current-thread runtime must build")
        .block_on(future)
}

/// #332 — the first extraction of new content under a fresh cache namespace must record
/// a `xberg.extraction.cache.misses` increment (and no hit); an immediately-repeated
/// extraction of the identical content and config must then be served from the cache
/// and record a `xberg.extraction.cache.hits` increment (and no additional miss).
///
/// Bytes-input extraction never reaches the file-backed extraction cache (only the
/// file/URI path does — see `core::extractor::file::extract_file_with_extractor`), so
/// this drives the public API through a real temp file and a local-path `ExtractInput`,
/// exactly as `crawlberg`-free local extraction does in production.
#[test]
#[serial]
fn cache_lookup_should_record_a_miss_then_a_hit() {
    let env = setup();

    let source_dir = tempfile::tempdir().expect("source temp dir must be creatable");
    let path = source_dir.path().join("issue-332-cache.txt");
    std::fs::write(&path, unique_body()).expect("fixture file must be writable");

    let config = ExtractionConfig {
        cache_namespace: Some("issue-332-cache-lookup".to_string()),
        ..Default::default()
    };
    let input = || ExtractInput::from_uri(path.to_string_lossy().into_owned());

    let before = snapshot(env);

    let miss_output = block_on(xberg::extract(input(), &config)).expect("the first (uncached) extraction must succeed");
    assert_eq!(miss_output.results.len(), 1, "expected exactly one extraction result");

    let after_miss = snapshot(env);
    assert_eq!(
        after_miss.cache_misses,
        before.cache_misses + 1,
        "extracting new content under a fresh cache namespace must record exactly one cache miss"
    );
    assert_eq!(
        after_miss.cache_hits, before.cache_hits,
        "the first (uncached) extraction must not also record a cache hit"
    );

    let hit_output = block_on(xberg::extract(input(), &config)).expect("the second (cached) extraction must succeed");
    assert_eq!(hit_output.results.len(), 1, "expected exactly one extraction result");

    let after_hit = snapshot(env);
    assert_eq!(
        after_hit.cache_hits,
        after_miss.cache_hits + 1,
        "re-extracting identical content under the same config must record exactly one cache hit"
    );
    assert_eq!(
        after_hit.cache_misses, after_miss.cache_misses,
        "the cached extraction must not also record a cache miss"
    );
}

/// #332 — one `xberg::extract_batch` call must record exactly one `xberg.batch.total`
/// increment and exactly one `xberg.batch.duration_ms` observation, regardless of how
/// many items the batch contains.
#[test]
#[serial]
fn extract_batch_should_record_batch_total_and_duration_metrics() {
    let env = setup();

    let inputs = vec![
        ExtractInput::from_bytes(
            b"issue-332 batch metrics fixture, item one".to_vec(),
            "text/plain",
            None,
        ),
        ExtractInput::from_bytes(
            b"issue-332 batch metrics fixture, item two".to_vec(),
            "text/plain",
            None,
        ),
    ];
    let config = ExtractionConfig {
        use_cache: false,
        ..Default::default()
    };

    let before = snapshot(env);

    let output = block_on(xberg::extract_batch(inputs, &config)).expect("batch extraction must succeed");
    assert_eq!(output.results.len(), 2, "expected both batch items to succeed");

    let after = snapshot(env);
    assert_eq!(
        after.batch_total,
        before.batch_total + 1,
        "one extract_batch call must record exactly one xberg.batch.total increment"
    );
    assert_eq!(
        after.batch_duration_updates,
        before.batch_duration_updates + 1,
        "one extract_batch call must record exactly one xberg.batch.duration_ms observation"
    );
}
