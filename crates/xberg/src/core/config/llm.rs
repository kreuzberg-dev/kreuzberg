//! LLM configuration types for liter-llm integration.
//!
//! These types are always available (not feature-gated) since they are
//! pure configuration data with no runtime dependency on liter-llm.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Placeholder printed in place of a credential by the hand-written `Debug` impls
/// in this module. Mirrors liter-llm's own `ClientConfig` debug policy.
const REDACTED: &str = "[redacted]";

/// Configuration for an LLM provider/model via liter-llm.
///
/// Each feature (VLM OCR, VLM embeddings, structured extraction) carries
/// its own `LlmConfig`, allowing different providers per feature.
///
/// # Example
///
/// ```toml
/// [structured_extraction.llm]
/// model = "openai/gpt-4o"
/// api_key = "sk-..."  # or use XBERG_LLM_API_KEY env var
/// ```
///
/// `Debug` is implemented by hand so `api_key`, header values, and the AWS
/// credentials in [`BedrockConfig`] are never printed.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
pub struct LlmConfig {
    /// Provider/model string using liter-llm routing format.
    ///
    /// Examples: `"openai/gpt-4o"`, `"anthropic/claude-sonnet-4-20250514"`,
    /// `"groq/llama-3.1-70b-versatile"`.
    pub model: String,

    /// API key for the provider. When `None`, liter-llm falls back to
    /// the provider's standard environment variable (e.g., `OPENAI_API_KEY`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,

    /// Custom base URL override for the provider endpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base_url: Option<String>,

    /// Request timeout in seconds. When `None`, liter-llm's built-in 60s default
    /// applies, except the VLM OCR path which uses a 300s default (a single page
    /// image transcription routinely exceeds 60s). Set explicitly to override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,

    /// Maximum retry attempts (default: 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u32>,

    /// Sampling temperature for generation tasks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f64>,

    /// Maximum tokens to generate.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,

    /// Reasoning effort level for extended-thinking models, applied to individual
    /// requests built from this config.
    ///
    /// Mirrors liter-llm's `ChatCompletionRequest::reasoning_effort`
    /// (`types::chat::ReasoningEffort`). A request-time parameter like `temperature`/
    /// `max_tokens` above, not a client-level setting — `into_client_builder` does not
    /// map it. Accepted as a plain string — one of `"low"`, `"medium"`, `"high"`,
    /// `"minimal"`, `"max"` (case-insensitive; liter-llm's own
    /// `#[serde(rename_all = "lowercase")]` spelling) — rather than importing
    /// liter-llm's enum, because this module compiles even when the `liter-llm`
    /// feature is disabled. See [`crate::llm::client::parse_reasoning_effort`] for the
    /// conversion into `liter_llm::ReasoningEffort`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.2.0"))]
    pub reasoning_effort: Option<String>,

    /// Provider-specific extra parameters merged into the request body (guardrails,
    /// safety settings, grounding config, etc.), applied to individual requests built
    /// from this config.
    ///
    /// Mirrors liter-llm's `ChatCompletionRequest::extra_body`. A request-time
    /// parameter like `temperature`/`max_tokens` above, not a client-level setting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.2.0"))]
    pub extra_body: Option<serde_json::Value>,

    /// Whether liter-llm should load provider credentials from environment variables.
    ///
    /// Mirrors liter-llm's `ClientConfigBuilder::load_env`. When `None`, liter-llm's
    /// own default behavior applies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_env: Option<bool>,

    /// Extra HTTP headers sent with every request to the provider.
    ///
    /// Mirrors liter-llm's `ClientConfigBuilder::header`, for gateways or providers
    /// that require custom auth/routing headers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<HashMap<String, String>>,

    /// Custom provider configurations, in addition to liter-llm's built-in providers.
    ///
    /// Mirrors liter-llm's `LlmConfig::providers`, for OpenAI-compatible gateways
    /// and self-hosted model servers that are not in the built-in provider catalog.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub providers: Option<Vec<LlmProviderConfig>>,

