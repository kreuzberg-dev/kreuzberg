#![allow(clippy::print_stdout, clippy::print_stderr, clippy::dbg_macro)] // ~keep: test binaries print by design; org logging policy exempts tests
//! Regression tests for https://github.com/xberg-io/xberg/issues/1381
//! ("enable full liter-llm configuration" / AWS Bedrock SigV4 signing).
//!
//! These tests exercise the **public** surface only (`xberg::doctor`,
//! `ExtractionConfig`, `LlmConfig`, `BedrockConfig`) — no network calls,
//! no AWS credentials required. They prove three things end-to-end, without
//! reaching into `xberg::llm` internals:
//!
//! 1. `BedrockConfig` is reachable from a config file (TOML) all the way down
//!    through `OcrConfig::vlm_config`.
//! 2. A `bedrock/`-prefixed model is dispatched to the `vlm` OCR backend,
//!    which is only registered when the `liter-llm` feature (which now
//!    compiles in liter-llm's own `bedrock` feature — SigV4 signing) is on.
//! 3. Building the client for a `bedrock/`-prefixed model with no credentials
//!    anywhere fails fast with an actionable error naming the AWS credential
//!    env vars, instead of silently sending an unsigned request.
//!
//! Run with:
//!
//! ```text
//! cargo test -p xberg --features "liter-llm,pdf" --test bedrock_config
//! ```

#![cfg(feature = "liter-llm")]

use serial_test::serial;
use xberg::core::config::{BedrockConfig, ExtractionConfig, LlmConfig, OcrConfig};
use xberg::doctor::ProbeStatus;

const BEDROCK_MODEL: &str = "bedrock/anthropic.claude-3-sonnet-20240229-v1:0";

fn bedrock_ocr_config(bedrock: Option<BedrockConfig>) -> ExtractionConfig {
    ExtractionConfig {
        ocr: Some(OcrConfig {
            enabled: true,
            backend: "vlm".to_string(),
            language: vec!["eng".to_string()],
            vlm_config: Some(LlmConfig {
                model: BEDROCK_MODEL.to_string(),
                bedrock: bedrock.map(Box::new),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    }
}

/// A `bedrock/`-prefixed model with no explicit `BedrockConfig` credentials and
/// no `AWS_ACCESS_KEY_ID` in the environment must be reported by `doctor` as a
/// `Fail`, naming the missing AWS credentials — not silently pass, and not fail
/// with a generic "backend not available" (which would mean the `vlm` backend,
/// and therefore Bedrock SigV4 signing, never got compiled in at all).
///
/// `#[serial]` because the test mutates the process-wide `AWS_ACCESS_KEY_ID`
/// environment variable, matching the convention used by
/// `xberg::llm::client`'s own equivalent unit test.
#[allow(unsafe_code)]
#[serial]
#[test]
fn test_doctor_reports_clear_failure_for_unconfigured_bedrock_model() {
    let original = std::env::var("AWS_ACCESS_KEY_ID").ok();
    // SAFETY: guarded by #[serial] — no other test in this process observes
    // AWS_ACCESS_KEY_ID concurrently while this one runs.
    unsafe {
        std::env::remove_var("AWS_ACCESS_KEY_ID");
    }

    let config = bedrock_ocr_config(None);
    let report = xberg::doctor(&config);

    // SAFETY: see above.
    unsafe {
        match &original {
            Some(val) => std::env::set_var("AWS_ACCESS_KEY_ID", val),
            None => std::env::remove_var("AWS_ACCESS_KEY_ID"),
        }
    }

    let check = report
        .checks
        .iter()
        .find(|c| c.name == "ocr.vlm")
        .unwrap_or_else(|| panic!("expected an 'ocr.vlm' check, got: {:?}", report.checks));

    assert_eq!(
        check.status,
        ProbeStatus::Fail,
        "expected Fail (unconfigured Bedrock), got {:?}: {}",
        check.status,
        check.message
    );
    assert_ne!(
        check.message, "backend not available: no backend registered for 'vlm'",
        "the 'vlm' backend must be registered when the liter-llm feature is on \
         (bedrock signing depends on it): {}",
        check.message
    );
    assert!(
        check.message.contains("AWS credentials") || check.message.contains("AWS_ACCESS_KEY_ID"),
        "error should name the missing AWS credentials, not fail generically: {}",
        check.message
    );
}

/// Explicit `BedrockConfig` credentials (region + static keys) must be enough
/// for `doctor` to accept the client build — proving `region`,
/// `cross_region_prefix`, and the three credential fields all survive the
/// `ExtractionConfig` -> `OcrConfig` -> `LlmConfig` -> `BedrockConfig` ->
/// liter-llm client chain end-to-end.
#[test]
fn test_doctor_passes_bedrock_model_with_explicit_credentials() {
    let config = bedrock_ocr_config(Some(BedrockConfig {
        region: Some("eu-central-1".to_string()),
        cross_region_prefix: Some("eu".to_string()),
        access_key_id: Some("AKIAEXAMPLE".to_string()),
        secret_access_key: Some("example-secret".to_string()),
        session_token: Some("example-token".to_string()),
    }));

    let report = xberg::doctor(&config);

    let check = report
        .checks
        .iter()
        .find(|c| c.name == "ocr.vlm")
        .unwrap_or_else(|| panic!("expected an 'ocr.vlm' check, got: {:?}", report.checks));

    assert_ne!(
        check.status,
        ProbeStatus::Fail,
        "explicit Bedrock credentials should let client construction succeed: {}",
        check.message
    );

    // The fake credentials must never leak into the report, even on success.
    for secret in ["AKIAEXAMPLE", "example-secret", "example-token"] {
        assert!(
            !check.message.contains(secret),
            "doctor report leaked a Bedrock credential: {}",
            check.message
        );
    }
}

/// `LlmConfig.bedrock` must round-trip through TOML exactly the way a user's
/// `xberg.toml` would express it under `[ocr.vlm_config.bedrock]`, all the way
/// through the full `ExtractionConfig` document (not just the `LlmConfig`
/// sub-struct in isolation, which `core::config::llm`'s own unit tests already
/// cover).
#[test]
fn test_extraction_config_toml_carries_bedrock_settings_through_vlm_config() {
    let toml_src = r#"
[ocr]
enabled = true
backend = "vlm"
language = ["eng"]

[ocr.vlm_config]
model = "bedrock/anthropic.claude-3-sonnet-20240229-v1:0"

[ocr.vlm_config.bedrock]
region = "us-east-1"
cross_region_prefix = "us"
access_key_id = "AKIAEXAMPLE"
secret_access_key = "example-secret"
session_token = "example-token"
"#;
    let config: ExtractionConfig = toml::from_str(toml_src).expect("deserialize ExtractionConfig from TOML");

    let ocr = config.ocr.as_ref().expect("ocr config present");
    assert_eq!(ocr.backend, "vlm");
    let vlm_config = ocr.vlm_config.as_ref().expect("vlm_config present");
    assert_eq!(vlm_config.model, BEDROCK_MODEL);

    let bedrock = vlm_config.bedrock.as_ref().expect("bedrock present");
    assert_eq!(bedrock.region.as_deref(), Some("us-east-1"));
    assert_eq!(bedrock.cross_region_prefix.as_deref(), Some("us"));
    assert_eq!(bedrock.access_key_id.as_deref(), Some("AKIAEXAMPLE"));
    assert_eq!(bedrock.secret_access_key.as_deref(), Some("example-secret"));
    assert_eq!(bedrock.session_token.as_deref(), Some("example-token"));
}
