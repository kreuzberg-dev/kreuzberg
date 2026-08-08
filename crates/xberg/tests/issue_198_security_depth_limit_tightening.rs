//! Regression test for #198 — the depth budget took the looser of two configured limits.
//!
//! `SecurityBudget::from_limits` built its `DepthValidator` from
//! `max_xml_depth.max(max_nesting_depth)`. Both knobs bound the *same* parse, so taking
//! the maximum meant a caller who tightened one of them got it silently ignored as long
//! as the other stayed at its 1024 default — the exact opposite of what configuring a
//! limit is for. The budget must honour the tighter of the two.
//!
//! `SecurityLimits::default()` sets both to 1024, so this only ever bit callers who
//! deliberately clamped nesting; the default path is unchanged.

#![cfg(feature = "xml")]

mod helpers;

use helpers::extract_bytes_document;
use xberg::SecurityLimits;
use xberg::core::config::ExtractionConfig;

/// Deeper than any limit configured below, so the budget — not the document — decides.
const NESTING_DEPTH: usize = 40;

/// `<n0><n1>…<n39>payload</n39>…</n0>`: one `Start` event per level, so the parser's
/// depth counter reaches exactly `depth`.
fn deeply_nested_xml(depth: usize) -> Vec<u8> {
    let mut xml = String::new();
    for level in 0..depth {
        xml.push_str(&format!("<n{level}>"));
    }
    xml.push_str("payload");
    for level in (0..depth).rev() {
        xml.push_str(&format!("</n{level}>"));
    }
    xml.into_bytes()
}

fn config_with_limits(max_nesting_depth: usize, max_xml_depth: usize) -> ExtractionConfig {
    ExtractionConfig {
        security_limits: Some(SecurityLimits {
            max_nesting_depth,
            max_xml_depth,
            ..SecurityLimits::default()
        }),
        ..ExtractionConfig::default()
    }
}

#[tokio::test]
async fn should_apply_max_nesting_depth_when_it_is_tighter_than_max_xml_depth() {
    let config = config_with_limits(5, 1024);

    let error = extract_bytes_document(&deeply_nested_xml(NESTING_DEPTH), "application/xml", &config)
        .await
        .expect_err("a max_nesting_depth of 5 must stop a 40-level document");

    let message = error.to_string();
    assert!(
        message.contains("Nesting too deep: 6 levels (max: 5)"),
        "the tighter max_nesting_depth must win over the 1024 max_xml_depth, got: {message}"
    );
}

#[tokio::test]
async fn should_apply_max_xml_depth_when_it_is_tighter_than_max_nesting_depth() {
    let config = config_with_limits(1024, 3);

    let error = extract_bytes_document(&deeply_nested_xml(NESTING_DEPTH), "application/xml", &config)
        .await
        .expect_err("a max_xml_depth of 3 must stop a 40-level document");

    let message = error.to_string();
    assert!(
        message.contains("Nesting too deep: 4 levels (max: 3)"),
        "the tighter max_xml_depth must win over the 1024 max_nesting_depth, got: {message}"
    );
}

#[tokio::test]
async fn should_accept_nesting_within_both_configured_depth_limits() {
    let config = config_with_limits(1024, 1024);

    let result = extract_bytes_document(&deeply_nested_xml(NESTING_DEPTH), "application/xml", &config)
        .await
        .expect("40 levels is well inside both limits");

    assert!(
        result.content.contains("payload"),
        "content must survive when no limit is exceeded: {:?}",
        result.content
    );
}

#[tokio::test]
async fn should_leave_the_default_depth_budget_unchanged() {
    let limits = SecurityLimits::default();
    assert_eq!(limits.max_nesting_depth, 1024);
    assert_eq!(limits.max_xml_depth, 1024);

    let result = extract_bytes_document(
        &deeply_nested_xml(NESTING_DEPTH),
        "application/xml",
        &ExtractionConfig::default(),
    )
    .await
    .expect("default limits must keep accepting ordinary nesting");

    assert!(
        result.content.contains("payload"),
        "default extraction must be unaffected by the tightening fix: {:?}",
        result.content
    );
}