    /// Response cache configuration.
    ///
    /// Mirrors liter-llm's `LlmConfig::cache`. Only takes effect when liter-llm's
    /// `tower` feature is compiled in; otherwise the value is accepted but unused.
    ///
    /// Boxed for the same reason as `bedrock` (a4579589ac): `LlmConfig` is the payload
    /// of `EmbeddingModelType::Llm` and `RerankerModelType::Llm`, whose other variants
    /// are tens of bytes. Inlining this and the two sub-configs below pushed that
    /// variant to 480 bytes and tripped `clippy::large_enum_variant` on the
    /// `--features full` leg. ~keep
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub cache: Option<Box<LlmCacheConfig>>,

    /// Budget enforcement configuration.
    ///
    /// Mirrors liter-llm's `LlmConfig::budget`. Only takes effect when liter-llm's
    /// `tower` feature is compiled in; otherwise the value is accepted but unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub budget: Option<Box<LlmBudgetConfig>>,

    /// Per-model rate limiting configuration.
    ///
    /// Mirrors liter-llm's `LlmConfig::rate_limit`. Only takes effect when liter-llm's
    /// `tower` feature is compiled in; otherwise the value is accepted but unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub rate_limit: Option<Box<LlmRateLimitConfig>>,

    /// Enable per-request cost tracking.
    ///
    /// Mirrors liter-llm's `LlmConfig::cost_tracking`. Only takes effect when
    /// liter-llm's `tower` feature is compiled in; otherwise the value is accepted
    /// but unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub cost_tracking: Option<bool>,

    /// Enable OpenTelemetry-compatible tracing spans.
    ///
    /// Mirrors liter-llm's `LlmConfig::tracing`. Only takes effect when liter-llm's
    /// `tower` feature is compiled in; otherwise the value is accepted but unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub tracing: Option<bool>,

    /// Cooldown duration after transient errors, in seconds.
    ///
    /// Mirrors liter-llm's `LlmConfig::cooldown_secs`. Only takes effect when
    /// liter-llm's `tower` feature is compiled in; otherwise the value is accepted
    /// but unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub cooldown_secs: Option<u64>,

    /// Background health check interval, in seconds.
    ///
    /// Mirrors liter-llm's `LlmConfig::health_check_secs`. Only takes effect when
    /// liter-llm's `tower` feature is compiled in; otherwise the value is accepted
    /// but unused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub health_check_secs: Option<u64>,

    /// AWS Bedrock settings (region, cross-region routing, explicit credentials).
    ///
    /// Only consulted for `bedrock/`-prefixed models. When `None` — or when an
    /// individual field inside it is `None` — liter-llm falls back to the standard
    /// AWS environment variables and the default credential chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
    pub bedrock: Option<Box<BedrockConfig>>,
}

/// A custom provider configuration entry, in addition to liter-llm's built-in providers.
///
/// Mirrors liter-llm's `LlmProviderConfig`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct LlmProviderConfig {
    /// Provider name, used to key model prefix matching.
    pub name: String,

    /// Base URL for the provider's OpenAI-compatible API.
    pub base_url: String,

    /// Header name used to carry the API key (defaults to `Authorization` when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth_header: Option<String>,

    /// Model name prefixes routed to this provider (e.g. `["my-provider/"]`).
    #[serde(default)]
    pub model_prefixes: Vec<String>,
}

/// Response cache configuration.
///
/// Mirrors liter-llm's `LlmCacheConfig`. Only takes effect when liter-llm's `tower`
/// feature is compiled in; otherwise the value round-trips through configuration
/// but is not consulted at request time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct LlmCacheConfig {
    /// Maximum number of cached entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_entries: Option<usize>,

    /// Cache entry time-to-live, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ttl_seconds: Option<u64>,

    /// Cache backend name (e.g. `"memory"`, or an `opendal` scheme).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,

    /// Backend-specific configuration key/value pairs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend_config: Option<HashMap<String, String>>,
}

