//! LLM client factory — converts xberg's LlmConfig to a liter-llm DefaultClient.

use liter_llm::client::{ClientConfig, DefaultClient};

use crate::core::config::{
    BedrockConfig, LlmBudgetConfig, LlmCacheConfig, LlmConfig, LlmProviderConfig, LlmRateLimitConfig,
};

/// Translate xberg's [`BedrockConfig`] into liter-llm's own (unboxed) `client::BedrockConfig`.
fn to_liter_llm_bedrock(bedrock: &BedrockConfig) -> liter_llm::client::BedrockConfig {
    liter_llm::client::BedrockConfig {
        region: bedrock.region.clone(),
        cross_region_prefix: bedrock.cross_region_prefix.clone(),
        access_key_id: bedrock.access_key_id.clone(),
        secret_access_key: bedrock.secret_access_key.clone(),
        session_token: bedrock.session_token.clone(),
    }
}

/// Translate xberg's [`LlmProviderConfig`] into liter-llm's own `client::LlmProviderConfig`.
fn to_liter_llm_provider(provider: &LlmProviderConfig) -> liter_llm::client::LlmProviderConfig {
    liter_llm::client::LlmProviderConfig {
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        auth_header: provider.auth_header.clone(),
        model_prefixes: provider.model_prefixes.clone(),
    }
}

/// Convert xberg's flat `auth_header` header-name-or-default into liter-llm's richer
/// [`liter_llm::AuthHeaderFormat`].
///
/// xberg's [`LlmProviderConfig::auth_header`] is a single optional header *name*,
/// documented as "defaults to `Authorization` when unset" — it can only express
/// liter-llm's `Bearer` scheme or a raw named header, never the explicit
/// "no authentication" mode `AuthHeaderFormat::None` supports; that mode is
/// unreachable through this mapping (a future field would be needed to expose it).
/// The honest mapping for what xberg *can* express:
/// - `None`, or `Some("Authorization")` (case-insensitive) -> [`liter_llm::AuthHeaderFormat::Bearer`]
///   (`Authorization: Bearer <key>`), matching xberg's documented default.
/// - `Some(name)` for any other header name -> [`liter_llm::AuthHeaderFormat::ApiKey`]
///   (`<name>: <key>`, the raw key with no scheme prefix).
fn to_auth_header_format(auth_header: Option<&str>) -> liter_llm::AuthHeaderFormat {
    match auth_header {
        None => liter_llm::AuthHeaderFormat::Bearer,
        Some(name) if name.eq_ignore_ascii_case("authorization") => liter_llm::AuthHeaderFormat::Bearer,
        Some(name) => liter_llm::AuthHeaderFormat::ApiKey(name.to_string()),
    }
}

/// Register every configured custom provider ([`LlmConfig::providers`]) in liter-llm's
/// process-global custom-provider registry ([`liter_llm::register_custom_provider`]).
///
/// liter-llm's `into_client_builder` never reads `providers` — there is no equivalent
/// field on `ClientConfig` at all (see [`to_liter_llm_config`]). Custom providers only
/// take effect once registered here; without this call, a `providers` entry in config
/// is silently a no-op — shipping a config key that does nothing is worse than a clear
/// error, so registration failures are surfaced as [`crate::XbergError::Validation`]
/// rather than swallowed.
///
/// # Idempotency and cross-config collisions
///
/// The registry is process-global and keyed by provider `name`:
/// `register_custom_provider` replaces an existing entry with the same name rather than
/// erroring. Calling this repeatedly with the *same* [`LlmConfig`] is therefore
/// idempotent — it re-registers identical data every time a client is built from that
/// config. Calling it with *different* [`LlmConfig`]s that both define a provider under
/// the same `name` is **not** conflict-free: the most recently built client's
/// registration wins for every model lookup process-wide, including requests already in
/// flight on a client built from the other config. liter-llm exposes no read API to
/// detect this collision before overwriting (`detect_custom_provider` is crate-private
/// to liter-llm), so it cannot be caught here — use distinct provider names across every
/// `LlmConfig` used within one process to avoid it.
fn register_configured_providers(config: &LlmConfig) -> crate::Result<()> {
    let Some(providers) = config.providers.as_ref() else {
        return Ok(());
    };

    for provider in providers {
        let custom = liter_llm::CustomProviderConfig {
            name: provider.name.clone(),
            base_url: provider.base_url.clone(),
            auth_header: to_auth_header_format(provider.auth_header.as_deref()),
            model_prefixes: provider.model_prefixes.clone(),
        };

        liter_llm::register_custom_provider(custom).map_err(|e| {
            let msg = format!("Failed to register custom LLM provider '{}': {e}", provider.name);
            crate::XbergError::Validation {
                message: msg,
                source: Some(Box::new(e)),
            }
        })?;
    }

    Ok(())
}

