//! Issue #282 — the telemetry layer declared span identities in
//! [`xberg::telemetry::conventions`] that no code path ever emitted, so a consumer that
//! enabled tracing/OTEL saw a strict subset of the documented surface.
//!
//! These tests install a span-recording `tracing_subscriber` layer, drive the real
//! extraction API, and assert on the exact span names and the exact convention field
//! values each span must carry. They deliberately assert on the `conventions::*`
//! constants rather than on hand-written strings, so renaming a constant without
//! updating its emission site fails here.
//!
//! Every span asserted on is gated behind the `otel` feature, matching the gating of
//! `xberg::telemetry::spans`, so this whole file is compiled only under that feature.
#![cfg(feature = "otel")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serial_test::serial;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::{Context, SubscriberExt};
use tracing_subscriber::registry::Registry;

use xberg::plugins::{Plugin, Validator, register_validator, unregister_validator};
use xberg::telemetry::conventions;
use xberg::types::ExtractedDocument;
use xberg::{ExtractInput, ExtractionConfig, Result};

/// One span as it was created, with every field that was populated at creation time.
///
/// Fields declared as `tracing::field::Empty` and never recorded do not appear here,
/// which is what lets the negative tests below distinguish "span emitted" from
/// "span declared".
#[derive(Debug, Clone)]
struct CapturedSpan {
    name: &'static str,
    fields: BTreeMap<String, String>,
}

#[derive(Clone, Default)]
struct SpanCapture {
    spans: Arc<Mutex<Vec<CapturedSpan>>>,
}

impl<S: tracing::Subscriber> Layer<S> for SpanCapture {
    fn on_new_span(&self, attributes: &tracing::span::Attributes<'_>, _id: &tracing::Id, _context: Context<'_, S>) {
        let mut fields = BTreeMap::new();
        attributes.record(&mut FieldVisitor(&mut fields));
        self.spans.lock().expect("span buffer poisoned").push(CapturedSpan {
            name: attributes.metadata().name(),
            fields,
        });
    }
}

struct FieldVisitor<'a>(&'a mut BTreeMap<String, String>);

impl tracing::field::Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.to_owned());
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.to_string());
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.insert(field.name().to_owned(), format!("{value:?}"));
    }
}

/// Run `body` with a span-recording subscriber installed on this thread and return
/// every span created while it ran.
fn capture_spans<T>(body: impl FnOnce() -> T) -> (T, Vec<CapturedSpan>) {
    let capture = SpanCapture::default();
    let spans = Arc::clone(&capture.spans);
    let subscriber = Registry::default().with(capture);

    // `with_default` installs a *thread-local* subscriber, but tracing's callsite
    // interest cache is process-global: a callsite first evaluated while no subscriber
    // was installed keeps a cached "not interested" verdict and its span is then never
    // created at all. Forcing a rebuild on both sides makes the capture depend only on
    // the subscriber installed here (same hazard as `cache::tests::capture_logs`, #301).
    tracing::callsite::rebuild_interest_cache();
    let value = tracing::subscriber::with_default(subscriber, body);
    tracing::callsite::rebuild_interest_cache();

    let captured = spans.lock().expect("span buffer poisoned").clone();
    (value, captured)
}

/// Drive an async extraction to completion on the current thread, so every span it
/// creates is observed by the thread-local subscriber installed by [`capture_spans`].
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .expect("current-thread runtime must build")
        .block_on(future)
}

/// All spans named `name` whose field `field` holds exactly `value`.
fn spans_matching<'a>(spans: &'a [CapturedSpan], name: &str, field: &str, value: &str) -> Vec<&'a CapturedSpan> {
    spans
        .iter()
        .filter(|span| span.name == name && span.fields.get(field).map(String::as_str) == Some(value))
        .collect()
}

/// The first span named `name` carrying `field=value`, or a failure naming every span
/// that was actually captured.
fn require_span<'a>(spans: &'a [CapturedSpan], name: &str, field: &str, value: &str) -> &'a CapturedSpan {
    let matches = spans_matching(spans, name, field, value);
    assert!(
        !matches.is_empty(),
        "expected a `{name}` span with `{field}={value}`; captured spans were:\n{}",
        describe(spans)
    );
    matches[0]
}

fn count_spans(spans: &[CapturedSpan], name: &str, field: &str, value: &str) -> usize {
    spans_matching(spans, name, field, value).len()
}

