//! VLM-based OCR using liter-llm vision models.
//!
//! Provides text extraction from images by sending them to a vision language
//! model (e.g., GPT-4o, Claude) via the liter-llm client.  This is an
//! alternative to traditional OCR backends (Tesseract, PaddleOCR) and can
//! produce higher-quality results for complex layouts, handwriting, or
//! low-quality scans.

use std::borrow::Cow;

use async_trait::async_trait;
use base64::Engine;
use liter_llm::types::ContentPart;
use liter_llm::{ChatCompletionRequest, ImageUrl, LlmClient, Message, UserContent, UserMessage};

use crate::core::config::LlmConfig;
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};

/// Default request timeout for VLM OCR when `vlm_config.timeout_secs` is unset.
///
/// Transcribing a single full page image routinely exceeds liter-llm's built-in
/// 60-second client default, so an unset timeout would otherwise fail long
/// extractions. Applied only to the VLM OCR path; callers that set
/// `timeout_secs` explicitly always win.
const DEFAULT_VLM_TIMEOUT_SECS: u64 = 300;

/// Return the config to use for the VLM client, applying [`DEFAULT_VLM_TIMEOUT_SECS`]
/// when the caller left `timeout_secs` unset. An explicit value is preserved.
fn effective_vlm_config(config: &LlmConfig) -> Cow<'_, LlmConfig> {
    if config.timeout_secs.is_none() {
        let mut owned = config.clone();
        owned.timeout_secs = Some(DEFAULT_VLM_TIMEOUT_SECS);
        Cow::Owned(owned)
    } else {
        Cow::Borrowed(config)
    }
}

/// VLM-based OCR backend using liter-llm vision models.
///
/// This backend sends images to a vision language model (e.g., GPT-4o, Claude)
/// for text extraction, as an alternative to traditional OCR backends.
#[cfg_attr(alef, alef(skip))]
pub struct VlmOcrBackend;

impl Plugin for VlmOcrBackend {
    fn name(&self) -> &str {
        "vlm"
    }

    fn version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_string()
    }

    fn initialize(&self) -> crate::Result<()> {
        Ok(())
    }

    fn shutdown(&self) -> crate::Result<()> {
        Ok(())
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl OcrBackend for VlmOcrBackend {
    async fn process_image(
        &self,
        image_bytes: &[u8],
        config: &crate::OcrConfig,
    ) -> crate::Result<crate::ExtractedDocument> {
        let vlm_config = config
            .vlm_config
            .as_ref()
            .ok_or_else(|| crate::XbergError::validation("VLM OCR requires vlm_config to be set"))?;

        let mime = infer::get(image_bytes).map(|t| t.mime_type()).unwrap_or("image/png");

        let languages = config.effective_languages();
        let lang_str = languages[0].as_str();

        let (text, usage) = vlm_ocr(image_bytes, mime, lang_str, vlm_config, config.vlm_prompt.as_deref()).await?;

        Ok(crate::ExtractedDocument {
            content: text,
            mime_type: Cow::Borrowed("text/plain"),
            llm_usage: usage.map(|u| vec![u]),
            ..Default::default()
        })
    }

    fn supports_language(&self, _lang: &str) -> bool {
        true
    }

    #[cfg_attr(alef, alef(skip))]
    fn backend_type(&self) -> OcrBackendType {
        OcrBackendType::Custom
    }

    #[cfg_attr(alef, alef(skip))]
    fn probe(&self, config: &crate::OcrConfig) -> crate::doctor::DoctorCheck {
        use crate::doctor::DoctorCheck;

        let Some(vlm_config) = config.vlm_config.as_ref() else {
            return DoctorCheck::fail(
                "ocr.vlm",
                "ocr.backend = \"vlm\" requires vlm_config (model endpoint configuration)",
            );
        };

        // Client construction resolves the provider, applies the env-var key
        // fallback, and validates provider requirements (e.g. Azure base_url).
        // liter-llm error messages name the missing env var, never the secret.
        if let Err(e) = super::client::create_client(vlm_config) {
            return DoctorCheck::fail("ocr.vlm", format!("{e}"));
        }

        let Some(base_url) = vlm_config
            .base_url
            .clone()
            .or_else(|| default_base_url_for_model(&vlm_config.model))
        else {
            return DoctorCheck::skip(
                "ocr.vlm",
                "client config ok; endpoint reachability not probed (no base_url in config or provider default)",
            );
        };

        probe_endpoint(&base_url)
    }
}

/// Resolve a provider's default base URL from the model string via liter-llm's
/// static provider registry (explicit `provider/` prefix first, then model
/// prefixes). Custom providers without a default return None.
fn default_base_url_for_model(model: &str) -> Option<String> {
    let providers = liter_llm::provider::all_providers().ok()?;
    let (prefixed, bare) = match model.split_once('/') {
        Some((head, rest)) => (Some(head), rest),
        None => (None, model),
    };
    providers
        .iter()
        .find(|p| Some(p.name.as_str()) == prefixed)
        .or_else(|| {
            providers.iter().find(|p| {
                p.model_prefixes
                    .as_ref()
                    .is_some_and(|prefixes| prefixes.iter().any(|px| bare.starts_with(px.as_str())))
            })
        })
        .and_then(|p| p.base_url.clone())
}

/// Unauthenticated GET against the configured endpoint: any HTTP response —
/// including 401/404 — proves reachability. No API key is sent, so nothing
/// billable happens and no secret can leak into the report; transport errors
/// are reported without the request URL.
fn probe_endpoint(base_url: &str) -> crate::doctor::DoctorCheck {
    use crate::doctor::DoctorCheck;

    let config = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(5)))
        .build();
    let agent = ureq::Agent::new_with_config(config);

    match agent.get(base_url).call() {
        Ok(response) => DoctorCheck::pass("ocr.vlm", format!("endpoint reachable (HTTP {})", response.status())),
        Err(ureq::Error::StatusCode(code)) => DoctorCheck::pass("ocr.vlm", format!("endpoint reachable (HTTP {code})")),
        Err(e) => {
            tracing::debug!(error = %e, "vlm endpoint probe failed");
            DoctorCheck::fail(
                "ocr.vlm",
                "endpoint unreachable (connection failed; run with RUST_LOG=debug for detail)",
            )
        }
    }
}