/// Parse xberg's plain-string [`LlmConfig::reasoning_effort`] into liter-llm's
/// [`liter_llm::ReasoningEffort`] request-time enum.
///
/// liter-llm serializes [`liter_llm::ReasoningEffort`] with
/// `#[serde(rename_all = "lowercase")]`; this matches the same five spellings
/// (`"low"`, `"medium"`, `"high"`, `"minimal"`, `"max"`) case-insensitively. An
/// unrecognized value is a [`crate::XbergError::Validation`] rather than a silent
/// drop, matching the "a clear error is better than silence" principle used for
/// `providers` above — a request built from a mistyped value (e.g. `"maximum"`)
/// should fail loudly instead of quietly going out without the hint.
///
/// Called from every site that builds a [`liter_llm::ChatCompletionRequest`] from an
/// [`LlmConfig`], alongside the `temperature`/`max_tokens` lines: `llm::text_completion`,
/// `llm::structured` (both request builders), `llm::vlm_ocr` and `text::ner::llm`. A new
/// request builder must wire this and `config.extra_body` too, or those two config fields
/// silently do nothing on that path.
pub(crate) fn parse_reasoning_effort(config: &LlmConfig) -> crate::Result<Option<liter_llm::ReasoningEffort>> {
    let Some(value) = config.reasoning_effort.as_deref() else {
        return Ok(None);
    };

    match value.to_ascii_lowercase().as_str() {
        "low" => Ok(Some(liter_llm::ReasoningEffort::Low)),
        "medium" => Ok(Some(liter_llm::ReasoningEffort::Medium)),
        "high" => Ok(Some(liter_llm::ReasoningEffort::High)),
        "minimal" => Ok(Some(liter_llm::ReasoningEffort::Minimal)),
        "max" => Ok(Some(liter_llm::ReasoningEffort::Max)),
        _ => Err(crate::XbergError::Validation {
            message: format!(
                "Invalid LLM reasoning_effort '{value}': expected one of \"low\", \"medium\", \"high\", \
                 \"minimal\", \"max\""
            ),
            source: None,
        }),
    }
}

/// Translate xberg's [`LlmCacheConfig`] into liter-llm's own `client::LlmCacheConfig`.
fn to_liter_llm_cache(cache: &LlmCacheConfig) -> liter_llm::client::LlmCacheConfig {
    liter_llm::client::LlmCacheConfig {
        max_entries: cache.max_entries,
        ttl_seconds: cache.ttl_seconds,
        backend: cache.backend.clone(),
        backend_config: cache.backend_config.clone(),
    }
}

/// Translate xberg's [`LlmBudgetConfig`] into liter-llm's own `client::LlmBudgetConfig`.
fn to_liter_llm_budget(budget: &LlmBudgetConfig) -> liter_llm::client::LlmBudgetConfig {
    liter_llm::client::LlmBudgetConfig {
        global_limit: budget.global_limit,
        model_limits: budget.model_limits.clone(),
        enforcement: budget.enforcement.clone(),
    }
}

/// Translate xberg's [`LlmRateLimitConfig`] into liter-llm's own `client::LlmRateLimitConfig`.
fn to_liter_llm_rate_limit(rate_limit: &LlmRateLimitConfig) -> liter_llm::client::LlmRateLimitConfig {
    liter_llm::client::LlmRateLimitConfig {
        rpm: rate_limit.rpm,
        tpm: rate_limit.tpm,
        window_seconds: rate_limit.window_seconds,
    }
}

/// Translate xberg's [`LlmConfig`] into liter-llm's own canonical `client::LlmConfig`,
/// field for field, so the two never drift silently: every field the two types share
/// (by name and type, by construction — see the `LlmConfig` module doc) is copied
/// here without any per-field re-derivation of liter-llm's own mapping semantics.
///
/// Two deliberate exceptions:
/// - `headers` is left `None`. liter-llm's `into_client_builder` silently drops any
///   header that fails to parse as an HTTP header name/value pair, while xberg's
///   [`build_client_config`] validates headers explicitly and returns a
///   [`crate::XbergError::Validation`] naming the offending header — so headers are
///   applied separately, after conversion, on the builder this function's caller
///   gets back from `into_client_builder()`.
/// - `base_url` is trimmed of a trailing slash before being handed to liter-llm,
///   which does not perform this normalization itself; without it a base URL like
///   `"https://api.openai.com/v1/"` would join with a request path into a doubled
///   `//`.
///
/// `providers` is copied below purely for a lossless DTO-to-DTO conversion (see the
/// dedicated test); liter-llm's own `into_client_builder` never reads it back off the
/// value returned here — [`build_client_config`] instead registers each entry directly
/// from xberg's [`LlmConfig::providers`] via [`register_configured_providers`], which is
/// the mapping that actually takes effect.
fn to_liter_llm_config(config: &LlmConfig) -> liter_llm::client::LlmConfig {
    liter_llm::client::LlmConfig {
        model: config.model.clone(),
        api_key: config.api_key.clone(),
        base_url: config
            .base_url
            .as_deref()
            .map(|url| url.trim_end_matches('/').to_string()),
        timeout_secs: config.timeout_secs,
        max_retries: config.max_retries,
        temperature: config.temperature,
        max_tokens: config.max_tokens,
        load_env: config.load_env,
        headers: None,
        providers: config
            .providers
            .as_ref()
            .map(|providers| providers.iter().map(to_liter_llm_provider).collect()),
        cache: config.cache.as_deref().map(to_liter_llm_cache),
        budget: config.budget.as_deref().map(to_liter_llm_budget),
        rate_limit: config.rate_limit.as_deref().map(to_liter_llm_rate_limit),
        cost_tracking: config.cost_tracking,
        tracing: config.tracing,
        cooldown_secs: config.cooldown_secs,
        health_check_secs: config.health_check_secs,
        bedrock: config.bedrock.as_deref().map(to_liter_llm_bedrock),
    }
}

