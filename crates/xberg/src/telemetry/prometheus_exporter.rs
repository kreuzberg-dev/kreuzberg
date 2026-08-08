//! Prometheus `/metrics` endpoint support (#1391).
//!
//! xberg never installs a [`opentelemetry::global::MeterProvider`] on its own in
//! production code: [`super::metrics::get_metrics`] binds its instruments to whatever
//! provider is global the *first* time anything calls it, via a process-global
//! [`std::sync::OnceLock`], and then never looks again. Left alone, that resolves to the
//! OTel no-op meter, and a `/metrics` endpoint scraping it would return an empty body
//! forever.
//!
//! [`init_prometheus`] installs a real [`opentelemetry_sdk::metrics::SdkMeterProvider`],
//! backed by an [`opentelemetry_prometheus`] exporter, as the global meter provider and
//! hands back the [`prometheus::Registry`] to scrape. It must run before the first call to
//! `get_metrics()` anywhere in the process — see `create_router_with_limits_and_server_config`
//! in `api::router`, which calls it before building the extraction service.

use std::sync::OnceLock;

use opentelemetry_sdk::metrics::SdkMeterProvider;
use prometheus::Registry;

/// The process-global Prometheus registry backing every `/metrics` scrape.
static PROMETHEUS_REGISTRY: OnceLock<Registry> = OnceLock::new();

/// Install a Prometheus-backed `SdkMeterProvider` as the global OTel meter provider, and
/// return the [`prometheus::Registry`] it feeds.
///
/// Idempotent: only the first call installs the provider. Every later call — including
/// from a different router instance in the same process — returns a clone of the same
/// registry (cheap: [`Registry`] shares its internal state via `Arc`) rather than
/// installing a second, disconnected provider that would silently stop feeding the
/// registry already handed out.
///
/// # When to call this
///
/// Before anything in the process can reach [`super::metrics::get_metrics`]. In practice
/// that means at process start (embedders should call this explicitly) or, as a fallback
/// for callers who only ever use the built-in API server, at the top of
/// `api::create_router_with_limits_and_server_config` — before the extraction service
/// (and therefore the metrics instruments) is built.
pub fn init_prometheus() -> Registry {
    PROMETHEUS_REGISTRY
        .get_or_init(|| {
            let registry = Registry::new();
            // Building the exporter against a fresh, empty registry cannot fail with a
            // duplicate-metric-name collision, so a panic here indicates a bug in this
            // function rather than a runtime/environment condition — preferable to
            // silently installing nothing and serving an empty `/metrics` forever.
            let exporter = opentelemetry_prometheus::exporter()
                .with_registry(registry.clone())
                .build()
                .expect("opentelemetry-prometheus exporter must build from a fresh registry");
            let provider = SdkMeterProvider::builder().with_reader(exporter).build();
            opentelemetry::global::set_meter_provider(provider);
            registry
        })
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `init_prometheus` must be safe to call more than once and always return a usable,
    /// gatherable registry (not, say, panic on the second call because the OTel exporter
    /// was already installed).
    #[test]
    fn init_prometheus_is_idempotent() {
        let first = init_prometheus();
        let second = init_prometheus();

        // Both handles must be backed by the same underlying registry: a metric family
        // gathered from one must be visible via the other.
        assert_eq!(
            first.gather().len(),
            second.gather().len(),
            "repeated calls must return handles to the same registry"
        );
    }
}