/// Perform OCR on an image using a vision language model.
///
/// Sends the image to a VLM (e.g., GPT-4o, Claude) which extracts text.
/// The language hint is included in the prompt when the document language
/// is not English.
///
/// # Arguments
///
/// * `image_bytes` - Raw image data (JPEG, PNG, WebP, etc.)
/// * `image_mime_type` - MIME type of the image (e.g., `"image/png"`)
/// * `language` - ISO 639 language code or Tesseract language name
///   (e.g., `"eng"`, `"de"`, `"fra"`)
/// * `config` - LLM provider/model configuration
///
/// # Returns
///
/// Extracted text from the image, or an error if the VLM call fails.
///
/// # Errors
///
/// - `XbergError::Ocr` if the VLM returns no content or the API call fails
/// - `XbergError::MissingDependency` if the liter-llm client cannot be created
#[allow(clippy::field_reassign_with_default)]
pub(crate) async fn vlm_ocr(
    image_bytes: &[u8],
    image_mime_type: &str,
    language: &str,
    config: &LlmConfig,
    vlm_prompt: Option<&str>,
) -> crate::Result<(String, Option<crate::types::LlmUsage>)> {
    // liter-llm applies its own 60s default when no timeout is set, which is too
    // short for full-page VLM transcription. Supply a VLM-appropriate default when
    // the caller left `timeout_secs` unset; an explicit value always takes priority. ~keep
    let effective_config = effective_vlm_config(config);
    let client = super::client::create_client(&effective_config)?;

    let b64 = base64::engine::general_purpose::STANDARD.encode(image_bytes);
    let data_url = format!("data:{image_mime_type};base64,{b64}");

    let template = vlm_prompt.unwrap_or(super::prompts::VLM_OCR_TEMPLATE);
    let ctx = minijinja::context! { language => language };
    let prompt = super::prompts::render_template(template, &ctx)?;

    let message = Message::User(UserMessage {
        content: UserContent::Parts(vec![
            ContentPart::Text { text: prompt },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: data_url,
                    detail: None,
                },
            },
        ]),
        name: None,
    });

    let mut request = ChatCompletionRequest::default();
    request.model = normalize_vlm_model(&config.model, config.base_url.as_deref());
    request.messages = vec![message];
    request.temperature = config.temperature;
    request.max_tokens = config.max_tokens;

    let response = client.chat(request).await.map_err(|e| {
        crate::XbergError::ocr(format!(
            "VLM OCR request failed: model={}, language={}, image_size={}KB: {e}",
            config.model,
            language,
            image_bytes.len() / 1024
        ))
    })?;

    let usage = super::usage::extract_usage_from_chat(&response, "vlm_ocr");

    let text = response
        .choices
        .first()
        .and_then(|choice| choice.message.content.as_ref().and_then(|m| m.as_text()))
        .ok_or_else(|| crate::XbergError::ocr(format!("VLM OCR returned no content (model={})", config.model)))?;

    Ok((text, usage))
}

/// Normalize the VLM model id for the target endpoint.
///
/// liter-llm uses a leading `openai/` segment purely to route to its built-in
/// OpenAI provider; it is not part of the model id the API understands. When the
/// caller points at OpenAI's own REST endpoint via a custom `base_url` (e.g. a
/// regional `https://eu.api.openai.com/v1/`), that endpoint rejects the prefixed
/// form as an invalid model id (issue #1339). Strip the prefix only in that case.
///
/// The strip is keyed on the base_url targeting `openai.com` on purpose: gateways
/// such as OpenRouter or Together are also reached through a custom `base_url` but
/// require an `org/model` id verbatim, so they must never have a prefix stripped.
/// With no `base_url` the prefix is what routes liter-llm, so it is left intact.
fn normalize_vlm_model(model: &str, base_url: Option<&str>) -> String {
    if let Some(url) = base_url
        && url.contains("openai.com")
        && let Some(stripped) = model.strip_prefix("openai/")
    {
        return stripped.to_string();
    }
    model.to_string()
}