/// Budget enforcement configuration.
///
/// Mirrors liter-llm's `LlmBudgetConfig`. Only takes effect when liter-llm's `tower`
/// feature is compiled in; otherwise the value round-trips through configuration
/// but is not enforced at request time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct LlmBudgetConfig {
    /// Global spend limit in USD.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub global_limit: Option<f64>,

    /// Per-model spend limits in USD, keyed by model name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_limits: Option<HashMap<String, f64>>,

    /// Enforcement mode: `"hard"` (reject over-budget requests) or `"soft"` (log only).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<String>,
}

/// Per-model rate limiting configuration.
///
/// Mirrors liter-llm's `LlmRateLimitConfig`. Only takes effect when liter-llm's
/// `tower` feature is compiled in; otherwise the value round-trips through
/// configuration but is not enforced at request time.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct LlmRateLimitConfig {
    /// Requests per minute limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rpm: Option<u32>,

    /// Tokens per minute limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tpm: Option<u64>,

    /// Rate limit window, in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<u64>,
}

/// AWS Bedrock configuration for `bedrock/`-prefixed models.
///
/// Mirrors liter-llm's `BedrockConfig`. Every field is optional: anything left
/// unset falls back to the standard AWS environment variables
/// (`AWS_DEFAULT_REGION` / `AWS_REGION`, `AWS_ACCESS_KEY_ID`,
/// `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `BEDROCK_CROSS_REGION`) and the
/// default AWS credential chain. Leave the credential fields unset unless you
/// have an explicit reason to pin them.
///
/// # Example
///
/// ```toml
/// [ocr.vlm_config]
/// model = "bedrock/anthropic.claude-3-sonnet-20240229-v1:0"
///
/// [ocr.vlm_config.bedrock]
/// region = "eu-central-1"
/// cross_region_prefix = "eu"
/// ```
///
/// `Debug` is implemented by hand so the three credential fields are never printed.
#[derive(Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[cfg_attr(feature = "alef-meta", alef(since = "1.1.0"))]
pub struct BedrockConfig {
    /// AWS region (e.g. `"us-east-1"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,

    /// Cross-region inference profile prefix (e.g. `"us"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cross_region_prefix: Option<String>,

    /// Explicit AWS access key ID. Secret — never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access_key_id: Option<String>,

    /// Explicit AWS secret access key. Secret — never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret_access_key: Option<String>,

    /// Explicit AWS session token for temporary credentials. Secret — never logged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_token: Option<String>,
}

/// Redacting `Debug`: `api_key` and every header value are credentials, so only
/// their presence (and, for headers, the header name) is printed.
impl std::fmt::Debug for LlmConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let redacted_headers: Option<Vec<(&str, &str)>> = self
            .headers
            .as_ref()
            .map(|headers| headers.keys().map(|name| (name.as_str(), REDACTED)).collect());

        f.debug_struct("LlmConfig")
            .field("model", &self.model)
            .field("api_key", &self.api_key.as_ref().map(|_| REDACTED))
            .field("base_url", &self.base_url)
            .field("timeout_secs", &self.timeout_secs)
            .field("max_retries", &self.max_retries)
            .field("temperature", &self.temperature)
            .field("max_tokens", &self.max_tokens)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("extra_body", &self.extra_body)
            .field("load_env", &self.load_env)
            .field("headers", &redacted_headers)
            .field("providers", &self.providers)
            .field("cache", &self.cache)
            .field("budget", &self.budget)
            .field("rate_limit", &self.rate_limit)
            .field("cost_tracking", &self.cost_tracking)
            .field("tracing", &self.tracing)
            .field("cooldown_secs", &self.cooldown_secs)
            .field("health_check_secs", &self.health_check_secs)
            .field("bedrock", &self.bedrock)
            .finish()
    }
}