/// Translate xberg's [`LlmConfig`] into a liter-llm [`ClientConfig`].
///
/// `model`, `temperature`, and `max_tokens` are request-time parameters, not
/// client-level settings; they are deliberately carried on [`LlmConfig`] without
/// being mapped here, matching liter-llm's own `LlmConfig::into_client_builder`.
///
/// The actual field-by-field wiring (`base_url`, `timeout_secs`, `max_retries`,
/// `load_env`, `cache`, `budget`, `rate_limit`, `cost_tracking`, `tracing`,
/// `cooldown_secs`, `health_check_secs`, `bedrock`) is delegated to liter-llm's
/// own [`liter_llm::client::LlmConfig::into_client_builder`] via
/// [`to_liter_llm_config`], rather than re-implemented here, so this function
/// cannot drift from liter-llm's own mapping as new fields are added upstream.
/// `headers` is applied separately (see [`to_liter_llm_config`]). `providers` has no
/// equivalent on [`ClientConfig`] at all; it is instead registered as a side effect
/// via [`register_configured_providers`] — see that function's doc for the
/// process-global registry semantics this implies.
///
/// Split out of [`create_client`] so the mapping is observable in tests without
/// constructing a live HTTP client.
fn build_client_config(config: &LlmConfig) -> crate::Result<ClientConfig> {
    register_configured_providers(config)?;

    let mut builder = to_liter_llm_config(config).into_client_builder();

    if let Some(ref headers) = config.headers {
        for (key, value) in headers {
            builder = builder.header(key.as_str(), value.as_str()).map_err(|e| {
                let msg = format!("Invalid LLM header '{key}': {e}");
                crate::XbergError::Validation {
                    message: msg,
                    source: Some(Box::new(e)),
                }
            })?;
        }
    }

    Ok(builder.build())
}