fn describe(spans: &[CapturedSpan]) -> String {
    spans
        .iter()
        .map(|span| format!("  {}{:?}", span.name, span.fields))
        .collect::<Vec<_>>()
        .join("\n")
}

fn plain_text_input(body: &str) -> ExtractInput {
    ExtractInput::from_bytes(body.as_bytes().to_vec(), "text/plain", None)
}

/// Caching is disabled everywhere below: a cache hit short-circuits extraction and the
/// pipeline, which is exactly the code these tests assert instrumentation on.
fn uncached_config() -> ExtractionConfig {
    ExtractionConfig {
        use_cache: false,
        ..Default::default()
    }
}

fn extract_plain_text(config: &ExtractionConfig) -> Vec<CapturedSpan> {
    let (output, spans) = capture_spans(|| block_on(xberg::extract(plain_text_input("hello telemetry"), config)));
    let output = output.expect("plain-text extraction must succeed");
    assert_eq!(output.results.len(), 1, "expected exactly one extraction result");
    spans
}

#[test]
#[serial]
fn extraction_stage_span_should_wrap_the_selected_extractor() {
    let spans = extract_plain_text(&uncached_config());

    assert_eq!(
        count_spans(
            &spans,
            "xberg.pipeline",
            conventions::PIPELINE_STAGE,
            conventions::stages::EXTRACTION
        ),
        1,
        "exactly one extractor runs for a single document; captured spans were:\n{}",
        describe(&spans)
    );

    let stage = require_span(
        &spans,
        "xberg.pipeline",
        conventions::PIPELINE_STAGE,
        conventions::stages::EXTRACTION,
    );

    assert_eq!(
        stage.fields.get(conventions::EXTRACTOR_NAME).map(String::as_str),
        Some("plain-text-extractor"),
        "the extraction stage span must name the extractor that ran; span was {:?}",
        stage.fields
    );
    assert_eq!(
        stage.fields.get(conventions::EXTRACTOR_PRIORITY).map(String::as_str),
        Some("50"),
        "the extraction stage span must record the selected extractor's priority; span was {:?}",
        stage.fields
    );
}

#[test]
#[serial]
fn run_pipeline_span_should_carry_the_pipeline_operation() {
    let spans = extract_plain_text(&uncached_config());

    assert_eq!(
        count_spans(
            &spans,
            "run_pipeline",
            conventions::OPERATION,
            conventions::operations::PIPELINE
        ),
        1,
        "the pipeline runs exactly once per document; captured spans were:\n{}",
        describe(&spans)
    );

    let pipeline = require_span(
        &spans,
        "run_pipeline",
        conventions::OPERATION,
        conventions::operations::PIPELINE,
    );

    assert!(
        pipeline.fields.contains_key("content.element_count"),
        "the pipeline span must keep its element-count field; span was {:?}",
        pipeline.fields
    );
}

#[test]
#[serial]
fn batch_extraction_should_emit_a_batch_span_and_one_item_span_per_input() {
    let config = uncached_config();
    let inputs = vec![plain_text_input("first document"), plain_text_input("second document")];

    let (output, spans) = capture_spans(|| block_on(xberg::extract_batch(inputs, &config)));
    let output = output.expect("batch extraction must succeed");
    assert_eq!(output.results.len(), 2, "expected two extraction results");

    let batch = require_span(
        &spans,
        "xberg.batch",
        conventions::OPERATION,
        conventions::operations::BATCH_EXTRACT,
    );
    assert_eq!(
        batch.fields.get(conventions::BATCH_SIZE).map(String::as_str),
        Some("2"),
        "the batch span must record the number of inputs; span was {:?}",
        batch.fields
    );

    for index in ["0", "1"] {
        let item = require_span(&spans, "xberg.batch.item", conventions::BATCH_INDEX, index);
        assert_eq!(
            item.fields.get(conventions::OPERATION).map(String::as_str),
            Some(conventions::operations::BATCH_EXTRACT),
            "each batch item span must be labelled as part of a batch extraction; span was {:?}",
            item.fields
        );
    }

    assert_eq!(
        spans.iter().filter(|span| span.name == "xberg.batch.item").count(),
        2,
        "expected exactly one item span per batch input; captured spans were:\n{}",
        describe(&spans)
    );
}

