//! Issue #1391 — opt-in Prometheus `/metrics` endpoint.
//!
//! xberg never installs an OTel `MeterProvider` in production code:
//! `telemetry::metrics::get_metrics()` binds its instruments to whichever meter provider
//! is global the *first* time anything calls it, via a process-global `OnceLock`, and then
//! never looks again (see that module's doc comment, and `issue_332_telemetry_metrics.rs`
//! for the same hazard). Left alone, that resolves to the OTel no-op meter, and `/metrics`
//! would scrape an empty registry forever.
//!
//! This test calls `xberg::telemetry::init_prometheus()` explicitly, before building the
//! router or driving any extraction, to prove the documented call order actually wires a
//! real, populated registry into `GET /metrics`. It is `#[serial]` because the installed
//! meter provider — and the counters it feeds — are process-global and cumulative;
//! concurrent extractions from another test in this binary could otherwise land inside this
//! test's before/after window.
#![cfg(all(feature = "api", feature = "prometheus"))]

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use serial_test::serial;
use tower::ServiceExt;
use xberg::api::{ApiSizeLimits, create_router_with_limits};

/// Body text that has never been extracted on this machine before, so this test's
/// `/extract` call is never served from a warm cache entry left by a previous run.
fn unique_body() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock must be after the unix epoch")
        .as_nanos();
    format!("issue 1391 prometheus metrics fixture, run {nanos}")
}

/// Build a single-file multipart `/extract` request body, matching the shape used by
/// `api_extract_multipart.rs`.
fn multipart_extract_request(body_text: &str) -> Request<Body> {
    let boundary = "X-BOUNDARY";
    let body = format!(
        "--{boundary}\r\n\
Content-Disposition: form-data; name=\"files\"; filename=\"issue-1391.txt\"\r\n\
Content-Type: text/plain\r\n\
\r\n\
{body_text}\r\n\
--{boundary}--\r\n"
    );
    let body_bytes = body.into_bytes();

    Request::builder()
        .method("POST")
        .uri("/extract")
        .header("content-type", format!("multipart/form-data; boundary={boundary}"))
        .header("content-length", body_bytes.len())
        .body(Body::from(body_bytes))
        .expect("failed to build /extract request")
}

/// The exact Prometheus name that `opentelemetry-prometheus` 0.32.0 (pinned exactly in
/// `Cargo.lock`, matching this crate's `opentelemetry`/`opentelemetry_sdk` 0.32 pin) exports
/// for the `xberg.extraction.total` OTel counter (see
/// `telemetry::conventions::metrics::EXTRACTION_TOTAL`).
///
/// Confirmed by reading `opentelemetry-prometheus-0.32.0/src/lib.rs` and `src/utils.rs`
/// directly (not assumed):
/// - `Collector::get_name` runs the OTel name through `utils::sanitize_name`, which replaces
///   every non-alphanumeric/`_`/`:` character with `_` — the two dots in
///   `xberg.extraction.total` become underscores, giving `xberg_extraction_total`. No
///   namespace and no unit are configured for this counter (see `init_prometheus` and
///   `ExtractionMetrics::new`), so neither the namespace-prefix nor unit-suffix branches in
///   `get_name` fire.
/// - `Collector::metric_type_and_name` then unconditionally appends the literal
///   `COUNTER_SUFFIX = "_total"` for every monotonic `Sum` (the `without_counter_suffixes`
///   config flag, which would skip this, defaults to `false` and is never set by
///   `init_prometheus`) — regardless of whether the name already ends in `total`.
///
/// So `xberg_extraction_total` gains a second, unconditional `_total` suffix, producing
/// `xberg_extraction_total_total`. If a future `opentelemetry-prometheus` upgrade changes
/// that mangling, this constant (and the assertion using it) is exactly what must be
/// re-derived from the new pinned source before bumping it — do not guess.
const EXPECTED_EXTRACTION_TOTAL_METRIC_NAME: &str = "xberg_extraction_total_total";

/// Sum every sample value recorded for the Prometheus counter family named exactly
/// `metric_name`.
fn sum_exact_counter(metrics_body: &str, metric_name: &str) -> f64 {
    let expected_type_line = format!("# TYPE {metric_name} counter");
    assert!(
        metrics_body.lines().any(|line| line == expected_type_line),
        "no `{expected_type_line}` line was found in the /metrics body; full body follows:\n{metrics_body}"
    );

    let samples: Vec<f64> = metrics_body
        .lines()
        .filter(|line| !line.starts_with('#') && (line.starts_with(metric_name)))
        .map(|line| {
            let value_str = line
                .rsplit(' ')
                .next()
                .unwrap_or_else(|| panic!("malformed sample line, no value found: {line:?}"));
            value_str
                .parse::<f64>()
                .unwrap_or_else(|_| panic!("sample value was not a valid float: {line:?}"))
        })
        .collect();
    assert!(
        !samples.is_empty(),
        "metric family {metric_name} was declared but had no sample lines; full body:\n{metrics_body}"
    );
    samples.into_iter().sum()
}

#[tokio::test]
#[serial]
async fn metrics_endpoint_reports_extraction_total_after_a_real_extraction() {
    // Must run before this binary's first extraction, or `get_metrics()`'s `OnceLock`
    // latches onto the OTel no-op meter and this test can never pass — see the module doc.
    let _registry = xberg::telemetry::init_prometheus();

    let router = create_router_with_limits(xberg::ExtractionConfig::default(), ApiSizeLimits::default());

    let extract_response = router
        .clone()
        .oneshot(multipart_extract_request(&unique_body()))
        .await
        .expect("/extract request must succeed at the transport level");
    assert_eq!(
        extract_response.status(),
        StatusCode::OK,
        "the real extraction driving this test must itself succeed"
    );

    let metrics_response = router
        .oneshot(
            Request::builder()
                .method("GET")
                .uri("/metrics")
                .body(Body::empty())
                .expect("failed to build /metrics request"),
        )
        .await
        .expect("/metrics request must succeed at the transport level");

    assert_eq!(metrics_response.status(), StatusCode::OK, "/metrics must return 200");
    let content_type = metrics_response
        .headers()
        .get(axum::http::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .expect("/metrics must set a Content-Type header")
        .to_string();
    assert_eq!(
        content_type, "text/plain; version=0.0.4",
        "/metrics must use the Prometheus text exposition format, not JSON"
    );

    let body_bytes = to_bytes(metrics_response.into_body(), 10 * 1024 * 1024)
        .await
        .expect("failed to read /metrics response body");
    let metrics_body = String::from_utf8(body_bytes.to_vec()).expect("/metrics body must be valid UTF-8");

    let extraction_total = sum_exact_counter(&metrics_body, EXPECTED_EXTRACTION_TOTAL_METRIC_NAME);
    assert!(
        extraction_total > 0.0,
        "expected metric {EXPECTED_EXTRACTION_TOTAL_METRIC_NAME} to be non-zero after a real \
         extraction; full /metrics body follows:\n{metrics_body}"
    );
}