/// Redacting `Debug`: the three AWS credential fields print only their presence.
/// `region` and `cross_region_prefix` are not secrets and are printed verbatim.
impl std::fmt::Debug for BedrockConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BedrockConfig")
            .field("region", &self.region)
            .field("cross_region_prefix", &self.cross_region_prefix)
            .field("access_key_id", &self.access_key_id.as_ref().map(|_| REDACTED))
            .field("secret_access_key", &self.secret_access_key.as_ref().map(|_| REDACTED))
            .field("session_token", &self.session_token.as_ref().map(|_| REDACTED))
            .finish()
    }
}

/// Configuration for LLM-based structured data extraction.
///
/// Sends extracted document content to a VLM with a JSON schema,
/// returning structured data that conforms to the schema.
///
/// # Example
///
/// ```toml
/// [structured_extraction]
/// schema_name = "invoice_data"
/// strict = true
///
/// [structured_extraction.schema]
/// type = "object"
/// properties.vendor = { type = "string" }
/// properties.total = { type = "number" }
/// required = ["vendor", "total"]
///
/// [structured_extraction.llm]
/// model = "openai/gpt-4o"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredExtractionConfig {
    /// JSON Schema defining the desired output structure.
    pub schema: serde_json::Value,

    /// Schema name passed to the LLM's structured output mode.
    #[serde(default = "default_schema_name")]
    pub schema_name: String,

    /// Optional schema description for the LLM.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_description: Option<String>,

    /// Enable strict mode — output must exactly match the schema.
    #[serde(default)]
    pub strict: bool,

    /// Custom Jinja2 extraction prompt template. When `None`, a default template is used.
    ///
    /// Available template variables:
    /// - `{{ content }}` — The extracted document text.
    /// - `{{ schema }}` — The JSON schema as a formatted string.
    /// - `{{ schema_name }}` — The schema name.
    /// - `{{ schema_description }}` — The schema description (may be empty).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    /// LLM configuration for the extraction.
    pub llm: LlmConfig,
}

fn default_schema_name() -> String {
    "extraction".to_string()
}

/// How a structured-extraction preset is dispatched to the model.
///
/// This is the preset-facing call mode (the `preferred_call_mode` field of a
/// [`crate::presets::Preset`]). The structured pipeline has a richer
/// runtime-only decision enum with skip and fallback states; this 3-variant
/// type is the stable, serializable surface presets and bindings depend on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum CallMode {
    /// Use the extracted text only.
    #[default]
    TextOnly,
    /// Use rasterized page images only.
    VisionOnly,
    /// Provide both extracted text and page images to the model.
    TextPlusVision,
}

/// How partial results from multiple model calls (e.g. per page batch) are combined.
///
/// Canonical home for the merge strategy referenced by presets and by the
/// structured pipeline's post-processing. There is intentionally only one merge
/// type across the crate — do not introduce a second.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[cfg_attr(feature = "api", derive(utoipa::ToSchema))]
#[serde(rename_all = "snake_case")]
pub enum MergeMode {
    /// Deep-merge JSON objects field by field (later calls fill missing fields).
    #[default]
    ObjectMerge,
    /// Concatenate top-level arrays across calls.
    ArrayConcat,
    /// Keep the first non-empty result; ignore subsequent calls.
    ObjectFirst,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression test for https://github.com/xberg-io/xberg/issues/716
    ///
    /// `LlmConfig` must implement `Default` so callers can use the struct-update
    /// syntax documented in the VLM OCR guide:
    ///
    /// ```rust
    /// use xberg::core::config::LlmConfig;
    /// let cfg = LlmConfig {
    ///     model: "openai/gpt-4o-mini".to_string(),
    ///     ..Default::default()
    /// };
    /// ```
    #[test]
    fn test_llm_config_default_trait_is_satisfied() {
        let cfg = LlmConfig::default();
        assert!(cfg.model.is_empty(), "default model should be empty string");
        assert!(cfg.api_key.is_none());
        assert!(cfg.base_url.is_none());
        assert!(cfg.timeout_secs.is_none());
        assert!(cfg.max_retries.is_none());
        assert!(cfg.temperature.is_none());
        assert!(cfg.max_tokens.is_none());
        assert!(cfg.reasoning_effort.is_none());
        assert!(cfg.extra_body.is_none());
        assert!(cfg.load_env.is_none());
        assert!(cfg.headers.is_none());
        assert!(cfg.providers.is_none());
        assert!(cfg.cache.is_none());
        assert!(cfg.budget.is_none());
        assert!(cfg.rate_limit.is_none());
        assert!(cfg.cost_tracking.is_none());
        assert!(cfg.tracing.is_none());
        assert!(cfg.cooldown_secs.is_none());
        assert!(cfg.health_check_secs.is_none());
        assert!(cfg.bedrock.is_none());
    }

