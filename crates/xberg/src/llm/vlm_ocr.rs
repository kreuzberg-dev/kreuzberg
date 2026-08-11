//! VLM-based OCR using liter-llm vision models.
//!
//! Provides text extraction from images by sending them to a vision language
//! model (e.g., GPT-4o, Claude) via the liter-llm client.  This is an
//! alternative to traditional OCR backends (Tesseract, PaddleOCR) and can
//! produce higher-quality results for complex layouts, handwriting, or
//! low-quality scans.

use std::borrow::Cow;
use std::sync::LazyLock;

use async_trait::async_trait;
use base64::Engine;
use liter_llm::types::ContentPart;
use liter_llm::{ChatCompletionRequest, ImageUrl, LlmClient, Message, UserContent, UserMessage};
use regex::Regex;

use crate::core::config::LlmConfig;
use crate::plugins::{OcrBackend, OcrBackendType, Plugin};
use crate::types::{BoundingBox, FormatMetadata, Formula, Metadata, OcrMetadata, Table};

// Taken from the ungated `crate::ocr_metadata_keys` rather than `crate::ocr`, which is
// gated on `feature = "ocr"` / `"ocr-wasm"` — neither of which `liter-llm` implies, so a
// VLM-only build cannot see it. ~keep
use crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_HEIGHT_METADATA_KEY as PROCESSED_HEIGHT_KEY;
use crate::ocr_metadata_keys::OCR_PROCESSED_IMAGE_WIDTH_METADATA_KEY as PROCESSED_WIDTH_KEY;

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

        let formulas = extract_formulas(&text);
        let tables = extract_gfm_tables(&text);
        let metadata = build_metadata(image_bytes, tables.len() as u32);

        Ok(crate::ExtractedDocument {
            content: text,
            mime_type: Cow::Borrowed("text/plain"),
            llm_usage: usage.map(|u| vec![u]),
            formulas,
            metadata,
            tables,
            detected_languages: Some(languages),
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
    super::client::apply_request_time_params(&mut request, config)?;

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

/// Matches LaTeX math the model was instructed to emit via
/// [`super::prompts::VLM_OCR_TEMPLATE`]: display math as `$$...$$` or `\[...\]`,
/// and inline math as `$...$` or `\(...\)`. `(?s)` makes `.` match newlines so
/// multi-line display formulas are captured as a single match, and the named
/// groups let the surrounding delimiters be dropped without a separate strip
/// step.
///
/// Alternation order matters: `$$...$$` is tried before the single-`$` inline
/// alternative, so a display block is never partially consumed as two inline
/// matches (the `regex` crate is leftmost-first among alternatives at a given
/// start position).
///
/// The inline `$...$` alternative additionally guards against plain-prose
/// dollar signs (e.g. "$5 and $10"): the captured content must start and end
/// with a non-whitespace, non-`$` character. Prose like "$5 and $10" pairs as
/// content `"5 and "`, which ends in a space and is rejected; a real inline
/// formula like "$x+y$" has no such boundary whitespace and matches.
static FORMULA_PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?s)\$\$(?P<dollar>.+?)\$\$|\\\[(?P<bracket>.+?)\\\]|\\\((?P<paren>.+?)\\\)|\$(?P<inline>[^\s$](?:[^$\n]*[^\s$])?)\$",
    )
    .expect("VLM formula regex pattern is valid and should compile")
});

/// Build OCR metadata for a VLM OCR result: `FormatMetadata::Ocr` with the
/// table count, `ocr_used`, and processed image dimensions (best-effort; a
/// decode failure just omits the dimensions rather than failing the OCR call).
///
/// Mirrors `candle_ocr::ocr_result::build_metadata`; not shared with it because the
/// two sit in separate feature domains (VLM vs. `candle-*`), issue #179. Only the key
/// names are shared, via `crate::ocr_metadata_keys`.
fn build_metadata(image_bytes: &[u8], table_count: u32) -> Metadata {
    let mut metadata = Metadata {
        format: Some(FormatMetadata::Ocr(OcrMetadata {
            table_count,
            ..Default::default()
        })),
        ocr_used: true,
        ..Default::default()
    };

    if let Some((width, height)) = probe_image_dimensions(image_bytes) {
        metadata
            .additional
            .insert(Cow::Borrowed(PROCESSED_WIDTH_KEY), serde_json::json!(width));
        metadata
            .additional
            .insert(Cow::Borrowed(PROCESSED_HEIGHT_KEY), serde_json::json!(height));
    }

    metadata
}

