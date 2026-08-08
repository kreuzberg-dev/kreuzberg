//! Telemetry and observability for xberg.
//!
//! This module provides:
//! - **Semantic conventions** ([`conventions`]) — constant attribute and metric
//!   names following the `xberg.*` namespace.
//! - **Span helpers** (`spans`) — functions to create properly-attributed
//!   tracing spans (requires `otel` feature).
//! - **Metrics instruments** (`metrics`) — counters, histograms, and gauges
//!   for monitoring extraction operations (requires `otel` feature).
//! - **Prometheus support** ([`init_prometheus`]) — installs a Prometheus-backed
//!   `SdkMeterProvider` as the global OTel meter provider (requires `prometheus` feature).
//!
//! The `conventions` module is always available (it's just string constants).
//! The `spans` and `metrics` modules are gated behind the `otel` feature.
//! `init_prometheus` is gated behind the `prometheus` feature.

pub mod conventions;

#[cfg(feature = "otel")]
pub mod metrics;

#[cfg(feature = "otel")]
pub mod spans;

#[cfg(feature = "prometheus")]
mod prometheus_exporter;

#[cfg(feature = "prometheus")]
pub use prometheus_exporter::init_prometheus;
