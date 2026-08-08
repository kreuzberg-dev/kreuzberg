#![cfg(feature = "qr-codes")]
//! Regression test for #168: a QR image-decode failure must be observable —
//! previously it only logged at `tracing::debug!`, indistinguishable at
//! default log levels from "ran a clean scan and found nothing".

use std::sync::{Arc, Mutex};

use tracing_subscriber::{EnvFilter, layer::SubscriberExt as _};
use xberg::extractors::qr::detect_qr_codes;

/// A tracing `Layer` that records the level of every emitted event.
#[derive(Clone, Default)]
struct EventCapture {
    levels: Arc<Mutex<Vec<tracing::Level>>>,
}

impl<S> tracing_subscriber::Layer<S> for EventCapture
where
    S: tracing::Subscriber,
{
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: tracing_subscriber::layer::Context<'_, S>) {
        self.levels.lock().unwrap().push(*event.metadata().level());
    }
}

/// Malformed image bytes must surface a `WARN`-level event, not just `DEBUG`.
#[test]
fn image_decode_failure_emits_warn() {
    let capture = EventCapture::default();
    let capture_clone = capture.clone();

    let filter = EnvFilter::new("warn");
    let subscriber = tracing_subscriber::registry().with(filter).with(capture_clone);

    let codes = tracing::subscriber::with_default(subscriber, || detect_qr_codes(&[0, 1, 2, 3, 4], Some("image/png")));

    assert!(codes.is_empty(), "malformed bytes must not yield any QR codes");
    let levels = capture.levels.lock().unwrap();
    assert!(
        levels.contains(&tracing::Level::WARN),
        "expected a WARN-level event for image decode failure, got: {levels:?}"
    );
}