#[cfg(test)]
mod tests {

    fn render_ocr_prompt(language: &str) -> String {
        let ctx = minijinja::context! { language => language };
        super::super::prompts::render_template(super::super::prompts::VLM_OCR_TEMPLATE, &ctx).unwrap()
    }

    #[test]
    fn test_vlm_ocr_prompt_non_english_includes_language() {
        let prompt = render_ocr_prompt("deu");
        assert!(prompt.contains("language: deu"));
    }

    #[test]
    fn test_vlm_ocr_prompt_english_no_language_hint() {
        let prompt = render_ocr_prompt("eng");
        assert!(!prompt.contains("language:"));
    }

    #[test]
    fn test_vlm_ocr_prompt_en_no_language_hint() {
        let prompt = render_ocr_prompt("en");
        assert!(!prompt.contains("language:"));
    }

    /// Regression tests for issue #1339: an `openai/`-prefixed model must be sent
    /// bare to OpenAI's own custom endpoint, but left untouched for gateways and for
    /// the default (no base_url) provider-routed path.
    #[test]
    fn test_normalize_vlm_model_strips_prefix_for_openai_endpoint() {
        assert_eq!(
            super::normalize_vlm_model("openai/gpt-4o-mini", Some("https://eu.api.openai.com/v1/")),
            "gpt-4o-mini"
        );
    }

    #[test]
    fn test_normalize_vlm_model_keeps_prefix_without_base_url() {
        assert_eq!(
            super::normalize_vlm_model("openai/gpt-4o-mini", None),
            "openai/gpt-4o-mini"
        );
    }

    #[test]
    fn test_normalize_vlm_model_keeps_prefix_for_gateway_base_url() {
        assert_eq!(
            super::normalize_vlm_model("openai/gpt-4o", Some("https://openrouter.ai/api/v1")),
            "openai/gpt-4o"
        );
    }

    #[test]
    fn test_normalize_vlm_model_leaves_unprefixed_model_untouched() {
        assert_eq!(
            super::normalize_vlm_model("gpt-4o-mini", Some("https://eu.api.openai.com/v1/")),
            "gpt-4o-mini"
        );
    }

    /// Regression test for issue #1273: an unset VLM `timeout_secs` must not inherit
    /// liter-llm's 60s default, which is too short for full-page transcription.
    #[test]
    fn test_effective_vlm_config_applies_default_timeout_when_unset() {
        let config = crate::core::config::LlmConfig {
            model: "openai/gpt-4o".to_string(),
            ..Default::default()
        };
        assert!(config.timeout_secs.is_none());
        let effective = super::effective_vlm_config(&config);
        assert_eq!(effective.timeout_secs, Some(super::DEFAULT_VLM_TIMEOUT_SECS));
        assert_ne!(effective.timeout_secs, Some(60));
    }

    #[test]
    fn test_effective_vlm_config_preserves_explicit_timeout() {
        let config = crate::core::config::LlmConfig {
            model: "openai/gpt-4o".to_string(),
            timeout_secs: Some(1200),
            ..Default::default()
        };
        let effective = super::effective_vlm_config(&config);
        assert_eq!(effective.timeout_secs, Some(1200));
    }

    /// Regression test for issue #760: OcrConfig.vlm_prompt must be honoured.
    ///
    /// Before the fix, vlm_prompt was never passed to vlm_ocr() and the hardcoded
    /// VLM_OCR_TEMPLATE was always used instead.
    #[test]
    fn test_vlm_prompt_custom_template_is_used_issue_760() {
        let custom_prompt = "Extract all text from this document image. \
                             Preserve formatting and use latex for mathematical formulas.";

        let ctx = minijinja::context! { language => "eng" };
        let prompt = super::super::prompts::render_template(custom_prompt, &ctx).unwrap();

        assert!(prompt.contains("latex"), "custom prompt must be used; got: {prompt}");
        assert!(
            prompt.contains("Preserve formatting"),
            "custom prompt must be used; got: {prompt}"
        );
        assert!(
            !prompt.contains("Extract all visible text"),
            "default template must NOT be used when custom prompt is set; got: {prompt}"
        );
    }

    /// When vlm_prompt is None the built-in default template is used.
    #[test]
    fn test_vlm_prompt_none_falls_back_to_default() {
        let ctx = minijinja::context! { language => "eng" };
        let prompt = super::super::prompts::render_template(super::super::prompts::VLM_OCR_TEMPLATE, &ctx).unwrap();

        assert!(
            prompt.contains("Extract all visible text"),
            "default template must be used when vlm_prompt is None; got: {prompt}"
        );
    }
}