#[cfg(feature = "chunking")]
#[test]
#[serial]
fn chunking_stage_span_should_be_emitted_when_chunking_is_configured() {
    let config = ExtractionConfig {
        chunking: Some(xberg::ChunkingConfig {
            max_characters: 8,
            overlap: 0,
            ..Default::default()
        }),
        ..uncached_config()
    };

    let spans = extract_plain_text(&config);

    // `run_pipeline` chunks twice on purpose — a provisional pass so Middle/Late
    // post-processors see `result.chunks`, then a corrective pass once `content` stops
    // changing (#213) — so both passes are instrumented.
    assert_eq!(
        count_spans(
            &spans,
            "xberg.pipeline",
            conventions::PIPELINE_STAGE,
            conventions::stages::CHUNKING
        ),
        2,
        "both chunking passes must be instrumented; captured spans were:\n{}",
        describe(&spans)
    );
}

#[cfg(feature = "language-detection")]
#[test]
#[serial]
fn language_detection_stage_span_should_be_emitted_when_configured() {
    let config = ExtractionConfig {
        language_detection: Some(xberg::LanguageDetectionConfig {
            enabled: true,
            min_confidence: 0.0,
            detect_multiple: false,
        }),
        ..uncached_config()
    };

    let spans = extract_plain_text(&config);

    assert_eq!(
        count_spans(
            &spans,
            "xberg.pipeline",
            conventions::PIPELINE_STAGE,
            conventions::stages::LANGUAGE_DETECTION
        ),
        1,
        "language detection runs once per pipeline; captured spans were:\n{}",
        describe(&spans)
    );
}

#[cfg(feature = "quality")]
#[test]
#[serial]
fn token_reduction_stage_span_should_be_emitted_when_configured() {
    let config = ExtractionConfig {
        token_reduction: Some(xberg::TokenReductionOptions {
            mode: "moderate".to_string(),
            preserve_important_words: true,
        }),
        ..uncached_config()
    };

    let spans = extract_plain_text(&config);

    assert_eq!(
        count_spans(
            &spans,
            "xberg.pipeline",
            conventions::PIPELINE_STAGE,
            conventions::stages::TOKEN_REDUCTION
        ),
        1,
        "token reduction runs once per pipeline; captured spans were:\n{}",
        describe(&spans)
    );
}

/// A validator that accepts everything — it exists only so the validation stage has
/// work to do, since the stage span is deliberately not emitted for an empty registry.
#[derive(Debug)]
struct AcceptEverythingValidator;

const ACCEPT_EVERYTHING_VALIDATOR: &str = "issue-282-accept-everything-validator";

impl Plugin for AcceptEverythingValidator {
    fn name(&self) -> &str {
        ACCEPT_EVERYTHING_VALIDATOR
    }

    fn version(&self) -> String {
        "1.0.0".to_string()
    }

    fn initialize(&self) -> Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
impl Validator for AcceptEverythingValidator {
    async fn validate(&self, _result: &ExtractedDocument, _config: &ExtractionConfig) -> Result<()> {
        Ok(())
    }
}

#[test]
#[serial]
fn validation_stage_span_should_be_emitted_when_a_validator_runs() {
    register_validator(Arc::new(AcceptEverythingValidator)).expect("validator registration must succeed");

    let spans = extract_plain_text(&uncached_config());

    unregister_validator(ACCEPT_EVERYTHING_VALIDATOR).expect("validator cleanup must succeed");

    assert_eq!(
        count_spans(
            &spans,
            "xberg.pipeline",
            conventions::PIPELINE_STAGE,
            conventions::stages::VALIDATION
        ),
        1,
        "the validation stage runs once per pipeline; captured spans were:\n{}",
        describe(&spans)
    );
}

#[test]
#[serial]
fn stage_spans_should_not_be_emitted_for_stages_that_did_not_run() {
    // Default config configures neither chunking, language detection nor token
    // reduction, and no validator is registered. A span emitted here would mean the
    // stage span was hung on the function rather than on the work it names.
    let spans = extract_plain_text(&uncached_config());

    for stage in [
        conventions::stages::CHUNKING,
        conventions::stages::LANGUAGE_DETECTION,
        conventions::stages::TOKEN_REDUCTION,
        conventions::stages::VALIDATION,
    ] {
        assert!(
            spans_matching(&spans, "xberg.pipeline", conventions::PIPELINE_STAGE, stage).is_empty(),
            "`{stage}` must not be reported when the stage did not run; captured spans were:\n{}",
            describe(&spans)
        );
    }
}