/// Create a liter-llm [`DefaultClient`] from xberg's [`LlmConfig`].
///
/// The `model` field from the config is passed as a model hint so that
/// liter-llm can resolve the correct provider automatically.
///
/// When `api_key` is `None`, liter-llm falls back to the provider's standard
/// environment variable (e.g., `OPENAI_API_KEY`).
///
/// For a `bedrock/`-prefixed model, liter-llm's own provider validation runs
/// here and rejects the request up front — with a message naming the required
/// AWS credentials and how to supply them — when neither explicit
/// [`BedrockConfig`](crate::core::config::BedrockConfig) credentials nor
/// `AWS_ACCESS_KEY_ID` in the environment are available. That upstream message
/// is wrapped with the model that failed to build, so the resulting error names
/// the operation (building the LLM client), the input (the model string), and
/// (via the wrapped source) a concrete suggestion.
pub(crate) fn create_client(config: &LlmConfig) -> crate::Result<DefaultClient> {
    let client_config = build_client_config(config)?;

    DefaultClient::new(client_config, Some(&config.model)).map_err(|e| {
        let msg = format!("Failed to build LLM client for model '{}': {e}", config.model);
        crate::XbergError::Validation {
            message: msg,
            source: Some(Box::new(e)),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::{BedrockConfig, LlmConfig};

    #[cfg(feature = "api")]
    #[tokio::test]
    async fn test_client_path_normalization_with_base_url() {
        use axum::{Router, routing::post};
        use liter_llm::LlmClient;
        use tokio::sync::mpsc;

        let (tx, mut rx) = mpsc::unbounded_channel::<String>();

        let app = Router::new().fallback(post(
            move |_method: axum::http::Method, uri: axum::http::Uri, headers: axum::http::HeaderMap| async move {
                assert_eq!(uri.path(), "/v1/chat/completions");

                let auth = headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("none")
                    .to_string();
                let _ = tx.send(auth);

                axum::response::Json(serde_json::json!({
                    "id": "test",
                    "object": "chat.completion",
                    "created": 12345,
                    "model": "test",
                    "choices": [{
                        "index": 0,
                        "message": { "role": "assistant", "content": "{\"foo\": \"bar\"}" },
                        "finish_reason": "stop"
                    }]
                }))
            },
        ));

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let base_url = format!("http://{}/v1/", addr);
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-api-key".to_string()),
            base_url: Some(base_url),
            ..LlmConfig::default()
        };

        let client = create_client(&config).unwrap();

        let request = liter_llm::ChatCompletionRequest {
            model: config.model.clone(),
            messages: vec![liter_llm::Message::User(liter_llm::UserMessage {
                content: liter_llm::UserContent::Text("test".to_string()),
                ..Default::default()
            })],
            ..Default::default()
        };

        let _ = client.chat(request).await.expect("Request failed");

        let auth_header = tokio::time::timeout(tokio::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("Timeout waiting for header")
            .expect("No header received");

        assert_eq!(auth_header, "Bearer test-api-key");
    }

    #[test]
    fn test_create_client_sanitizes_base_url() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            base_url: Some("https://api.openai.com/v1/".to_string()),
            ..LlmConfig::default()
        };

        let _ = create_client(&config).unwrap();
    }

    #[test]
    fn test_create_client_applies_load_env_and_valid_headers() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Gateway-Key".to_string(), "secret123".to_string());
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            load_env: Some(true),
            headers: Some(headers),
            ..LlmConfig::default()
        };

        assert!(
            create_client(&config).is_ok(),
            "valid load_env + headers should build a client"
        );
    }

    #[test]
    fn test_create_client_rejects_invalid_header() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Bad\r\nInjected".to_string(), "value".to_string());
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            headers: Some(headers),
            ..LlmConfig::default()
        };

        match create_client(&config) {
            Err(crate::XbergError::Validation { message, .. }) => {
                assert!(message.contains("Invalid LLM header"), "unexpected message: {message}");
            }
            Err(other) => panic!("expected a Validation error, got: {other}"),
            Ok(_) => panic!("expected create_client to reject the invalid header"),
        }
    }

    fn bedrock_model_config(bedrock: BedrockConfig) -> LlmConfig {
        LlmConfig {
            model: "bedrock/anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
            bedrock: Some(Box::new(bedrock)),
            ..LlmConfig::default()
        }
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// Region and cross-region prefix must reach the liter-llm client config —
    /// before this they were unreachable because `LlmConfig` had no `bedrock` field.
    #[test]
    fn test_build_client_config_maps_bedrock_region_and_cross_region_prefix() {
        let config = bedrock_model_config(BedrockConfig {
            region: Some("eu-central-1".to_string()),
            cross_region_prefix: Some("eu".to_string()),
            ..BedrockConfig::default()
        });

        let client_config = build_client_config(&config).expect("build client config");

        assert_eq!(client_config.bedrock_region.as_deref(), Some("eu-central-1"));
        assert_eq!(client_config.bedrock_cross_region_prefix.as_deref(), Some("eu"));
        assert_eq!(client_config.bedrock_access_key_id, None);
        assert_eq!(client_config.bedrock_secret_access_key, None);
        assert_eq!(client_config.bedrock_session_token, None);
    }

    /// Explicit static credentials must reach the client config verbatim.
    #[test]
    fn test_build_client_config_maps_bedrock_credentials() {
        let config = bedrock_model_config(BedrockConfig {
            region: Some("us-east-1".to_string()),
            access_key_id: Some("AKIAEXAMPLE".to_string()),
            secret_access_key: Some("example-secret".to_string()),
            session_token: Some("example-token".to_string()),
            ..BedrockConfig::default()
        });

        let client_config = build_client_config(&config).expect("build client config");

        assert_eq!(client_config.bedrock_region.as_deref(), Some("us-east-1"));
        assert_eq!(client_config.bedrock_access_key_id.as_deref(), Some("AKIAEXAMPLE"));
        assert_eq!(
            client_config.bedrock_secret_access_key.as_deref(),
            Some("example-secret")
        );
        assert_eq!(client_config.bedrock_session_token.as_deref(), Some("example-token"));
    }

    /// Session token is optional: long-lived key pairs must map without one.
    #[test]
    fn test_build_client_config_maps_bedrock_credentials_without_session_token() {
        let config = bedrock_model_config(BedrockConfig {
            access_key_id: Some("AKIAEXAMPLE".to_string()),
            secret_access_key: Some("example-secret".to_string()),
            ..BedrockConfig::default()
        });

        let client_config = build_client_config(&config).expect("build client config");

        assert_eq!(client_config.bedrock_access_key_id.as_deref(), Some("AKIAEXAMPLE"));
        assert_eq!(
            client_config.bedrock_secret_access_key.as_deref(),
            Some("example-secret")
        );
        assert_eq!(client_config.bedrock_session_token, None);
    }

    /// With no `bedrock` block, every Bedrock slot must stay `None` so liter-llm
    /// resolves region and credentials from the AWS environment / default chain.
    #[test]
    fn test_build_client_config_leaves_bedrock_unset_when_absent() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");

        assert_eq!(client_config.bedrock_region, None);
        assert_eq!(client_config.bedrock_cross_region_prefix, None);
        assert_eq!(client_config.bedrock_access_key_id, None);
        assert_eq!(client_config.bedrock_secret_access_key, None);
        assert_eq!(client_config.bedrock_session_token, None);
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// A `bedrock/`-prefixed model with no explicit `BedrockConfig` credentials
    /// and no `AWS_ACCESS_KEY_ID` in the environment must fail fast at
    /// `create_client` time with a clear, actionable error instead of a bare
    /// "failed to build client" message or a confusing runtime request failure.
    /// `#[serial]` because the test mutates the process-wide `AWS_ACCESS_KEY_ID`
    /// environment variable, matching the save/restore convention used by the
    /// other env-mutating tests in this crate (see
    /// `core::server_config::tests::env_tests`).
    ///
    /// `#[allow(unsafe_code)]` is scoped to this one function rather than the module:
    /// the crate sets `#![deny(unsafe_code)]` (lib.rs:36), and `std::env::set_var` /
    /// `remove_var` are `unsafe` as of edition 2024. `core::server_config::tests::env_tests`
    /// takes the same exemption, but as a file-level `#![allow]` — it can, because it is a
    /// dedicated test file. These tests sit inline in a production module, so a
    /// module-scoped allow would also cover `create_client` itself. ~keep
    #[allow(unsafe_code)]
    #[serial_test::serial]
    #[test]
    fn test_create_client_reports_clear_error_for_unconfigured_bedrock() {
        let original = std::env::var("AWS_ACCESS_KEY_ID").ok();
        // SAFETY: guarded by #[serial_test::serial] — no other test in this
        // process observes AWS_ACCESS_KEY_ID concurrently while this one runs.
        unsafe {
            std::env::remove_var("AWS_ACCESS_KEY_ID");
        }

        let config = bedrock_model_config(BedrockConfig::default());
        let result = create_client(&config);

        // SAFETY: see above.
        unsafe {
            match &original {
                Some(val) => std::env::set_var("AWS_ACCESS_KEY_ID", val),
                None => std::env::remove_var("AWS_ACCESS_KEY_ID"),
            }
        }

        match result {
            Err(crate::XbergError::Validation { message, .. }) => {
                assert!(
                    message.contains("bedrock/anthropic.claude-3-sonnet-20240229-v1:0"),
                    "error should name the model (the input) that failed to build: {message}"
                );
                assert!(
                    message.contains("AWS credentials"),
                    "error should name the root cause (missing AWS credentials): {message}"
                );
                assert!(
                    message.contains("AWS_ACCESS_KEY_ID") && message.contains("AWS_SECRET_ACCESS_KEY"),
                    "error should suggest how to fix it (set explicit config or the AWS env vars): {message}"
                );
            }
            Err(other) => panic!("expected a Validation error, got: {other}"),
            Ok(_) => panic!("expected create_client to reject an unconfigured bedrock model"),
        }
    }

    /// Once Bedrock credentials actually reach liter-llm's own `ClientConfig`
    /// (built by `build_client_config`), that type's `Debug` impl — not xberg's
    /// `LlmConfig`/`BedrockConfig` impls — is the last line of defense against a
    /// credential reaching a log via an accidental `tracing::debug!("{config:?}")`
    /// or panic dump. This proves the credential survives the xberg -> liter-llm
    /// boundary redacted, using real-looking (but fake) values.
    #[test]
    fn test_build_client_config_debug_never_leaks_bedrock_credentials() {
        let config = bedrock_model_config(BedrockConfig {
            region: Some("us-east-1".to_string()),
            access_key_id: Some("AKIAEXAMPLEFAKE".to_string()),
            secret_access_key: Some("example-fake-secret".to_string()),
            session_token: Some("example-fake-token".to_string()),
            ..BedrockConfig::default()
        });

        let client_config = build_client_config(&config).expect("build client config");
        let rendered = format!("{client_config:?}");

        for secret in ["AKIAEXAMPLEFAKE", "example-fake-secret", "example-fake-token"] {
            assert!(
                !rendered.contains(secret),
                "liter-llm ClientConfig Debug leaked {secret}: {rendered}"
            );
        }
        assert!(
            rendered.contains("us-east-1"),
            "non-secret region should stay visible for diagnosability: {rendered}"
        );
    }

    /// A misconfigured request (invalid header) on a model that also carries
    /// live-looking Bedrock credentials must never leak those credentials into
    /// the resulting error — the error's `Display` output is exactly the string
    /// that reaches xberg's logs and callers, so this is the real "does a
    /// credential reach a log" boundary (as opposed to the config-file
    /// persistence `Serialize` impl, which legitimately carries a credential the
    /// caller explicitly set, the same way `api_key` already does).
    #[test]
    fn test_create_client_error_never_leaks_bedrock_credentials() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Bad\r\nInjected".to_string(), "value".to_string());
        let mut config = bedrock_model_config(BedrockConfig {
            region: Some("us-east-1".to_string()),
            access_key_id: Some("AKIAEXAMPLEFAKE".to_string()),
            secret_access_key: Some("example-fake-secret".to_string()),
            session_token: Some("example-fake-token".to_string()),
            ..BedrockConfig::default()
        });
        config.headers = Some(headers);

        // Not `expect_err`: that needs `T: Debug` and liter-llm's `DefaultClient` does not
        // implement it, so the success arm has to be discarded by hand.
        let Err(err) = create_client(&config) else {
            panic!("invalid header must reject the request");
        };
        let rendered = format!("{err}");

        for secret in ["AKIAEXAMPLEFAKE", "example-fake-secret", "example-fake-token"] {
            assert!(
                !rendered.contains(secret),
                "create_client error leaked {secret}: {rendered}"
            );
        }
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// The response cache configuration must reach liter-llm's `ClientConfig`
    /// unchanged, via the delegated `into_client_builder` mapping.
    #[test]
    fn test_build_client_config_maps_cache_settings() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            cache: Some(Box::new(LlmCacheConfig {
                max_entries: Some(128),
                ttl_seconds: Some(90),
                backend: Some("memory".to_string()),
                backend_config: None,
            })),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");
        let cache = client_config.cache_config.expect("cache_config present");

        assert_eq!(cache.max_entries, 128);
        assert_eq!(cache.ttl, std::time::Duration::from_secs(90));
        assert!(matches!(cache.backend, liter_llm::tower::CacheBackend::Memory));
    }

    /// Without the `opendal-cache` liter-llm feature, a non-memory cache backend
    /// name gracefully degrades to the in-memory backend instead of failing —
    /// matching liter-llm's own `into_client_builder` fallback exactly.
    #[test]
    fn test_build_client_config_cache_backend_falls_back_to_memory_without_opendal_cache() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            cache: Some(Box::new(LlmCacheConfig {
                backend: Some("s3".to_string()),
                ..LlmCacheConfig::default()
            })),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");
        let cache = client_config.cache_config.expect("cache_config present");

        assert!(matches!(cache.backend, liter_llm::tower::CacheBackend::Memory));
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// Budget limits, per-model limits, and enforcement mode must all reach
    /// liter-llm's `ClientConfig`.
    #[test]
    fn test_build_client_config_maps_budget_settings() {
        let mut model_limits = std::collections::HashMap::new();
        model_limits.insert("openai/gpt-4o".to_string(), 25.0);
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            budget: Some(Box::new(LlmBudgetConfig {
                global_limit: Some(100.0),
                model_limits: Some(model_limits),
                enforcement: Some("soft".to_string()),
            })),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");
        let budget = client_config.budget_config.expect("budget_config present");

        assert_eq!(budget.global_limit, Some(100.0));
        assert_eq!(budget.model_limits.get("openai/gpt-4o"), Some(&25.0));
        assert_eq!(budget.enforcement, liter_llm::tower::Enforcement::Soft);
    }

    /// An unrecognized enforcement string must fall back to liter-llm's own
    /// default (`Hard`), matching `into_client_builder`'s `_ => Enforcement::Hard`.
    #[test]
    fn test_build_client_config_defaults_unknown_enforcement_to_hard() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            budget: Some(Box::new(LlmBudgetConfig {
                global_limit: Some(10.0),
                enforcement: Some("not-a-real-mode".to_string()),
                ..LlmBudgetConfig::default()
            })),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");
        let budget = client_config.budget_config.expect("budget_config present");

        assert_eq!(budget.enforcement, liter_llm::tower::Enforcement::Hard);
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// Rate limit RPM/TPM/window must reach liter-llm's `ClientConfig`.
    #[test]
    fn test_build_client_config_maps_rate_limit_settings() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            rate_limit: Some(Box::new(LlmRateLimitConfig {
                rpm: Some(60),
                tpm: Some(100_000),
                window_seconds: Some(30),
            })),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");
        let rate_limit = client_config.rate_limit_config.expect("rate_limit_config present");

        assert_eq!(rate_limit.rpm, Some(60));
        assert_eq!(rate_limit.tpm, Some(100_000));
        assert_eq!(rate_limit.window, std::time::Duration::from_secs(30));
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// `cost_tracking` and `tracing` flags must reach liter-llm's `ClientConfig`.
    #[test]
    fn test_build_client_config_maps_cost_tracking_and_tracing_flags() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            cost_tracking: Some(true),
            tracing: Some(true),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");

        assert!(client_config.enable_cost_tracking);
        assert!(client_config.enable_tracing);
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// `cooldown_secs` and `health_check_secs` must reach liter-llm's
    /// `ClientConfig` as `Duration`s.
    #[test]
    fn test_build_client_config_maps_cooldown_and_health_check_intervals() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            cooldown_secs: Some(15),
            health_check_secs: Some(45),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");

        assert_eq!(
            client_config.cooldown_duration,
            Some(std::time::Duration::from_secs(15))
        );
        assert_eq!(
            client_config.health_check_interval,
            Some(std::time::Duration::from_secs(45))
        );
    }

    /// With none of the new liter-llm passthrough fields set, every corresponding
    /// `ClientConfig` slot must stay at liter-llm's own default/unset state.
    #[test]
    fn test_build_client_config_leaves_new_passthrough_fields_unset_when_absent() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            ..LlmConfig::default()
        };

        let client_config = build_client_config(&config).expect("build client config");

        assert!(client_config.cache_config.is_none());
        assert!(client_config.budget_config.is_none());
        assert!(client_config.rate_limit_config.is_none());
        assert!(client_config.cooldown_duration.is_none());
        assert!(client_config.health_check_interval.is_none());
        assert!(!client_config.enable_cost_tracking);
        assert!(!client_config.enable_tracing);
    }

    /// Header validation must still run even when the request also carries the
    /// new tower-backed passthrough fields — `to_liter_llm_config` deliberately
    /// leaves `headers` unset on the DTO handed to `into_client_builder`, so this
    /// validation (not liter-llm's own silent-drop-on-parse-failure behavior) is
    /// what actually runs.
    #[test]
    fn test_build_client_config_still_validates_headers_with_tower_fields_set() {
        let mut headers = std::collections::HashMap::new();
        headers.insert("X-Bad\r\nInjected".to_string(), "value".to_string());
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            headers: Some(headers),
            cost_tracking: Some(true),
            ..LlmConfig::default()
        };

        match build_client_config(&config) {
            Err(crate::XbergError::Validation { message, .. }) => {
                assert!(message.contains("Invalid LLM header"), "unexpected message: {message}");
            }
            Err(other) => panic!("expected a Validation error, got: {other}"),
            Ok(_) => panic!("expected build_client_config to reject the invalid header"),
        }
    }

    /// `providers` has no equivalent field on liter-llm's `ClientConfig` at all —
    /// liter-llm's own `into_client_builder` does not map it either. This test proves
    /// the DTO-to-DTO conversion in `to_liter_llm_config` is lossless for `providers`
    /// on its own terms; it is *not* proof that a configured provider takes effect —
    /// see `test_build_client_config_registers_configured_providers_in_the_liter_llm_registry`
    /// below for that, which exercises the actual runtime registration in
    /// `register_configured_providers`/`build_client_config`.
    #[test]
    fn test_to_liter_llm_config_forwards_providers_into_the_dto_boundary() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            providers: Some(vec![LlmProviderConfig {
                name: "my-provider".to_string(),
                base_url: "https://my-llm.example.com/v1".to_string(),
                auth_header: Some("X-Api-Key".to_string()),
                model_prefixes: vec!["my-provider/".to_string()],
            }]),
            ..LlmConfig::default()
        };

        let upstream = to_liter_llm_config(&config);
        let providers = upstream.providers.expect("providers present");

        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "my-provider");
        assert_eq!(providers[0].base_url, "https://my-llm.example.com/v1");
        assert_eq!(providers[0].auth_header.as_deref(), Some("X-Api-Key"));
        assert_eq!(providers[0].model_prefixes, vec!["my-provider/".to_string()]);
    }

    /// `to_auth_header_format` must map `None` and the literal `"Authorization"` header
    /// name (case-insensitively) onto liter-llm's `Bearer` scheme — xberg's documented
    /// default.
    #[test]
    fn test_to_auth_header_format_maps_none_and_authorization_to_bearer() {
        assert!(matches!(
            to_auth_header_format(None),
            liter_llm::AuthHeaderFormat::Bearer
        ));
        assert!(matches!(
            to_auth_header_format(Some("Authorization")),
            liter_llm::AuthHeaderFormat::Bearer
        ));
        assert!(matches!(
            to_auth_header_format(Some("authorization")),
            liter_llm::AuthHeaderFormat::Bearer
        ));
    }

    /// Any other configured header name must map onto liter-llm's `ApiKey` variant,
    /// carrying the exact header name through unchanged.
    #[test]
    fn test_to_auth_header_format_maps_custom_header_name_to_api_key() {
        match to_auth_header_format(Some("X-Api-Key")) {
            liter_llm::AuthHeaderFormat::ApiKey(name) => assert_eq!(name, "X-Api-Key"),
            other => panic!("expected ApiKey variant, got {other:?}"),
        }
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// A configured `providers` entry must actually reach liter-llm's process-global
    /// custom-provider registry, not merely round-trip through the `to_liter_llm_config`
    /// DTO conversion (that is all `test_to_liter_llm_config_forwards_providers_into_the_dto_boundary`
    /// proves). `register_custom_provider`/`unregister_custom_provider` are the only
    /// public read/write surface liter-llm exposes on that registry —
    /// `unregister_custom_provider` returning `true` is used here as the observable
    /// proof of a prior successful registration, since there is no public getter.
    #[test]
    fn test_build_client_config_registers_configured_providers_in_the_liter_llm_registry() {
        let provider_name = "xberg-test-provider-registers-in-registry";
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            api_key: Some("test-key".to_string()),
            providers: Some(vec![LlmProviderConfig {
                name: provider_name.to_string(),
                base_url: "https://my-llm.example.com/v1".to_string(),
                auth_header: Some("X-Api-Key".to_string()),
                model_prefixes: vec!["xberg-test-provider-registers/".to_string()],
            }]),
            ..LlmConfig::default()
        };

        build_client_config(&config).expect("build client config");

        let removed = liter_llm::unregister_custom_provider(provider_name).expect("unregister should succeed");
        assert!(removed, "expected provider '{provider_name}' to have been registered");

        let removed_again = liter_llm::unregister_custom_provider(provider_name).expect("unregister should succeed");
        assert!(
            !removed_again,
            "provider should already be gone after the first unregister"
        );
    }

    /// Building a client twice from the same config (e.g. two extraction calls
    /// reusing one `LlmConfig`) must not fail merely because the provider was already
    /// registered — `register_custom_provider` replaces same-named entries rather than
    /// erroring, so repeated registration of identical data is idempotent.
    #[test]
    fn test_build_client_config_provider_registration_is_idempotent_for_same_config() {
        let provider_name = "xberg-test-provider-idempotent";
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            providers: Some(vec![LlmProviderConfig {
                name: provider_name.to_string(),
                base_url: "https://my-llm.example.com/v1".to_string(),
                auth_header: None,
                model_prefixes: vec!["xberg-test-provider-idempotent/".to_string()],
            }]),
            ..LlmConfig::default()
        };

        build_client_config(&config).expect("first registration should succeed");
        build_client_config(&config).expect("second registration with identical config should succeed");

        let removed = liter_llm::unregister_custom_provider(provider_name).expect("unregister should succeed");
        assert!(removed);
    }

    /// An invalid provider entry (liter-llm rejects an empty `model_prefixes` list —
    /// it would match every model) must surface as a named `XbergError::Validation`,
    /// not be swallowed.
    #[test]
    fn test_build_client_config_rejects_invalid_provider_registration() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            providers: Some(vec![LlmProviderConfig {
                name: "xberg-test-provider-invalid".to_string(),
                base_url: "https://my-llm.example.com/v1".to_string(),
                auth_header: None,
                model_prefixes: vec![],
            }]),
            ..LlmConfig::default()
        };

        match build_client_config(&config) {
            Err(crate::XbergError::Validation { message, .. }) => {
                assert!(
                    message.contains("xberg-test-provider-invalid"),
                    "error should name the provider: {message}"
                );
            }
            Err(other) => panic!("expected a Validation error, got: {other}"),
            Ok(_) => panic!("expected build_client_config to reject the invalid provider config"),
        }
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// `reasoning_effort` must parse into the exact matching `liter_llm::ReasoningEffort`
    /// variant, case-insensitively, so it can be assigned onto
    /// `ChatCompletionRequest::reasoning_effort` at each request-building call site.
    #[test]
    fn test_parse_reasoning_effort_maps_known_values_case_insensitively() {
        let cases = [
            ("low", liter_llm::ReasoningEffort::Low),
            ("MEDIUM", liter_llm::ReasoningEffort::Medium),
            ("High", liter_llm::ReasoningEffort::High),
            ("minimal", liter_llm::ReasoningEffort::Minimal),
            ("max", liter_llm::ReasoningEffort::Max),
        ];
        for (input, expected) in cases {
            let config = LlmConfig {
                model: "openai/gpt-4o".to_string(),
                reasoning_effort: Some(input.to_string()),
                ..LlmConfig::default()
            };
            assert_eq!(
                parse_reasoning_effort(&config).expect("known value should parse"),
                Some(expected),
                "input {input:?} mapped incorrectly"
            );
        }
    }

    /// An absent `reasoning_effort` must parse to `None` rather than defaulting to a
    /// specific effort level — silence in config should stay silence on the request.
    #[test]
    fn test_parse_reasoning_effort_returns_none_when_unset() {
        let config = LlmConfig::default();
        assert_eq!(parse_reasoning_effort(&config).expect("no value should parse"), None);
    }

    /// An unrecognized `reasoning_effort` string must be a named `XbergError::Validation`,
    /// not a silent drop of the misconfigured value.
    #[test]
    fn test_parse_reasoning_effort_rejects_unrecognized_value() {
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            reasoning_effort: Some("maximum".to_string()),
            ..LlmConfig::default()
        };

        match parse_reasoning_effort(&config) {
            Err(crate::XbergError::Validation { message, .. }) => {
                assert!(
                    message.contains("maximum"),
                    "error should name the bad value: {message}"
                );
            }
            other => panic!("expected a Validation error, got: {other:?}"),
        }
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// Demonstrates (and pins the exact value semantics of) the wiring a future change
    /// must add at every `ChatCompletionRequest`-building call site:
    /// `request.extra_body = config.extra_body.clone();`, exactly parallel to the
    /// existing `request.temperature = config.temperature;` /
    /// `request.max_tokens = config.max_tokens;` lines.
    #[test]
    fn test_extra_body_clones_into_a_liter_llm_chat_completion_request() {
        let extra = serde_json::json!({"safety_settings": {"harassment": "block_none"}});
        let config = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            extra_body: Some(extra.clone()),
            ..LlmConfig::default()
        };

        let request = liter_llm::ChatCompletionRequest {
            extra_body: config.extra_body.clone(),
            ..Default::default()
        };

        assert_eq!(request.extra_body, Some(extra));
    }
}