    /// Verify the struct-update pattern from the issue compiles and produces
    /// only the explicitly set field.
    #[test]
    fn test_llm_config_struct_update_syntax() {
        let cfg = LlmConfig {
            model: "openai/gpt-4o-mini".to_string(),
            ..Default::default()
        };
        assert_eq!(cfg.model, "openai/gpt-4o-mini");
        assert!(cfg.api_key.is_none());
        assert!(cfg.base_url.is_none());
        assert!(cfg.timeout_secs.is_none());
        assert!(cfg.max_retries.is_none());
        assert!(cfg.temperature.is_none());
        assert!(cfg.max_tokens.is_none());
        assert!(cfg.reasoning_effort.is_none());
        assert!(cfg.extra_body.is_none());
        assert!(cfg.load_env.is_none());
        assert!(cfg.headers.is_none());
        assert!(cfg.providers.is_none());
        assert!(cfg.cache.is_none());
        assert!(cfg.budget.is_none());
        assert!(cfg.rate_limit.is_none());
        assert!(cfg.cost_tracking.is_none());
        assert!(cfg.tracing.is_none());
        assert!(cfg.cooldown_secs.is_none());
        assert!(cfg.health_check_secs.is_none());
        assert!(cfg.bedrock.is_none());
    }

    /// `load_env` and `headers` must round-trip through TOML so they are settable
    /// from a config file and every language binding.
    #[test]
    fn test_llm_config_load_env_and_headers_round_trip() {
        let toml_src = r#"
model = "openai/gpt-4o"
load_env = true

[headers]
"X-Gateway-Key" = "abc123"
"X-Tenant" = "acme"
"#;
        let cfg: LlmConfig = toml::from_str(toml_src).expect("deserialize LlmConfig from TOML");
        assert_eq!(cfg.model, "openai/gpt-4o");
        assert_eq!(cfg.load_env, Some(true));
        let headers = cfg.headers.as_ref().expect("headers present");
        assert_eq!(headers.get("X-Gateway-Key").map(String::as_str), Some("abc123"));
        assert_eq!(headers.get("X-Tenant").map(String::as_str), Some("acme"));

        let round_tripped: LlmConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).expect("serialize")).expect("deserialize");
        assert_eq!(round_tripped, cfg);
    }

    /// Empty passthrough fields stay absent from serialized output so bindings
    /// never emit `null`/empty knobs the user did not set.
    #[test]
    fn test_llm_config_omits_empty_passthrough_fields() {
        let cfg = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).expect("serialize");
        assert!(
            !json.contains("reasoning_effort"),
            "reasoning_effort should be omitted when None: {json}"
        );
        assert!(
            !json.contains("extra_body"),
            "extra_body should be omitted when None: {json}"
        );
        assert!(
            !json.contains("load_env"),
            "load_env should be omitted when None: {json}"
        );
        assert!(!json.contains("headers"), "headers should be omitted when None: {json}");
        assert!(
            !json.contains("providers"),
            "providers should be omitted when None: {json}"
        );
        assert!(!json.contains("cache"), "cache should be omitted when None: {json}");
        assert!(!json.contains("budget"), "budget should be omitted when None: {json}");
        assert!(
            !json.contains("rate_limit"),
            "rate_limit should be omitted when None: {json}"
        );
        assert!(
            !json.contains("cost_tracking"),
            "cost_tracking should be omitted when None: {json}"
        );
        assert!(!json.contains("tracing"), "tracing should be omitted when None: {json}");
        assert!(
            !json.contains("cooldown_secs"),
            "cooldown_secs should be omitted when None: {json}"
        );
        assert!(
            !json.contains("health_check_secs"),
            "health_check_secs should be omitted when None: {json}"
        );
        assert!(!json.contains("bedrock"), "bedrock should be omitted when None: {json}");
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// `providers`, `cache`, `budget`, `rate_limit`, `cost_tracking`, `tracing`,
    /// `cooldown_secs`, and `health_check_secs` must survive a TOML load and a
    /// JSON round-trip so they are settable from a config file and from every
    /// language binding, matching liter-llm's canonical `client::LlmConfig`.
    #[test]
    fn test_llm_config_full_passthrough_fields_round_trip_through_toml_and_json() {
        let toml_src = r#"
model = "openai/gpt-4o"
cost_tracking = true
tracing = false
cooldown_secs = 30
health_check_secs = 60

[[providers]]
name = "my-provider"
base_url = "https://my-llm.example.com/v1"
auth_header = "X-Api-Key"
model_prefixes = ["my-provider/"]

[cache]
max_entries = 512
ttl_seconds = 600
backend = "memory"

[budget]
global_limit = 100.0
enforcement = "hard"

[budget.model_limits]
"openai/gpt-4o" = 25.0

[rate_limit]
rpm = 60
tpm = 100000
window_seconds = 60
"#;
        let cfg: LlmConfig = toml::from_str(toml_src).expect("deserialize LlmConfig from TOML");

        assert_eq!(cfg.cost_tracking, Some(true));
        assert_eq!(cfg.tracing, Some(false));
        assert_eq!(cfg.cooldown_secs, Some(30));
        assert_eq!(cfg.health_check_secs, Some(60));

        let providers = cfg.providers.as_ref().expect("providers present");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0].name, "my-provider");
        assert_eq!(providers[0].base_url, "https://my-llm.example.com/v1");
        assert_eq!(providers[0].auth_header.as_deref(), Some("X-Api-Key"));
        assert_eq!(providers[0].model_prefixes, vec!["my-provider/".to_string()]);

        let cache = cfg.cache.as_ref().expect("cache present");
        assert_eq!(cache.max_entries, Some(512));
        assert_eq!(cache.ttl_seconds, Some(600));
        assert_eq!(cache.backend.as_deref(), Some("memory"));
        assert_eq!(cache.backend_config, None);

        let budget = cfg.budget.as_ref().expect("budget present");
        assert_eq!(budget.global_limit, Some(100.0));
        assert_eq!(budget.enforcement.as_deref(), Some("hard"));
        assert_eq!(
            budget.model_limits.as_ref().and_then(|m| m.get("openai/gpt-4o")),
            Some(&25.0)
        );

        let rate_limit = cfg.rate_limit.as_ref().expect("rate_limit present");
        assert_eq!(rate_limit.rpm, Some(60));
        assert_eq!(rate_limit.tpm, Some(100_000));
        assert_eq!(rate_limit.window_seconds, Some(60));

        let round_tripped: LlmConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).expect("serialize")).expect("deserialize");
        assert_eq!(round_tripped, cfg);
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// `reasoning_effort` and `extra_body` must survive a TOML load and a JSON
    /// round-trip so they are settable from a config file and from every language
    /// binding, matching liter-llm's `ChatCompletionRequest` request-time fields.
    #[test]
    fn test_llm_config_reasoning_effort_and_extra_body_round_trip_through_toml_and_json() {
        let toml_src = r#"
model = "openai/gpt-4o"
reasoning_effort = "high"

[extra_body]
safety_settings = { harassment = "block_none" }
"#;
        let cfg: LlmConfig = toml::from_str(toml_src).expect("deserialize LlmConfig from TOML");

        assert_eq!(cfg.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(
            cfg.extra_body,
            Some(serde_json::json!({"safety_settings": {"harassment": "block_none"}}))
        );

        let round_tripped: LlmConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).expect("serialize")).expect("deserialize");
        assert_eq!(round_tripped, cfg);
    }

    /// `Debug` prints `reasoning_effort` and `extra_body` verbatim — neither is a
    /// credential.
    #[test]
    fn test_llm_config_debug_prints_reasoning_effort_and_extra_body_verbatim() {
        let cfg = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            reasoning_effort: Some("high".to_string()),
            extra_body: Some(serde_json::json!({"foo": "bar"})),
            ..Default::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains(r#"reasoning_effort: Some("high")"#), "{rendered}");
        assert!(rendered.contains(r#"extra_body: Some(Object"#), "{rendered}");
        assert!(rendered.contains("bar"), "{rendered}");
    }

    /// `Debug` prints the new passthrough fields verbatim — none of them are
    /// credentials, unlike `api_key`, header values, and the Bedrock secrets.
    #[test]
    fn test_llm_config_debug_prints_new_passthrough_fields_verbatim() {
        let cfg = LlmConfig {
            model: "openai/gpt-4o".to_string(),
            cost_tracking: Some(true),
            tracing: Some(true),
            cooldown_secs: Some(15),
            health_check_secs: Some(45),
            rate_limit: Some(Box::new(LlmRateLimitConfig {
                rpm: Some(30),
                tpm: None,
                window_seconds: None,
            })),
            ..Default::default()
        };
        let rendered = format!("{cfg:?}");
        assert!(rendered.contains("cost_tracking: Some(true)"), "{rendered}");
        assert!(rendered.contains("tracing: Some(true)"), "{rendered}");
        assert!(rendered.contains("cooldown_secs: Some(15)"), "{rendered}");
        assert!(rendered.contains("health_check_secs: Some(45)"), "{rendered}");
        assert!(rendered.contains("rpm: Some(30)"), "{rendered}");
    }

    /// Regression test for https://github.com/xberg-io/xberg/issues/1381
    ///
    /// Bedrock region, cross-region prefix, and credentials must survive a TOML
    /// load and a JSON round-trip so they are settable from a config file and
    /// from every language binding.
    #[test]
    fn test_llm_config_bedrock_round_trips_through_toml_and_json() {
        let toml_src = r#"
model = "bedrock/anthropic.claude-3-sonnet-20240229-v1:0"

[bedrock]
region = "eu-central-1"
cross_region_prefix = "eu"
access_key_id = "AKIAEXAMPLE"
secret_access_key = "example-secret"
session_token = "example-token"
"#;
        let cfg: LlmConfig = toml::from_str(toml_src).expect("deserialize LlmConfig from TOML");
        assert_eq!(cfg.model, "bedrock/anthropic.claude-3-sonnet-20240229-v1:0");
        let bedrock = cfg.bedrock.as_ref().expect("bedrock present");
        assert_eq!(bedrock.region.as_deref(), Some("eu-central-1"));
        assert_eq!(bedrock.cross_region_prefix.as_deref(), Some("eu"));
        assert_eq!(bedrock.access_key_id.as_deref(), Some("AKIAEXAMPLE"));
        assert_eq!(bedrock.secret_access_key.as_deref(), Some("example-secret"));
        assert_eq!(bedrock.session_token.as_deref(), Some("example-token"));

        let round_tripped: LlmConfig =
            serde_json::from_str(&serde_json::to_string(&cfg).expect("serialize")).expect("deserialize");
        assert_eq!(round_tripped, cfg);
    }

    /// A `bedrock` table carrying only `region` must leave the credential fields
    /// unset so the AWS default credential chain still applies.
    #[test]
    fn test_llm_config_bedrock_region_only_leaves_credentials_unset() {
        let cfg: LlmConfig = toml::from_str(
            r#"
model = "bedrock/anthropic.claude-3-sonnet-20240229-v1:0"

[bedrock]
region = "us-east-1"
"#,
        )
        .expect("deserialize LlmConfig from TOML");
        let bedrock = cfg.bedrock.as_ref().expect("bedrock present");
        assert_eq!(bedrock.region.as_deref(), Some("us-east-1"));
        assert_eq!(bedrock.cross_region_prefix, None);
        assert_eq!(bedrock.access_key_id, None);
        assert_eq!(bedrock.secret_access_key, None);
        assert_eq!(bedrock.session_token, None);
    }

    /// `Debug` must never print a credential — `LlmConfig` and `BedrockConfig` are
    /// reachable from error contexts and diagnostic dumps.
    #[test]
    fn test_llm_config_debug_redacts_every_credential() {
        let mut headers = HashMap::new();
        headers.insert("X-Gateway-Key".to_string(), "header-secret".to_string());
        let cfg = LlmConfig {
            model: "bedrock/anthropic.claude-3-sonnet-20240229-v1:0".to_string(),
            api_key: Some("sk-super-secret".to_string()),
            headers: Some(headers),
            bedrock: Some(Box::new(BedrockConfig {
                region: Some("eu-central-1".to_string()),
                cross_region_prefix: Some("eu".to_string()),
                access_key_id: Some("AKIAEXAMPLE".to_string()),
                secret_access_key: Some("aws-secret".to_string()),
                session_token: Some("aws-token".to_string()),
            })),
            ..Default::default()
        };

        let rendered = format!("{cfg:?}");
        for secret in [
            "sk-super-secret",
            "header-secret",
            "AKIAEXAMPLE",
            "aws-secret",
            "aws-token",
        ] {
            assert!(!rendered.contains(secret), "Debug leaked {secret}: {rendered}");
        }
        // Non-secret routing values stay visible so the output is still diagnosable.
        assert!(
            rendered.contains("eu-central-1"),
            "region should be printed: {rendered}"
        );
        assert!(rendered.contains("\"eu\""), "prefix should be printed: {rendered}");
        assert!(
            rendered.contains("X-Gateway-Key"),
            "header name should be printed: {rendered}"
        );
        assert_eq!(
            rendered.matches(REDACTED).count(),
            5,
            "expected 5 redactions: {rendered}"
        );
    }

    /// An unset credential must render as `None`, not as a redaction placeholder,
    /// so an operator can tell "not configured" from "configured but hidden".
    #[test]
    fn test_bedrock_config_debug_distinguishes_unset_from_redacted() {
        let bedrock = BedrockConfig {
            region: Some("us-east-1".to_string()),
            ..Default::default()
        };
        let rendered = format!("{bedrock:?}");
        assert_eq!(rendered.matches(REDACTED).count(), 0, "nothing to redact: {rendered}");
        assert_eq!(rendered.matches("None").count(), 4, "4 unset fields: {rendered}");
    }

    #[test]
    fn test_call_mode_serde_round_trip() {
        for (mode, wire) in [
            (CallMode::TextOnly, "\"text_only\""),
            (CallMode::VisionOnly, "\"vision_only\""),
            (CallMode::TextPlusVision, "\"text_plus_vision\""),
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            assert_eq!(json, wire);
            let decoded: CallMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, mode);
        }
        assert_eq!(CallMode::default(), CallMode::TextOnly);
    }

    #[test]
    fn test_merge_mode_serde_round_trip() {
        for (mode, wire) in [
            (MergeMode::ObjectMerge, "\"object_merge\""),
            (MergeMode::ArrayConcat, "\"array_concat\""),
            (MergeMode::ObjectFirst, "\"object_first\""),
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            assert_eq!(json, wire);
            let decoded: MergeMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, mode);
        }
        assert_eq!(MergeMode::default(), MergeMode::ObjectMerge);
    }
}