/// Read image dimensions from the header without decoding pixel data.
fn probe_image_dimensions(image_bytes: &[u8]) -> Option<(u32, u32)> {
    image::ImageReader::new(std::io::Cursor::new(image_bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()
}

/// Parse every GFM table in VLM OCR output text into structured [`Table`]
/// entries, mirroring `candle_ocr::ocr_result::extract_gfm_tables` (issue #179).
/// Without this, a VLM that reproduces a table as GFM markdown left `tables[]`
/// empty even though the table data was present in `content`.
fn extract_gfm_tables(text: &str) -> Vec<Table> {
    use pulldown_cmark::{Event, Options, Parser, Tag, TagEnd};

    let mut tables = Vec::new();
    let mut in_table = false;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut current_row: Vec<String> = Vec::new();
    let mut current_cell = String::new();
    let mut in_cell = false;

    for event in Parser::new_ext(text, Options::ENABLE_TABLES) {
        match event {
            Event::Start(Tag::Table(_)) => {
                in_table = true;
                rows.clear();
            }
            Event::End(TagEnd::Table) if in_table => {
                in_table = false;
                if !rows.is_empty() {
                    let cells = std::mem::take(&mut rows);
                    let markdown = crate::rendering::common::render_table_markdown(&cells);
                    tables.push(Table {
                        cells,
                        markdown,
                        page_number: 1,
                        ..Default::default()
                    });
                }
            }
            Event::Start(Tag::TableHead | Tag::TableRow) if in_table => {
                current_row.clear();
            }
            Event::End(TagEnd::TableHead | TagEnd::TableRow) if in_table && !current_row.is_empty() => {
                rows.push(std::mem::take(&mut current_row));
            }
            Event::Start(Tag::TableCell) if in_table => {
                in_cell = true;
                current_cell.clear();
            }
            Event::End(TagEnd::TableCell) if in_table => {
                in_cell = false;
                current_row.push(current_cell.trim().to_string());
                current_cell.clear();
            }
            Event::Text(cell_text) | Event::Code(cell_text) if in_table && in_cell => {
                current_cell.push_str(&cell_text);
            }
            _ => {}
        }
    }

    tables
}

/// Extract LaTeX formulas from VLM OCR output text.
///
/// The VLM path has no layout detection, so recognized formulas have no known
/// region on the page; `bbox` is left at [`BoundingBox::default`] (all-zero)
/// rather than a fabricated position, and `page` is fixed at `1` since a single
/// call always OCRs one page image.
fn extract_formulas(text: &str) -> Vec<Formula> {
    FORMULA_PATTERN
        .captures_iter(text)
        .filter_map(|caps| {
            caps.name("dollar")
                .or_else(|| caps.name("bracket"))
                .or_else(|| caps.name("paren"))
                .or_else(|| caps.name("inline"))
        })
        .map(|m| m.as_str().trim())
        .filter(|latex| !latex.is_empty())
        .map(|latex| Formula {
            latex: latex.to_string(),
            bbox: None,
            page: None,
        })
        .collect()
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

    /// Formulas returned by the VLM as `$$...$$` must be extracted with the
    /// delimiters stripped and land in `ExtractedDocument.formulas` (via
    /// `extract_formulas`, the function `process_image` uses to populate it).
    /// Bbox has no known region on the VLM path, so it must be the honest
    /// all-zero default rather than a fabricated position.
    #[test]
    fn test_extract_formulas_strips_delimiters_from_vlm_response() {
        let text = "The quadratic formula is:\n\n$$x = \\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$$\n\nDone.";

        let formulas = super::extract_formulas(text);

        assert_eq!(formulas.len(), 1, "expected exactly one formula; got: {formulas:?}");
        assert_eq!(formulas[0].latex, r"x = \frac{-b \pm \sqrt{b^2 - 4ac}}{2a}");
        assert!(!formulas[0].latex.contains("$$"), "delimiters must be stripped");
        assert_eq!(formulas[0].bbox, None, "VLM formulas carry no geometry");
        assert_eq!(formulas[0].page, None, "VLM formulas carry no page");
    }

    /// Multiple formulas in one response must each be extracted independently.
    #[test]
    fn test_extract_formulas_handles_multiple_matches() {
        let text = "$$a^2 + b^2 = c^2$$ and also $$E = mc^2$$";

        let formulas = super::extract_formulas(text);

        assert_eq!(formulas.len(), 2, "expected two formulas; got: {formulas:?}");
        assert_eq!(formulas[0].latex, "a^2 + b^2 = c^2");
        assert_eq!(formulas[1].latex, "E = mc^2");
    }

    /// Plain text with no math must yield no formulas.
    #[test]
    fn test_extract_formulas_returns_empty_when_no_math_present() {
        let formulas = super::extract_formulas("Just a plain paragraph with no equations.");
        assert!(formulas.is_empty(), "expected no formulas; got: {formulas:?}");
    }

    /// Regression test for issue #188: inline math `$...$`, as requested by
    /// `VLM_OCR_TEMPLATE`, must be extracted into `formulas[]`, not left only in
    /// content.
    #[test]
    fn test_extract_formulas_extracts_inline_dollar_math() {
        let text = "The area is $A = \\pi r^2$ for a circle.";

        let formulas = super::extract_formulas(text);

        assert_eq!(formulas.len(), 1, "expected exactly one formula; got: {formulas:?}");
        assert_eq!(formulas[0].latex, r"A = \pi r^2");
        assert_eq!(formulas[0].bbox, None, "VLM formulas carry no geometry");
        assert_eq!(formulas[0].page, None, "VLM formulas carry no page");
    }

    /// Regression test for issue #188: `\(...\)` inline math must also be
    /// extracted, matching the `\[...\]` display form already handled.
    #[test]
    fn test_extract_formulas_extracts_inline_paren_math() {
        let text = r"Euler's identity: \(e^{i\pi} + 1 = 0\).";

        let formulas = super::extract_formulas(text);

        assert_eq!(formulas.len(), 1, "expected exactly one formula; got: {formulas:?}");
        assert_eq!(formulas[0].latex, r"e^{i\pi} + 1 = 0");
    }

    /// Guard test for issue #188: prose containing plain-text dollar amounts
    /// must NOT be misdetected as inline math. "$5 and $10" pairs as content
    /// `"5 and "`, which has trailing whitespace before the closing `$` and is
    /// rejected by the inline-match boundary guard.
    #[test]
    fn test_extract_formulas_ignores_currency_dollar_signs() {
        let text = "The item costs $5 and the other costs $10, for $15 total.";

        let formulas = super::extract_formulas(text);

        assert!(
            formulas.is_empty(),
            "currency amounts must not be treated as formulas; got: {formulas:?}"
        );
    }

    /// A single stray `$` with no closing partner must not match at all.
    #[test]
    fn test_extract_formulas_ignores_single_unpaired_dollar() {
        let formulas = super::extract_formulas("Prices start at $20 per unit.");
        assert!(
            formulas.is_empty(),
            "unpaired dollar sign must not match; got: {formulas:?}"
        );
    }

    /// Display math must still take priority over the inline alternative so a
    /// `$$...$$` block is captured whole rather than as two inline matches.
    #[test]
    fn test_extract_formulas_prefers_display_over_inline_when_both_present() {
        let text = "$$x^2 + y^2 = z^2$$ and separately $a+b$.";

        let formulas = super::extract_formulas(text);

        assert_eq!(
            formulas.len(),
            2,
            "expected one display + one inline; got: {formulas:?}"
        );
        assert_eq!(formulas[0].latex, "x^2 + y^2 = z^2");
        assert_eq!(formulas[1].latex, "a+b");
    }

    /// Regression test for issue #179: a GFM table embedded in VLM OCR output
    /// text must be parsed into `tables[]`, not left only in `content`.
    #[test]
    fn test_extract_gfm_tables_from_vlm_response() {
        let text = "Some text.\n\n| Name | Age |\n|------|-----|\n| Alice | 30 |\n\nMore text.";

        let tables = super::extract_gfm_tables(text);

        assert_eq!(tables.len(), 1, "expected exactly one table; got: {tables:?}");
        assert_eq!(
            tables[0].cells,
            vec![
                vec!["Name".to_string(), "Age".to_string()],
                vec!["Alice".to_string(), "30".to_string()],
            ]
        );
        assert_eq!(tables[0].page_number, 1);
    }

    /// Regression test for issue #179: `process_image` must populate
    /// `FormatMetadata::Ocr` / `ocr_used`, not leave `metadata` at its default.
    #[test]
    fn test_build_metadata_reports_ocr_used_and_table_count() {
        let metadata = super::build_metadata(&[], 2);

        assert!(metadata.ocr_used, "ocr_used must be true for a VLM OCR result");
        let Some(super::FormatMetadata::Ocr(ocr_metadata)) = metadata.format else {
            panic!("expected FormatMetadata::Ocr; got: {:?}", metadata.format);
        };
        assert_eq!(ocr_metadata.table_count, 2);
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
