//! Subprocess-based adapter for language bindings
//!
//! This adapter provides a base for running extraction via subprocess.
//! It's used by Python, Node.js, and Ruby adapters to execute extraction
//! in separate processes while monitoring resource usage.

use crate::adapter::FrameworkAdapter;
use crate::monitoring::{ResourceMonitor, ResourceStats};
use crate::types::{
    BatchCapability, BatchEntryPoint, BenchmarkResult, ErrorKind, FrameworkCapabilities, OcrStatus, OutputFormat,
    PerformanceMetrics,
};
use crate::{Error, Result};
use async_trait::async_trait;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
#[cfg(unix)]
use tokio::io::AsyncWriteExt;

struct MeasuredCommandOutcome {
    output: Option<std::process::Output>,
    duration: Duration,
    resource_stats: ResourceStats,
    error: Option<Error>,
}

struct SubprocessExecution {
    stdout: String,
    duration: Duration,
    resource_stats: ResourceStats,
    error: Option<Error>,
}

/// Extract JSON content from raw stdout, stripping non-JSON prefix lines.
///
/// Some runtimes (notably Elixir's BEAM VM) emit log messages to stdout
/// during module initialization before the script can redirect them. This
/// function finds the earliest `[` or `{` character and returns everything
/// from that point, ignoring any preceding log lines. Whichever delimiter
/// appears first wins — must not bias toward `[` because object outputs
/// (e.g. xberg-cli's envelope) contain nested arrays.
fn extract_json_from_stdout(raw: &str) -> &str {
    let bracket = raw.find('[');
    let brace = raw.find('{');
    let pos = match (bracket, brace) {
        (Some(b), Some(c)) => Some(b.min(c)),
        (Some(b), None) => Some(b),
        (None, Some(c)) => Some(c),
        (None, None) => None,
    };
    match pos {
        Some(p) => &raw[p..],
        None => raw,
    }
}

/// Marker printed by our extraction scripts (e.g. `docling_extract.py`,
/// `markitdown_extract.py`) to stderr when the *framework itself* raises during
/// extraction: `print(f"Error extracting with {Framework}: {e}", file=sys.stderr)`
/// followed by a non-zero exit. This is a framework-side crash, not ours — see
/// [`error_to_error_kind`].
const FRAMEWORK_CRASH_STDERR_MARKER: &str = "error extracting with";

/// Map a harness `Error` to the appropriate `ErrorKind`.
///
/// Detects config/setup errors (missing dependencies, environment issues) vs
/// actual harness infrastructure failures vs framework-side crashes.
///
/// Subprocess non-zero exits are wrapped as `Error::Benchmark` regardless of
/// *why* the subprocess died, so the message text (which embeds captured
/// stderr — see `execute_subprocess`/`execute_subprocess_batch`) is inspected
/// here to distinguish three cases:
/// 1. The framework crashed while extracting (our extraction scripts print
///    `"Error extracting with {Framework}: ..."` to stderr before exiting
///    non-zero) → `FrameworkError`, not our fault.
/// 2. A missing dependency/model/library (config/setup issue) → `ConfigSetupError`.
/// 3. Anything else (spawn failure, our own panics, unexpected subprocess death)
///    → `HarnessError`, potentially our fault.
fn error_to_error_kind(e: &Error) -> ErrorKind {
    match e {
        Error::Timeout(_) => ErrorKind::Timeout,
        Error::FrameworkError(_) => ErrorKind::FrameworkError,
        Error::EmptyContent(_) => ErrorKind::EmptyContent,
        Error::Benchmark(msg) | Error::Config(msg) => {
            let msg_lower = msg.to_lowercase();

            if (msg_lower.contains("torch.") && msg_lower.contains("not found"))
                || (msg_lower.contains("partition_") && msg_lower.contains("not available"))
                || msg_lower.contains("tessdata")
                || (msg_lower.contains("tesseract") && msg_lower.contains("not found"))
                || (msg_lower.contains("module")
                    && (msg_lower.contains("not found") || msg_lower.contains("not installed")))
                || msg_lower.contains("import error")
                || msg_lower.contains("importerror")
                || (msg_lower.contains("no such file")
                    && (msg_lower.contains(".so") || msg_lower.contains(".dylib") || msg_lower.contains(".dll")))
                || (msg_lower.contains("failed to find")
                    && (msg_lower.contains("model") || msg_lower.contains("library")))
            {
                ErrorKind::ConfigSetupError
            } else if msg_lower.contains(FRAMEWORK_CRASH_STDERR_MARKER) {
                ErrorKind::FrameworkError
            } else {
                ErrorKind::HarnessError
            }
        }
        _ => ErrorKind::HarnessError,
    }
}
use tokio::process::Command;

/// Minimum duration in seconds for a valid throughput calculation.
/// Durations below this threshold produce unreliable throughput values
/// and will result in throughput being set to 0.0 (filtered in aggregation).
const MIN_VALID_DURATION_SECS: f64 = 0.000_001;

fn bytes_per_second(bytes: u64, duration: Duration) -> f64 {
    if duration.as_secs_f64() >= MIN_VALID_DURATION_SECS {
        bytes as f64 / duration.as_secs_f64()
    } else {
        0.0
    }
}

/// Detect a PDF's page count using the harness-side (framework-agnostic) `xberg` page counter.
///
/// This is intentionally independent of whatever a competing framework self-reports, so the
/// resulting `pages_per_sec` aggregate metric compares every framework against the same
/// ground truth. Returns `None` when the file cannot be read or does not parse as a PDF.
fn detect_pdf_page_count(path: &Path) -> Option<u32> {
    let bytes = std::fs::read(path).ok()?;
    xberg::pdf_page_count(&bytes, None).ok().map(|count| count as u32)
}

#[derive(Debug)]
struct ParsedBatchOutput {
    items: Vec<serde_json::Value>,
    reported_total_duration: Option<Duration>,
    per_file_durations: Vec<Option<Duration>>,
}

fn duration_from_ms(value: &serde_json::Value, field: &str) -> Result<Duration> {
    let milliseconds = value
        .as_f64()
        .filter(|milliseconds| milliseconds.is_finite() && *milliseconds >= 0.0)
        .ok_or_else(|| Error::Benchmark(format!("batch output field '{field}' must be a non-negative number")))?;
    Ok(Duration::from_secs_f64(milliseconds / 1000.0))
}

fn parse_batch_output(stdout: &str) -> Result<ParsedBatchOutput> {
    let raw: serde_json::Value = serde_json::from_str(stdout)
        .map_err(|error| Error::Benchmark(format!("Failed to parse batch output as JSON: {error}")))?;

    if let Some(results) = raw.get("results") {
        let items = results
            .as_array()
            .cloned()
            .ok_or_else(|| Error::Benchmark("batch output field 'results' must be an array".to_string()))?;
        let reported_total_duration = raw
            .get("total_ms")
            .ok_or_else(|| Error::Benchmark("batch envelope is missing required 'total_ms'".to_string()))
            .and_then(|value| duration_from_ms(value, "total_ms"))?;
        let per_file_values = raw
            .get("per_file_ms")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| Error::Benchmark("batch envelope is missing required 'per_file_ms' array".to_string()))?;
        let per_file_durations = per_file_values
            .iter()
            .enumerate()
            .map(|(index, value)| {
                if value.is_null() {
                    Ok(None)
                } else {
                    duration_from_ms(value, &format!("per_file_ms[{index}]")).map(Some)
                }
            })
            .collect::<Result<Vec<_>>>()?;

        return Ok(ParsedBatchOutput {
            items,
            reported_total_duration: Some(reported_total_duration),
            per_file_durations,
        });
    }

    let items = match raw {
        serde_json::Value::Array(items) => items,
        serde_json::Value::Object(_) => vec![raw],
        _ => {
            return Err(Error::Benchmark(
                "batch output must be a JSON array, object, or Xberg batch envelope".to_string(),
            ));
        }
    };
    let per_file_durations = items
        .iter()
        .map(|item| {
            item.get("_extraction_time_ms")
                .map(|value| duration_from_ms(value, "_extraction_time_ms"))
                .transpose()
        })
        .collect::<Result<Vec<_>>>()?;

    Ok(ParsedBatchOutput {
        items,
        reported_total_duration: None,
        per_file_durations,
    })
}

/// Check if verbose benchmark debugging is enabled via BENCHMARK_DEBUG env var.
fn is_debug_enabled() -> bool {
    std::env::var("BENCHMARK_DEBUG").is_ok()
}

const DOCLING_VERSION_PROBE: &str = r#"
import importlib.metadata as metadata
import sys

version = None
for distribution in ("docling", "docling-slim"):
    try:
        candidate = metadata.version(distribution).strip()
    except metadata.PackageNotFoundError:
        continue
    if candidate:
        version = candidate
        break

if version is None:
    try:
        import docling
    except ImportError:
        candidate = ""
    else:
        module_version = getattr(docling, "__version__", None)
        candidate = str(module_version).strip() if module_version is not None else ""
    if candidate:
        version = candidate

if version is None:
    sys.exit(1)

print(version)
"#;

const MINERU_VERSION_PROBE: &str = r#"
from importlib.metadata import PackageNotFoundError, version

try:
    print(version("mineru"))
except PackageNotFoundError:
    raise SystemExit(1)
"#;

fn first_output_line(output: std::process::Output) -> Option<String> {
    output
        .status
        .success()
        .then_some(output.stdout)
        .and_then(|stdout| String::from_utf8(stdout).ok())
        .and_then(|value| value.lines().next().map(str::trim).map(str::to_string))
        .filter(|value| !value.is_empty())
}

fn config_json_enables_ocr(args: &[String]) -> bool {
    args.windows(2)
        .rev()
        .find(|pair| pair[0] == "--config-json")
        .and_then(|pair| serde_json::from_str::<serde_json::Value>(&pair[1]).ok())
        .and_then(|config| config.pointer("/ocr/enabled").and_then(serde_json::Value::as_bool))
        == Some(true)
}

fn effective_ocr_config_from_args(args: &[String]) -> Option<serde_json::Value> {
    let cli_backend = args
        .windows(2)
        .rev()
        .find(|pair| pair[0] == "--ocr-backend")
        .map(|pair| pair[1].as_str());
    let cli_enabled = args.iter().enumerate().rev().find_map(|(index, arg)| {
        if arg == "--no-ocr" {
            return Some(false);
        }
        (arg == "--ocr").then(|| args.get(index + 1).is_none_or(|value| value != "false"))
    });
    if cli_enabled == Some(false) {
        return None;
    }

    let configured_ocr = args
        .windows(2)
        .rev()
        .find(|pair| pair[0] == "--config-json")
        .and_then(|pair| serde_json::from_str::<serde_json::Value>(&pair[1]).ok())
        .and_then(|config| config.get("ocr").cloned());
    if let Some(mut ocr) = configured_ocr {
        let object = ocr.as_object_mut()?;
        if cli_enabled != Some(true) && object.get("enabled").and_then(serde_json::Value::as_bool) == Some(false) {
            return None;
        }
        if cli_enabled == Some(true) {
            object.insert("enabled".to_string(), serde_json::Value::Bool(true));
        }
        if let Some(cli_backend) = cli_backend {
            object.insert(
                "backend".to_string(),
                serde_json::Value::String(cli_backend.to_string()),
            );
        }
        return Some(ocr);
    }

    (cli_enabled == Some(true)).then(|| match cli_backend {
        Some("tesseract") | None => serde_json::json!({
            "enabled": true,
            "backend": "tesseract"
        }),
        Some(backend) => serde_json::json!({
            "enabled": true,
            "backend": backend
        }),
    })
}

/// True if a JSON `ocr` object's backend — or any stage of a multi-stage `ocr.pipeline` — is
/// `"tesseract"`. Used to gate the PSM-aware Tesseract result-cache materialization below: only
/// Tesseract has an independent on-disk OCR result cache and PSM auto-selection to preserve.
fn ocr_uses_tesseract(ocr_object: &serde_json::Map<String, serde_json::Value>) -> bool {
    if ocr_object.get("backend").and_then(serde_json::Value::as_str) == Some("tesseract") {
        return true;
    }
    ocr_object
        .get("pipeline")
        .and_then(|pipeline| pipeline.get("stages"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|stages| {
            stages.iter().any(|stage| {
                stage
                    .as_object()
                    .and_then(|stage_object| stage_object.get("backend"))
                    .and_then(serde_json::Value::as_str)
                    == Some("tesseract")
            })
        })
}

/// Sets `languages` on a JSON `ocr` object (and any Tesseract stage of a multi-stage
/// `ocr.pipeline`), and ensures every Tesseract config's result cache is genuinely disabled.
///
/// A `tesseract_config` that already exists (an explicit PSM preset) only has its `language` and
/// `use_cache` refreshed — its `psm` is left untouched. A `tesseract_config` that is absent is
/// materialized with `use_cache: false` and the PSM `apply_default_whole_image_tesseract_psm` in
/// `crates/xberg/src/extractors/image.rs` would itself have picked for `languages` (PSM 11, or
/// PSM 5 for a `*_vert` language) — never a bare `{"use_cache": false}`, which would deserialize
/// with the Tesseract PSM default (3) and silently defeat xberg's own auto-PSM selection. ~keep
fn materialize_tesseract_ocr(ocr_object: &mut serde_json::Map<String, serde_json::Value>, languages: &[String]) {
    ocr_object.insert("language".to_string(), serde_json::json!(languages));

    if ocr_object.get("backend").and_then(serde_json::Value::as_str) == Some("tesseract") {
        apply_tesseract_result_cache_control(ocr_object, languages);
    }

    if let Some(stages) = ocr_object
        .get_mut("pipeline")
        .and_then(|pipeline| pipeline.get_mut("stages"))
        .and_then(serde_json::Value::as_array_mut)
    {
        for stage in stages {
            let Some(stage_object) = stage.as_object_mut() else {
                continue;
            };
            if stage_object.get("backend").and_then(serde_json::Value::as_str) != Some("tesseract") {
                continue;
            }
            stage_object.insert("language".to_string(), serde_json::json!(languages));
            apply_tesseract_result_cache_control(stage_object, languages);
        }
    }
}

fn apply_tesseract_result_cache_control(object: &mut serde_json::Map<String, serde_json::Value>, languages: &[String]) {
    match object
        .get_mut("tesseract_config")
        .and_then(serde_json::Value::as_object_mut)
    {
        Some(tesseract_config) => {
            tesseract_config.insert("language".to_string(), serde_json::json!(languages));
            tesseract_config.insert("use_cache".to_string(), serde_json::json!(false));
        }
        None => {
            let psm = crate::adapter::xberg_default_tesseract_psm(languages);
            object.insert(
                "tesseract_config".to_string(),
                serde_json::json!({ "use_cache": false, "psm": psm, "language": languages }),
            );
        }
    }
}

/// Resolves the effective language list for a Tesseract benchmark call: the fixture's
/// canonicalized `ocr_language` when present, else `["eng"]` — xberg's own default (see
/// `default_eng` in `crates/xberg/src/core/config/ocr.rs`) — so every Tesseract call gets a
/// concrete language to compute the matching auto-PSM from, not just fixtures that pin one.
fn effective_tesseract_languages(ocr_language: Option<&str>) -> Vec<String> {
    ocr_language
        .map(crate::adapter::canonicalize_ocr_languages)
        .filter(|languages| !languages.is_empty())
        .unwrap_or_else(|| vec!["eng".to_string()])
}

/// Rewrites the request's effective OCR config so a Tesseract benchmark subprocess gets a
/// genuinely cold OCR result cache without regressing PSM (see [`materialize_tesseract_ocr`]).
///
/// This is the final request-args boundary: it starts from [`effective_ocr_config_from_args`],
/// which already merges an `ocr` key that a `--config-json` value may carry with any CLI-only
/// `--ocr`/`--ocr-backend`/`--no-ocr` override — including the case where OCR is enabled purely
/// via a CLI flag (e.g. `request_args_from`'s force-OCR upgrade path) and the base
/// `--config-json` carries no `ocr` key at all. Whatever that merge produces is where the
/// materialized Tesseract config gets written back to, via [`inject_ocr_config_into_args`] —
/// creating a `--config-json` flag if none existed — so this always applies, not just when a
/// `tesseract_config`-carrying `--config-json` was already present.
///
/// Applies unconditionally, even when `ocr_language` is `None` (defaults to `"eng"`), because
/// every Tesseract benchmark call needs its result cache disabled, not just fixtures that pin an
/// explicit language. Returns `None` when the args carry no enabled Tesseract OCR config to
/// rewrite (non-tesseract backend or OCR disabled) — callers must fall back to
/// [`xberg_ocr_language_args`] for those. ~keep
fn apply_tesseract_ocr_override_to_args(args: &[String], ocr_language: Option<&str>) -> Option<Vec<String>> {
    let mut effective_ocr = effective_ocr_config_from_args(args)?;
    let ocr_object = effective_ocr.as_object_mut()?;
    if ocr_object.get("enabled").and_then(serde_json::Value::as_bool) != Some(true) || !ocr_uses_tesseract(ocr_object) {
        return None;
    }

    let languages = effective_tesseract_languages(ocr_language);
    materialize_tesseract_ocr(ocr_object, &languages);

    inject_ocr_config_into_args(args, effective_ocr)
}

/// Writes `ocr` into the request's `--config-json` value, replacing any `ocr` key it already
/// carries so the materialized cache/PSM/language settings win. Creates a `--config-json` flag
/// (`{"ocr": ocr}`) when the request has none — the CLI-only OCR-enable path (no pre-existing
/// `--config-json`) still needs the result cache disabled and PSM set. Returns `None` (leaving
/// the request untouched) if an existing `--config-json` value fails to parse as a JSON object,
/// rather than risk clobbering an unparseable-but-intentional value.
fn inject_ocr_config_into_args(args: &[String], ocr: serde_json::Value) -> Option<Vec<String>> {
    let mut new_args = args.to_vec();
    if let Some(config_index) = new_args.iter().rposition(|arg| arg == "--config-json") {
        let raw = new_args.get(config_index + 1)?;
        let mut config: serde_json::Value = serde_json::from_str(raw).ok()?;
        config.as_object_mut()?.insert("ocr".to_string(), ocr);
        new_args[config_index + 1] = config.to_string();
    } else {
        new_args.push("--config-json".to_string());
        new_args.push(serde_json::json!({ "ocr": ocr }).to_string());
    }
    Some(new_args)
}

fn xberg_ocr_language_args(args: &[String], ocr_language: Option<&str>) -> Option<[String; 2]> {
    effective_ocr_config_from_args(args)?;
    let language = ocr_language.and_then(crate::adapter::canonical_ocr_language_arg)?;
    Some(["--ocr-language".to_string(), language])
}

fn build_batch_file_configs(
    file_paths: &[&Path],
    ocr_languages: &[Option<String>],
    cwd: &Path,
    base_ocr: Option<&serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    let Some(base_ocr) = base_ocr else {
        return serde_json::Map::new();
    };
    let Some(base_ocr_object) = base_ocr.as_object() else {
        return serde_json::Map::new();
    };
    let uses_tesseract = ocr_uses_tesseract(base_ocr_object);

    file_paths
        .iter()
        .zip(ocr_languages)
        .filter_map(|(path, language)| {
            // Every Tesseract fixture needs a per-file override so its result cache is genuinely
            // disabled — even without an explicit fixture language (defaults to "eng"). A
            // non-Tesseract fixture without an explicit language needs no override at all: the
            // base `--config-json` already carries its (correct) default language.
            let languages = match language.as_deref().map(crate::adapter::canonicalize_ocr_languages) {
                Some(languages) if !languages.is_empty() => languages,
                _ if uses_tesseract => vec!["eng".to_string()],
                _ => return None,
            };
            let absolute_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            let mut ocr = base_ocr.clone();
            let ocr_object = ocr.as_object_mut()?;
            if uses_tesseract {
                materialize_tesseract_ocr(ocr_object, &languages);
            } else {
                ocr_object.insert("language".to_string(), serde_json::json!(languages));
            }
            Some((
                absolute_path.to_string_lossy().into_owned(),
                serde_json::json!({ "ocr": ocr }),
            ))
        })
        .collect()
}

/// Base adapter for subprocess-based extraction
///
/// This adapter spawns a subprocess to perform extraction and monitors
/// its resource usage. Subclasses implement the specific command construction
/// for each language binding.
pub struct SubprocessAdapter {
    name: String,
    command: PathBuf,
    args: Vec<String>,
    env: Vec<(String, String)>,
    batch_capability: Option<BatchCapability>,
    working_dir: Option<PathBuf>,
    supported_formats: Vec<String>,
    max_timeout: Option<Duration>,
    skip_files: Vec<String>,
    /// When true, append --format=<output_format> to subprocess args
    format_aware: bool,
    supported_output_formats: Vec<OutputFormat>,
    /// Single-file command arguments for adapters whose batch command uses a
    /// different subcommand. Used by warmup and mixed per-file OCR fallback.
    single_file_args: Option<Vec<String>>,
    /// OCR mode requested by an external adapter when its output does not
    /// report whether OCR ran. Xberg adapters leave this unset and use their
    /// emitted per-document metadata.
    configured_ocr_status: Option<OcrStatus>,
    /// Worker limit passed to native batch implementations.
    batch_workers: usize,
    /// Resolved executable used by a specialized native batch path.
    native_batch_command: Option<PathBuf>,
    /// Per-adapter sequence used to distinguish repeated batch invocations.
    batch_sequence: AtomicU64,
    /// Explicit `--max-threads` budget passed to Xberg in either mode.
    ///
    /// When unset, single-file mode preserves Xberg's automatic budget while
    /// native batch mode falls back to [`Self::batch_workers`].
    xberg_max_threads: Option<usize>,
    /// CLI flag an external wrapper uses to receive the fixture's OCR language,
    /// forwarded in canonical Tesseract form (e.g. `eng+kor`, `jpn_vert`). The
    /// wrapper maps it onto its own engine's codes. `None` means the framework
    /// exposes no explicit OCR-language selection, so the language is not
    /// forwarded and parity is not assumed on its behalf.
    ocr_language_arg: Option<String>,
    ocr_language_policy: crate::adapter::OcrLanguagePolicy,
}

impl SubprocessAdapter {
    /// Build request arguments, upgrading an adapter configured with OCR disabled
    /// when the fixture explicitly requires OCR.
    fn request_args_from(&self, base_args: &[String], force_ocr: bool) -> Vec<String> {
        let mut args = base_args.to_vec();
        if !force_ocr {
            return args;
        }

        let is_xberg = self.name.starts_with("xberg-");
        let has_cli_ocr_override = args.iter().any(|arg| matches!(arg.as_str(), "--ocr" | "--no-ocr"));
        let preserve_configured_xberg_ocr = is_xberg && !has_cli_ocr_override && config_json_enables_ocr(base_args);
        if !preserve_configured_xberg_ocr {
            if let Some(index) = args.iter().position(|arg| arg == "--no-ocr") {
                args[index] = "--ocr".to_string();
            } else if let Some(index) = args.iter().position(|arg| arg == "--ocr") {
                if let Some(value) = args.get_mut(index + 1)
                    && matches!(value.as_str(), "true" | "false")
                {
                    *value = "true".to_string();
                }
            } else {
                args.push("--ocr".to_string());
            }
        }

        if is_xberg {
            if let Some(index) = args.iter().position(|arg| arg == "--force-ocr") {
                if let Some(value) = args.get_mut(index + 1) {
                    *value = "true".to_string();
                }
            } else {
                args.extend(["--force-ocr".to_string(), "true".to_string()]);
            }
        }

        args
    }

    fn request_args(&self, force_ocr: bool) -> Vec<String> {
        self.request_args_from(&self.args, force_ocr)
    }

    fn is_xberg(&self) -> bool {
        self.name.starts_with("xberg-")
    }

    fn append_explicit_xberg_thread_budget(&self, args: &mut Vec<String>) {
        if self.is_xberg()
            && let Some(max_threads) = self.xberg_max_threads
        {
            args.extend(["--max-threads".to_string(), max_threads.to_string()]);
        }
    }

    fn single_file_request_args(&self, force_ocr: bool) -> Vec<String> {
        let mut args = self.request_args_from(self.single_file_args.as_deref().unwrap_or(&self.args), force_ocr);
        self.append_explicit_xberg_thread_budget(&mut args);
        args
    }

    fn provenance_args_for_mode(&self, mode: crate::config::BenchmarkMode) -> Vec<String> {
        let mut args = match mode {
            crate::config::BenchmarkMode::SingleFile => self.single_file_args.as_deref().unwrap_or(&self.args).to_vec(),
            crate::config::BenchmarkMode::Batch => self.args.clone(),
        };
        match mode {
            crate::config::BenchmarkMode::SingleFile => self.append_explicit_xberg_thread_budget(&mut args),
            crate::config::BenchmarkMode::Batch
                if self.is_xberg()
                    && self
                        .batch_capability
                        .is_some_and(|capability| capability.entry_point == BatchEntryPoint::XbergCliExtractBatch) =>
            {
                args.extend([
                    "--max-concurrent".to_string(),
                    self.batch_workers.to_string(),
                    "--max-threads".to_string(),
                    self.effective_xberg_max_threads().to_string(),
                ]);
            }
            crate::config::BenchmarkMode::Batch => {}
        }
        args
    }

    fn resolve_ocr_status(&self, value: Option<&serde_json::Value>, force_ocr: bool) -> OcrStatus {
        value
            .and_then(serde_json::Value::as_bool)
            .map(|used| if used { OcrStatus::Used } else { OcrStatus::NotUsed })
            .or_else(|| force_ocr.then_some(OcrStatus::Used))
            .or(self.configured_ocr_status)
            .unwrap_or(OcrStatus::Unknown)
    }

    fn timeout_error(operation: &str, timeout: Duration, reaped: Option<&std::process::Output>) -> Error {
        #[cfg(windows)]
        let cleanup = "; Windows timeout cleanup terminates the direct child only; descendant cleanup is unsupported";
        #[cfg(not(windows))]
        let cleanup = "";
        let mut message = format!("{operation} exceeded {timeout:?}{cleanup}");
        if let Some(output) = reaped {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stderr_tail = Self::tail_chars(stderr.trim_end(), 2000);
            if !stderr_tail.is_empty() {
                message.push_str(&format!("\nlast subprocess stderr (tail):\n{stderr_tail}"));
            }
            let stdout = String::from_utf8_lossy(&output.stdout);
            let stdout_tail = Self::tail_chars(stdout.trim_end(), 500);
            if !stdout_tail.is_empty() {
                message.push_str(&format!("\nlast subprocess stdout (tail):\n{stdout_tail}"));
            }
        }
        Error::Timeout(message)
    }

    /// Return the last `max` bytes of `s`, prefixed with `…` when truncated, snapped to a char boundary.
    fn tail_chars(s: &str, max: usize) -> String {
        if s.len() <= max {
            return s.to_string();
        }
        let mut cut = s.len() - max;
        while cut < s.len() && !s.is_char_boundary(cut) {
            cut += 1;
        }
        format!("…{}", &s[cut..])
    }

    fn measured_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
        #[cfg(unix)]
        {
            let mut command = Command::new("sh");
            command
                .arg("-c")
                .arg("IFS= read -r _ || exit 125; exec \"$@\"")
                .arg("xberg-benchmark-start-barrier")
                .arg(program);
            command
        }
        #[cfg(not(unix))]
        {
            Command::new(program)
        }
    }

    fn configure_measured_stdin(cmd: &mut Command) {
        #[cfg(unix)]
        cmd.stdin(Stdio::piped());
        #[cfg(not(unix))]
        cmd.stdin(Stdio::null());
    }

    async fn execute_measured_command(
        cmd: &mut Command,
        timeout: Duration,
        operation: &str,
        sample_interval: Duration,
    ) -> Result<MeasuredCommandOutcome> {
        #[cfg(not(unix))]
        let start = Instant::now();
        #[cfg(not(unix))]
        let deadline = start + timeout;
        let child = cmd
            .spawn()
            .map_err(|error| Error::Benchmark(format!("Failed to spawn {operation}: {error}")))?;
        let child_pid = child.id();
        #[cfg(unix)]
        let (child, mut start_barrier) = {
            let mut child = child;
            let start_barrier = child.stdin.take();
            (child, start_barrier)
        };
        let monitor = child_pid.map(ResourceMonitor::new_for_pid);
        if let Some(monitor) = &monitor {
            #[cfg(unix)]
            monitor.prepare().await;
            #[cfg(not(unix))]
            monitor.start(sample_interval).await;
        }
        #[cfg(unix)]
        let start = Instant::now();
        #[cfg(unix)]
        let barrier_error = match start_barrier.take() {
            Some(mut barrier) => match barrier.write_all(b"start\n").await {
                Ok(()) => barrier.shutdown().await.err(),
                Err(error) => Some(error),
            }
            .map(|error| Error::Benchmark(format!("Failed to release {operation} start barrier: {error}"))),
            None => Some(Error::Benchmark(format!("Failed to open {operation} start barrier"))),
        };
        #[cfg(not(unix))]
        let barrier_error: Option<Error> = None;
        #[cfg(unix)]
        if barrier_error.is_none()
            && let Some(monitor) = &monitor
        {
            monitor.activate(sample_interval).await;
        }
        #[cfg(unix)]
        let wait_timeout = timeout;
        #[cfg(not(unix))]
        let wait_timeout = deadline.saturating_duration_since(Instant::now());
        let mut wait = Box::pin(child.wait_with_output());
        let (output, error, duration) = if let Some(error) = barrier_error {
            #[cfg(unix)]
            Self::kill_process_group(child_pid);
            let _ = wait.await;
            (None, Some(error), start.elapsed())
        } else {
            match tokio::time::timeout(wait_timeout, &mut wait).await {
                Ok(Ok(output)) => (Some(output), None, start.elapsed()),
                Ok(Err(error)) => (
                    None,
                    Some(Error::Benchmark(format!("Failed to wait for {operation}: {error}"))),
                    start.elapsed(),
                ),
                Err(_) => {
                    let duration = start.elapsed();
                    // Capture the reaped child output so the hung subprocess's last
                    // stderr/stdout is surfaced in the timeout error (CI self-diagnosis).
                    #[cfg(unix)]
                    let reaped = {
                        Self::kill_process_group(child_pid);
                        wait.await.ok()
                    };
                    #[cfg(not(unix))]
                    let reaped: Option<std::process::Output> = None;
                    (
                        None,
                        Some(Self::timeout_error(operation, timeout, reaped.as_ref())),
                        duration,
                    )
                }
            }
        };
        let resource_stats = if let Some(monitor) = monitor {
            let samples = monitor.stop().await;
            let snapshots = monitor.get_snapshots().await;
            let baseline = monitor.baseline_memory().await;
            ResourceMonitor::calculate_stats(&samples, &snapshots, baseline)
        } else {
            ResourceStats::default()
        };
        let error = if child_pid.is_some() && resource_stats.sample_count == 0 && error.is_none() {
            Some(Error::Benchmark(format!(
                "{operation} completed before RSS monitoring captured a target sample; result is not measurable on this platform"
            )))
        } else {
            error
        };
        Ok(MeasuredCommandOutcome {
            output,
            duration,
            resource_stats,
            error,
        })
    }

    fn finish_measured_command(measured: MeasuredCommandOutcome, operation: &str) -> SubprocessExecution {
        let mut error = measured.error;
        let stdout = measured.output.map_or_else(String::new, |output| {
            let raw_stdout = String::from_utf8_lossy(&output.stdout);
            let stdout = extract_json_from_stdout(&raw_stdout).to_string();
            let stderr = String::from_utf8_lossy(&output.stderr).to_string();
            if !output.status.success() {
                let mut message = format!("{operation} failed with exit code {:?}", output.status.code());
                if !stderr.is_empty() {
                    message.push_str(&format!("\nstderr: {stderr}"));
                }
                if !stdout.is_empty() && stdout.len() < 500 {
                    message.push_str(&format!("\nstdout: {stdout}"));
                }
                error = Some(Error::Benchmark(message));
            }
            stdout
        });

        SubprocessExecution {
            stdout,
            duration: measured.duration,
            resource_stats: measured.resource_stats,
            error,
        }
    }

    fn configure_child_process(cmd: &mut Command) {
        cmd.kill_on_drop(true);
        #[cfg(unix)]
        cmd.process_group(0);
    }

    #[cfg(unix)]
    fn kill_process_group(pid: Option<u32>) {
        if let Some(pid) = pid {
            // SAFETY: the child was placed in a process group whose id equals its ~keep
            // pid. A negative pid targets only that group, never the harness.
            unsafe {
                libc::kill(-(pid as libc::pid_t), libc::SIGKILL);
            }
        }
    }

    /// Determine if a framework supports OCR based on its name
    ///
    /// Known frameworks with OCR support:
    /// - xberg-* (all Xberg bindings support OCR)
    /// - pymupdf (supports OCR via tesseract)
    ///
    /// Frameworks without OCR support include other basic PDF parsers.
    fn framework_supports_ocr(framework_name: &str) -> bool {
        let name_lower = framework_name.to_lowercase();

        if name_lower.starts_with("xberg-") || name_lower == "xberg" {
            return true;
        }

        if name_lower.contains("pymupdf") {
            return true;
        }

        if name_lower.contains("docling") {
            return true;
        }

        if name_lower.contains("unstructured") {
            return true;
        }

        if name_lower.contains("tika") {
            return true;
        }

        if name_lower.contains("mineru") {
            return true;
        }

        if name_lower.contains("liteparse") {
            return true;
        }

        false
    }

    /// Create a new subprocess adapter
    ///
    /// # Arguments
    /// * `name` - Framework name (e.g., "xberg-python")
    /// * `command` - Path to executable (e.g., "python3", "node")
    /// * `args` - Base arguments (e.g., ["-m", "xberg"])
    /// * `env` - Environment variables
    /// * `supported_formats` - List of file extensions this framework can process (e.g., ["pdf", "docx"])
    pub fn new(
        name: impl Into<String>,
        command: impl Into<PathBuf>,
        args: Vec<String>,
        env: Vec<(String, String)>,
        supported_formats: Vec<String>,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            env,
            batch_capability: None,
            working_dir: None,
            supported_formats,
            max_timeout: None,
            skip_files: vec![],
            format_aware: false,
            supported_output_formats: vec![OutputFormat::Markdown],
            single_file_args: None,
            configured_ocr_status: None,
            batch_workers: 1,
            native_batch_command: None,
            batch_sequence: AtomicU64::new(0),
            xberg_max_threads: None,
            ocr_language_arg: None,
            ocr_language_policy: crate::adapter::OcrLanguagePolicy::DefaultOnly,
        }
    }

    /// Create a new subprocess adapter with batch support
    ///
    /// This adapter will call `extract_batch()` with all files at once,
    /// allowing the subprocess to use its native batch API for parallel processing.
    ///
    /// # Arguments
    /// * `name` - Framework name (e.g., "xberg-python-batch")
    /// * `command` - Path to executable (e.g., "python3", "node")
    /// * `args` - Base arguments (e.g., ["-m", "xberg"])
    /// * `env` - Environment variables
    /// * `supported_formats` - List of file extensions this framework can process
    pub(crate) fn with_batch_capability(
        name: impl Into<String>,
        command: impl Into<PathBuf>,
        args: Vec<String>,
        env: Vec<(String, String)>,
        supported_formats: Vec<String>,
        batch_capability: BatchCapability,
    ) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            args,
            env,
            batch_capability: Some(batch_capability),
            working_dir: None,
            supported_formats,
            max_timeout: None,
            skip_files: vec![],
            format_aware: false,
            supported_output_formats: vec![OutputFormat::Markdown],
            single_file_args: None,
            configured_ocr_status: None,
            batch_workers: 1,
            native_batch_command: None,
            batch_sequence: AtomicU64::new(0),
            xberg_max_threads: None,
            ocr_language_arg: None,
            ocr_language_policy: crate::adapter::OcrLanguagePolicy::DefaultOnly,
        }
    }

    /// Set a maximum timeout for this adapter, overriding the global config timeout
    /// if the adapter's max is lower.
    pub fn with_max_timeout(mut self, timeout: Duration) -> Self {
        self.max_timeout = Some(timeout);
        self
    }

    /// Set files to skip for this adapter.
    pub fn with_skip_files(mut self, files: Vec<String>) -> Self {
        self.skip_files = files;
        self
    }

    /// Enable format awareness: append --format=<output_format> to subprocess args
    pub fn with_format_aware(mut self, enabled: bool) -> Self {
        self.format_aware = enabled;
        if enabled {
            self.supported_output_formats = vec![OutputFormat::Plaintext, OutputFormat::Markdown];
        }
        self
    }

    pub fn with_supported_output_formats(mut self, formats: Vec<OutputFormat>) -> Self {
        self.supported_output_formats = formats;
        self
    }

    pub fn with_single_file_args(mut self, args: Vec<String>) -> Self {
        self.single_file_args = Some(args);
        self
    }

    /// Record the OCR mode requested from an external framework. This is used
    /// only when the framework does not emit per-document OCR metadata.
    pub fn with_configured_ocr(mut self, enabled: bool) -> Self {
        self.configured_ocr_status = Some(if enabled { OcrStatus::Used } else { OcrStatus::NotUsed });
        self
    }

    /// Configure the CLI flag an external wrapper uses to receive the fixture's
    /// OCR language. Set this only for frameworks that expose explicit
    /// OCR-language selection; the forwarded value is the canonical Tesseract
    /// form (`eng+kor`, `jpn_vert`) and the wrapper maps it to its own engine.
    pub fn with_ocr_language_arg(mut self, flag: impl Into<String>) -> Self {
        self.ocr_language_arg = Some(flag.into());
        self.ocr_language_policy = crate::adapter::OcrLanguagePolicy::AnyPerDocument;
        self
    }

    pub fn with_ocr_language_policy(mut self, policy: crate::adapter::OcrLanguagePolicy) -> Self {
        self.ocr_language_policy = policy;
        self
    }

    /// Build the single-token `--flag=<language>` argument that forwards a
    /// fixture's OCR language to an external wrapper, or `None` when the adapter
    /// forwards no language (flag unconfigured or fixture pins none). Emitted as
    /// one token to match the wrappers' `--key=value` parsing and to avoid
    /// collision with the positional file path.
    fn ocr_language_forward_arg(&self, ocr_language: Option<&str>) -> Option<String> {
        let flag = self.ocr_language_arg.as_deref()?;
        let language = ocr_language.and_then(crate::adapter::canonical_ocr_language_arg)?;
        Some(format!("{flag}={language}"))
    }

    fn batch_ocr_language_forward_arg(&self, ocr_languages: &[Option<String>]) -> Result<Option<String>> {
        if !self.ocr_language_policy.requires_homogeneous_batch_language() {
            return Ok(None);
        }
        let Some(first) = ocr_languages.first() else {
            return Ok(None);
        };
        let expected = self.ocr_language_policy.partition_key(first.as_deref());
        if ocr_languages
            .iter()
            .any(|language| self.ocr_language_policy.partition_key(language.as_deref()) != expected)
        {
            return Err(Error::Config(format!(
                "framework '{}' received a native batch with mixed OCR languages",
                self.name
            )));
        }
        Ok(self.ocr_language_forward_arg(first.as_deref()))
    }

    /// Set the bounded worker count used by native batch implementations.
    pub fn with_batch_workers(mut self, workers: usize) -> Self {
        self.batch_workers = workers.max(1);
        self
    }

    pub(crate) fn with_native_batch_command(mut self, command: PathBuf) -> Self {
        self.native_batch_command = Some(command);
        self
    }

    /// Set Xberg's configured thread budget independently of batch workers.
    pub fn with_xberg_max_threads(mut self, max_threads: usize) -> Self {
        self.xberg_max_threads = Some(max_threads.max(1));
        self
    }

    fn effective_xberg_max_threads(&self) -> usize {
        self.xberg_max_threads.unwrap_or(self.batch_workers)
    }

    fn liteparse_batch_command(&self) -> &Path {
        self.native_batch_command.as_deref().unwrap_or_else(|| Path::new("lit"))
    }

    fn liteparse_batch_args(
        &self,
        input_dir: impl Into<String>,
        output_dir: impl Into<String>,
        output_format: impl Into<String>,
        disable_ocr: bool,
    ) -> Vec<String> {
        let mut args = vec![
            "batch-parse".to_string(),
            input_dir.into(),
            output_dir.into(),
            "--format".to_string(),
            output_format.into(),
            "--num-workers".to_string(),
            self.batch_workers.to_string(),
            "--quiet".to_string(),
        ];
        if disable_ocr {
            args.push("--no-ocr".to_string());
        }
        args
    }

    fn batch_sample_id(&self, file_paths: &[&Path], force_ocr: bool, output_format: OutputFormat) -> String {
        let mut hasher = blake3::Hasher::new();
        let sequence = self.batch_sequence.fetch_add(1, Ordering::Relaxed);
        let invocation_time = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        hasher.update(&std::process::id().to_le_bytes());
        hasher.update(&(std::ptr::from_ref(self).addr() as u64).to_le_bytes());
        hasher.update(&sequence.to_le_bytes());
        hasher.update(&invocation_time.to_le_bytes());
        hasher.update(&(self.batch_workers as u64).to_le_bytes());
        hasher.update(&(self.effective_xberg_max_threads() as u64).to_le_bytes());
        let output_format = output_format.to_string();
        let ocr_mode: &[u8] = match (force_ocr, self.configured_ocr_status) {
            (true, _) => b"force-ocr",
            (false, Some(OcrStatus::Used)) => b"configured-ocr-enabled",
            (false, Some(OcrStatus::NotUsed)) => b"configured-ocr-disabled",
            (false, _) => b"framework-reported-ocr",
        };
        let entry_point: &[u8] = match self.batch_capability.map(|capability| capability.entry_point) {
            Some(BatchEntryPoint::XbergCliExtractBatch) => b"xberg-cli-extract-batch",
            Some(BatchEntryPoint::DoclingJobkit) => b"docling-jobkit",
            Some(BatchEntryPoint::LiteparseBatchParse) => b"liteparse-batch-parse",
            Some(BatchEntryPoint::MineruDoParse) => b"mineru-do-parse",
            None => b"unverified",
        };
        for value in [self.name.as_bytes(), entry_point, output_format.as_bytes(), ocr_mode] {
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value);
        }
        for path in file_paths {
            let value = path.as_os_str().as_encoded_bytes();
            hasher.update(&(value.len() as u64).to_le_bytes());
            hasher.update(value);
        }
        hasher.finalize().to_hex().to_string()
    }

    /// Get the effective timeout, clamped by the adapter's max_timeout if set.
    fn effective_timeout(&self, timeout: Duration) -> Duration {
        match self.max_timeout {
            Some(max) => timeout.min(max),
            None => timeout,
        }
    }

    /// Set the working directory for subprocess execution
    ///
    /// # Arguments
    /// * `dir` - Directory path to change to before running the command
    pub fn set_working_dir(&mut self, dir: PathBuf) {
        self.working_dir = Some(dir);
    }

    /// Execute the extraction subprocess
    async fn execute_subprocess(
        &self,
        file_path: &Path,
        timeout: Duration,
        force_ocr: bool,
        ocr_language: Option<&str>,
        output_format: OutputFormat,
    ) -> Result<SubprocessExecution> {
        let absolute_path = if file_path.is_absolute() {
            file_path.to_path_buf()
        } else {
            std::env::current_dir().map_err(Error::Io)?.join(file_path)
        };

        let mut cmd = Self::measured_command(&self.command);
        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }
        let mut request_args = self.single_file_request_args(force_ocr);
        let mut tesseract_ocr_rewritten = false;
        if self.name.starts_with("xberg-")
            && let Some(rewritten) = apply_tesseract_ocr_override_to_args(&request_args, ocr_language)
        {
            request_args = rewritten;
            tesseract_ocr_rewritten = true;
        }
        cmd.args(&request_args);
        if !tesseract_ocr_rewritten
            && self.name.starts_with("xberg-")
            && let Some(language_args) = xberg_ocr_language_args(&request_args, ocr_language)
        {
            cmd.args(language_args);
        }
        if let Some(forward) = self.ocr_language_forward_arg(ocr_language) {
            cmd.arg(forward);
        }

        if self.format_aware {
            cmd.arg(format!("--format={}", output_format));
        }

        cmd.arg(&*absolute_path.to_string_lossy());

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        Self::configure_measured_stdin(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Self::configure_child_process(&mut cmd);

        let sampling_ms =
            crate::monitoring::adaptive_sampling_interval_ms(std::fs::metadata(file_path).map_err(Error::Io)?.len());
        let measured =
            Self::execute_measured_command(&mut cmd, timeout, "subprocess", Duration::from_millis(sampling_ms)).await?;
        Ok(Self::finish_measured_command(measured, "Subprocess"))
    }

    /// Execute batch extraction subprocess with multiple files
    async fn execute_subprocess_batch(
        &self,
        file_paths: &[&Path],
        timeout: Duration,
        force_ocr: bool,
        ocr_languages: &[Option<String>],
        output_format: OutputFormat,
    ) -> Result<SubprocessExecution> {
        if self
            .batch_capability
            .is_some_and(|capability| capability.entry_point == BatchEntryPoint::LiteparseBatchParse)
        {
            return self
                .execute_liteparse_native_batch(file_paths, timeout, force_ocr, output_format)
                .await;
        }

        let mut cmd = Self::measured_command(&self.command);
        if let Some(dir) = &self.working_dir {
            cmd.current_dir(dir);
        }
        let request_args = self.request_args(force_ocr);
        cmd.args(&request_args);
        if let Some(language_arg) = self.batch_ocr_language_forward_arg(ocr_languages)? {
            cmd.arg(language_arg);
        }

        let file_configs = if self
            .batch_capability
            .is_some_and(|capability| capability.entry_point == BatchEntryPoint::XbergCliExtractBatch)
        {
            let cwd = std::env::current_dir().map_err(Error::Io)?;
            let base_ocr = effective_ocr_config_from_args(&request_args);
            let configs = build_batch_file_configs(file_paths, ocr_languages, &cwd, base_ocr.as_ref());
            if configs.is_empty() {
                None
            } else {
                let mut file = tempfile::NamedTempFile::new().map_err(Error::Io)?;
                serde_json::to_writer(file.as_file_mut(), &configs)?;
                cmd.arg("--file-configs").arg(file.path());
                Some(file)
            }
        } else {
            None
        };

        if self
            .batch_capability
            .is_some_and(|capability| capability.entry_point == BatchEntryPoint::XbergCliExtractBatch)
        {
            let batch_workers = self.batch_workers.to_string();
            let max_threads = self.effective_xberg_max_threads().to_string();
            cmd.arg("--max-concurrent")
                .arg(batch_workers)
                .arg("--max-threads")
                .arg(max_threads);
        }

        if self.format_aware {
            cmd.arg(format!("--format={}", output_format));
        }

        let cwd = std::env::current_dir().map_err(Error::Io)?;
        for path in file_paths {
            let absolute_path = if path.is_absolute() {
                path.to_path_buf()
            } else {
                cwd.join(path)
            };
            cmd.arg(&*absolute_path.to_string_lossy());
        }

        for (key, value) in &self.env {
            cmd.env(key, value);
        }

        Self::configure_measured_stdin(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Self::configure_child_process(&mut cmd);

        let total_file_size = file_paths
            .iter()
            .filter_map(|path| std::fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        let sampling_ms = crate::monitoring::adaptive_sampling_interval_ms(total_file_size);
        let measured = Self::execute_measured_command(
            &mut cmd,
            timeout,
            "batch subprocess",
            Duration::from_millis(sampling_ms),
        )
        .await?;
        drop(file_configs);
        Ok(Self::finish_measured_command(measured, "Batch subprocess"))
    }

    fn stage_liteparse_input(source: &Path, destination: &Path) -> Result<()> {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(source, destination).map_err(|error| {
                Error::Benchmark(format!(
                    "Failed to stage LiteParse input {} at {} using a symlink: {}",
                    source.display(),
                    destination.display(),
                    error
                ))
            })
        }

        #[cfg(windows)]
        {
            if std::fs::hard_link(source, destination).is_ok() {
                return Ok(());
            }

            std::fs::copy(source, destination).map(|_| ()).map_err(|error| {
                Error::Benchmark(format!(
                    "Failed to stage LiteParse input {} at {} using a hard link or copy: {}",
                    source.display(),
                    destination.display(),
                    error
                ))
            })
        }

        #[cfg(not(any(unix, windows)))]
        {
            std::fs::copy(source, destination).map(|_| ()).map_err(|error| {
                Error::Benchmark(format!(
                    "Failed to stage LiteParse input {} at {} using a copy: {}",
                    source.display(),
                    destination.display(),
                    error
                ))
            })
        }
    }

    /// Execute liteparse native batch using lit batch-parse
    /// Uses lit batch-parse with temp directories for optimal apples-to-apples comparison
    async fn execute_liteparse_native_batch(
        &self,
        file_paths: &[&Path],
        timeout: Duration,
        force_ocr: bool,
        output_format: OutputFormat,
    ) -> Result<SubprocessExecution> {
        use std::fs;
        let temp_dir =
            tempfile::tempdir().map_err(|e| Error::Benchmark(format!("Failed to create temp directory: {}", e)))?;
        let input_dir = temp_dir.path().join("input");
        let output_dir = temp_dir.path().join("output");

        fs::create_dir(&input_dir).map_err(|e| Error::Benchmark(format!("Failed to create input directory: {}", e)))?;
        fs::create_dir(&output_dir)
            .map_err(|e| Error::Benchmark(format!("Failed to create output directory: {}", e)))?;

        for (idx, path) in file_paths.iter().enumerate() {
            let file_name = path
                .file_name()
                .ok_or_else(|| Error::Benchmark("Invalid file path".to_string()))?;

            let src_absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                std::env::current_dir().map_err(Error::Io)?.join(path)
            };

            let staged_name = format!("{}_{}", idx, file_name.to_string_lossy());
            let dest_link = input_dir.join(staged_name);
            Self::stage_liteparse_input(&src_absolute, &dest_link)?;
        }

        let format_arg = match output_format {
            OutputFormat::Markdown => "markdown",
            OutputFormat::Plaintext => "text",
        };
        let disable_ocr = !force_ocr && self.args.iter().any(|arg| arg == "--no-ocr");
        let args = self.liteparse_batch_args(
            input_dir.to_string_lossy().into_owned(),
            output_dir.to_string_lossy().into_owned(),
            format_arg,
            disable_ocr,
        );

        let mut cmd = Self::measured_command(self.liteparse_batch_command());
        cmd.args(args);

        Self::configure_measured_stdin(&mut cmd);
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        Self::configure_child_process(&mut cmd);
        // Staging is harness setup, not framework work. Start the measured
        // interval only after tempdir creation and input symlinks are complete. ~keep
        let total_file_size = file_paths
            .iter()
            .filter_map(|path| fs::metadata(path).ok())
            .map(|metadata| metadata.len())
            .sum();
        let sampling_ms = crate::monitoring::adaptive_sampling_interval_ms(total_file_size);
        let measured =
            Self::execute_measured_command(&mut cmd, timeout, "lit batch-parse", Duration::from_millis(sampling_ms))
                .await?;
        let mut execution = Self::finish_measured_command(measured, "lit batch-parse");
        if execution.error.is_some() {
            return Ok(execution);
        }

        let preferred_exts: [&str; 2] = match output_format {
            OutputFormat::Markdown => ["md", "markdown"],
            OutputFormat::Plaintext => ["txt", "text"],
        };
        let produced: Vec<(String, std::path::PathBuf)> = fs::read_dir(&output_dir)
            .map_err(|e| Error::Benchmark(format!("Failed to read lit output dir {}: {}", output_dir.display(), e)))?
            .filter_map(|entry| entry.ok())
            .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.path()))
            .collect();

        let mut results = Vec::new();
        for (idx, _path) in file_paths.iter().enumerate() {
            let prefix = format!("{idx}_");
            let matches: Vec<&(String, std::path::PathBuf)> =
                produced.iter().filter(|(name, _)| name.starts_with(&prefix)).collect();
            let hit = matches
                .iter()
                .find(|(name, _)| preferred_exts.iter().any(|e| name.ends_with(&format!(".{e}"))))
                .or_else(|| matches.first());

            match hit {
                Some((_, output_path)) => {
                    let content = fs::read_to_string(output_path).map_err(|e| {
                        Error::Benchmark(format!("Failed to read lit output {}: {}", output_path.display(), e))
                    })?;
                    results.push(serde_json::json!({
                        "content": content,
                        "metadata": {
                            "framework": "liteparse",
                            "output_format": output_format.to_string()
                        }
                    }));
                }
                None => {
                    let listing: Vec<&String> = produced.iter().map(|(name, _)| name).collect();
                    return Err(Error::Benchmark(format!(
                        "lit batch-parse produced no output for input #{idx} (prefix '{prefix}'). \
                         Output dir {} contains {} file(s): {:?}",
                        output_dir.display(),
                        produced.len(),
                        listing
                    )));
                }
            }
        }

        let stdout = serde_json::to_string(&results)
            .map_err(|e| Error::Benchmark(format!("Failed to serialize results: {}", e)))?;
        execution.stdout = stdout;
        Ok(execution)
    }

    /// Execute extraction via persistent subprocess (stdin/stdout protocol)
    /// Build a failure `BenchmarkResult` for error paths in `extract()`.
    ///
    /// Centralises the repeated pattern of constructing an error result with
    /// resource statistics, throughput, and framework capabilities.
    fn build_failure_result(
        &self,
        file_path: &Path,
        file_size: u64,
        duration: Duration,
        resource_stats: &crate::monitoring::ResourceStats,
        error: &Error,
        output_format: OutputFormat,
    ) -> BenchmarkResult {
        let framework_capabilities = FrameworkCapabilities {
            supported_extensions: self.supported_formats.clone(),
            ocr_support: Self::framework_supports_ocr(&self.name),
            batch_support: self.batch_capability.is_some(),
            batch_capability: self.batch_capability,
            batch_performance_sample: Some(true),
            ..Default::default()
        };

        let error_kind = error_to_error_kind(error);

        BenchmarkResult {
            framework: self.name.clone(),
            output_format,
            file_path: file_path.to_path_buf(),
            file_size,
            success: false,
            error_message: Some(error.to_string()),
            error_kind,
            duration,
            extraction_duration: None,
            subprocess_overhead: None,
            metrics: PerformanceMetrics {
                baseline_memory_bytes: resource_stats.baseline_memory_bytes,
                peak_memory_bytes: resource_stats.peak_memory_bytes,
                peak_memory_delta_bytes: resource_stats.peak_memory_delta_bytes,
                avg_cpu_percent: resource_stats.avg_cpu_percent,
                cpu_seconds: resource_stats.cpu_seconds,
                throughput_bytes_per_sec: 0.0,
                p50_memory_bytes: resource_stats.p50_memory_bytes,
                p95_memory_bytes: resource_stats.p95_memory_bytes,
                p99_memory_bytes: resource_stats.p99_memory_bytes,
            },
            quality: None,
            iterations: vec![],
            statistics: None,
            cold_start_duration: None,
            file_extension: file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("unknown")
                .to_lowercase(),
            framework_capabilities,
            pdf_metadata: None,
            ocr_status: OcrStatus::Unknown,
            extracted_text: None,
            system_load: None,
        }
    }

    /// Parse extraction result from subprocess output
    ///
    /// Expected subprocess output format:
    /// ```json
    /// {
    ///   "content": "extracted text...",          // REQUIRED
    ///   "_ocr_used": true|false,                 // optional
    ///   "_extraction_time_ms": 123.45            // optional
    /// }
    /// ```
    fn parse_output(&self, stdout: &str) -> Result<serde_json::Value> {
        if is_debug_enabled() {
            let preview = if stdout.len() > 300 {
                let end = (0..=300).rev().find(|&i| stdout.is_char_boundary(i)).unwrap_or(0);
                format!("{}...[{} bytes total]", &stdout[..end], stdout.len())
            } else {
                stdout.to_string()
            };
            tracing::debug!(
                framework = %self.name,
                raw_len = stdout.len(),
                preview = %preview.trim(),
                "parsed subprocess output preview"
            );
        }

        let raw: serde_json::Value = serde_json::from_str(stdout)
            .map_err(|e| Error::Benchmark(format!("Failed to parse subprocess output as JSON: {}", e)))?;

        if !raw.is_object() {
            return Err(Error::Benchmark(
                "Subprocess output must be a JSON object with 'content' field".to_string(),
            ));
        }

        let parsed = if let Some(inner) = raw.get("result").filter(|v| v.is_object()) {
            let mut flat = inner.clone();
            if let (Some(obj), Some(t)) = (flat.as_object_mut(), raw.get("extraction_time_ms")) {
                obj.insert("_extraction_time_ms".to_string(), t.clone());
            }
            if let (Some(obj), Some(meta)) = (flat.as_object_mut(), inner.get("metadata"))
                && let Some(ocr) = meta.get("ocr_used")
            {
                obj.insert("_ocr_used".to_string(), ocr.clone());
            }
            flat
        } else {
            raw
        };

        if let Some(error_val) = parsed.get("error") {
            let error_msg = error_val.as_str().unwrap_or("unknown error");
            if !error_msg.is_empty() {
                if error_msg.contains("timed out") {
                    return Err(Error::Timeout(error_msg.to_string()));
                }
                return Err(Error::FrameworkError(error_msg.to_string()));
            }
        }

        if !parsed.get("content").is_some_and(|v| v.is_string()) {
            let extraction_time = parsed
                .get("_extraction_time_ms")
                .and_then(|v| v.as_f64())
                .unwrap_or(0.0);
            if extraction_time == 0.0 {
                return Err(Error::EmptyContent(
                    "No content extracted (unsupported format or empty result)".to_string(),
                ));
            }
            return Err(Error::Benchmark(
                "Subprocess output missing required 'content' field (must be a string)".to_string(),
            ));
        }

        let content_str = parsed["content"].as_str().unwrap();
        if content_str.trim().is_empty() {
            return Err(Error::EmptyContent("Framework returned empty content".to_string()));
        }

        Ok(parsed)
    }
}

#[async_trait]
impl FrameworkAdapter for SubprocessAdapter {
    fn name(&self) -> &str {
        &self.name
    }

    fn supports_format(&self, file_type: &str) -> bool {
        let file_type_lower = file_type.to_lowercase();
        self.supported_formats
            .iter()
            .any(|fmt| fmt.to_lowercase() == file_type_lower)
    }

    fn should_skip_file(&self, file_name: &str) -> bool {
        self.skip_files.iter().any(|f| f == file_name)
    }

    fn supported_output_formats(&self) -> Vec<OutputFormat> {
        self.supported_output_formats.clone()
    }

    fn ocr_language_policy(&self) -> crate::adapter::OcrLanguagePolicy {
        self.ocr_language_policy
    }

    fn executable_provenance(&self) -> Option<crate::provenance::ExecutableProvenance> {
        self.executable_provenance_for_mode(crate::config::BenchmarkMode::Batch)
    }

    fn executable_provenance_for_mode(
        &self,
        mode: crate::config::BenchmarkMode,
    ) -> Option<crate::provenance::ExecutableProvenance> {
        if self.batch_capability.is_some_and(|capability| {
            mode == crate::config::BenchmarkMode::Batch
                && capability.entry_point == crate::types::BatchEntryPoint::LiteparseBatchParse
        }) {
            let args = self.liteparse_batch_args(
                "<input-dir>",
                "<output-dir>",
                "<output-format>",
                self.args.iter().any(|arg| arg == "--no-ocr"),
            );
            return Some(crate::provenance::ExecutableProvenance::from_invocation(
                self.liteparse_batch_command(),
                &args,
            ));
        }
        let args = self.provenance_args_for_mode(mode);
        Some(crate::provenance::ExecutableProvenance::from_invocation(
            &self.command,
            &args,
        ))
    }

    fn worker_provenance(&self, requested: usize) -> (Option<usize>, Option<usize>) {
        match self.batch_capability.map(|capability| capability.entry_point) {
            Some(crate::types::BatchEntryPoint::DoclingJobkit) => (None, None),
            Some(crate::types::BatchEntryPoint::MineruDoParse) => (None, None),
            Some(crate::types::BatchEntryPoint::XbergCliExtractBatch) => (Some(requested), None),
            Some(crate::types::BatchEntryPoint::LiteparseBatchParse) => (Some(requested), Some(self.batch_workers)),
            None => (Some(requested), Some(requested)),
        }
    }

    fn configured_thread_budget(&self) -> Option<usize> {
        if !self.is_xberg() {
            return None;
        }
        self.xberg_max_threads.or_else(|| {
            self.batch_capability
                .is_some_and(|capability| capability.entry_point == BatchEntryPoint::XbergCliExtractBatch)
                .then(|| self.effective_xberg_max_threads())
        })
    }

    async fn extract(
        &self,
        file_path: &Path,
        timeout: Duration,
        force_ocr: bool,
        ocr_language: Option<&str>,
        output_format: OutputFormat,
    ) -> Result<BenchmarkResult> {
        let timeout = self.effective_timeout(timeout);
        let file_size = std::fs::metadata(file_path).map_err(Error::Io)?.len();

        let start_time = std::time::Instant::now();

        let execution = match self
            .execute_subprocess(file_path, timeout, force_ocr, ocr_language, output_format)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                let actual_duration = start_time.elapsed();
                return Ok(self.build_failure_result(
                    file_path,
                    file_size,
                    actual_duration,
                    &ResourceStats::default(),
                    &e,
                    output_format,
                ));
            }
        };
        let SubprocessExecution {
            stdout,
            duration,
            resource_stats,
            error,
            ..
        } = execution;
        if let Some(error) = error {
            return Ok(self.build_failure_result(
                file_path,
                file_size,
                duration,
                &resource_stats,
                &error,
                output_format,
            ));
        }

        let parsed = match self.parse_output(&stdout) {
            Ok(value) => value,
            Err(e) => {
                return Ok(self.build_failure_result(
                    file_path,
                    file_size,
                    duration,
                    &resource_stats,
                    &e,
                    output_format,
                ));
            }
        };

        let extraction_time_raw = parsed.get("_extraction_time_ms");
        if is_debug_enabled() {
            tracing::debug!(
                framework = %self.name,
                extraction_time_ms = ?extraction_time_raw,
                keys = ?parsed.as_object().map(|object| object.keys().collect::<Vec<_>>()),
                "parsed subprocess extraction metadata"
            );
        }

        let extraction_duration = extraction_time_raw
            .and_then(|v| v.as_f64())
            .map(|ms| Duration::from_secs_f64(ms / 1000.0));

        let extracted_text = parsed.get("content").and_then(|v| v.as_str()).map(|s| s.to_string());

        let subprocess_overhead = extraction_duration.map(|ext| duration.saturating_sub(ext));

        let throughput = bytes_per_second(file_size, duration);

        let self_reported_memory = parsed.get("_peak_memory_bytes").and_then(|v| v.as_u64());

        let metrics = match self_reported_memory {
            Some(reported_mem) if reported_mem >= resource_stats.peak_memory_bytes => PerformanceMetrics {
                baseline_memory_bytes: resource_stats.baseline_memory_bytes,
                peak_memory_bytes: reported_mem,
                peak_memory_delta_bytes: reported_mem.saturating_sub(resource_stats.baseline_memory_bytes),
                avg_cpu_percent: resource_stats.avg_cpu_percent,
                cpu_seconds: resource_stats.cpu_seconds,
                throughput_bytes_per_sec: throughput,
                p50_memory_bytes: reported_mem,
                p95_memory_bytes: reported_mem,
                p99_memory_bytes: reported_mem,
            },
            _ => PerformanceMetrics {
                baseline_memory_bytes: resource_stats.baseline_memory_bytes,
                peak_memory_bytes: resource_stats.peak_memory_bytes,
                peak_memory_delta_bytes: resource_stats.peak_memory_delta_bytes,
                avg_cpu_percent: resource_stats.avg_cpu_percent,
                cpu_seconds: resource_stats.cpu_seconds,
                throughput_bytes_per_sec: throughput,
                p50_memory_bytes: resource_stats.p50_memory_bytes,
                p95_memory_bytes: resource_stats.p95_memory_bytes,
                p99_memory_bytes: resource_stats.p99_memory_bytes,
            },
        };

        let ocr_status = self.resolve_ocr_status(parsed.get("_ocr_used"), force_ocr);

        let framework_capabilities = FrameworkCapabilities {
            supported_extensions: self.supported_formats.clone(),
            ocr_support: Self::framework_supports_ocr(&self.name),
            batch_support: self.batch_capability.is_some(),
            batch_capability: self.batch_capability,
            batch_performance_sample: Some(true),
            ..Default::default()
        };

        let pdf_metadata = if file_path.extension().and_then(|e| e.to_str()) == Some("pdf") {
            Some(crate::types::PdfMetadata {
                has_text_layer: false,
                detection_method: "unknown".to_string(),
                page_count: detect_pdf_page_count(file_path),
                ocr_enabled: ocr_status == OcrStatus::Used,
                text_quality_score: None,
            })
        } else {
            None
        };

        Ok(BenchmarkResult {
            framework: self.name.clone(),
            output_format,
            file_path: file_path.to_path_buf(),
            file_size,
            success: true,
            error_message: None,
            error_kind: ErrorKind::None,
            duration,
            extraction_duration,
            subprocess_overhead,
            metrics,
            quality: None,
            iterations: vec![],
            statistics: None,
            cold_start_duration: None,
            file_extension: file_path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("unknown")
                .to_lowercase(),
            framework_capabilities,
            pdf_metadata,
            ocr_status,
            extracted_text,
            system_load: None,
        })
    }

    fn version(&self) -> String {
        let output = match self.batch_capability.map(|capability| capability.entry_point) {
            Some(crate::types::BatchEntryPoint::DoclingJobkit) => std::process::Command::new(&self.command)
                .args(["-c", DOCLING_VERSION_PROBE])
                .envs(self.env.iter().map(|(key, value)| (key, value)))
                .output(),
            Some(crate::types::BatchEntryPoint::LiteparseBatchParse) => {
                std::process::Command::new(self.liteparse_batch_command())
                    .arg("--version")
                    .output()
            }
            Some(crate::types::BatchEntryPoint::MineruDoParse) => std::process::Command::new(&self.command)
                .args(["-c", MINERU_VERSION_PROBE])
                .envs(self.env.iter().map(|(key, value)| (key, value)))
                .output(),
            _ => std::process::Command::new(&self.command).arg("--version").output(),
        };
        output
            .ok()
            .and_then(first_output_line)
            .unwrap_or_else(|| "unknown".to_string())
    }

    fn batch_capability(&self) -> Option<BatchCapability> {
        self.batch_capability
    }

    async fn extract_batch(
        &self,
        file_paths: &[&Path],
        timeout: Duration,
        force_ocr: &[bool],
        ocr_languages: &[Option<String>],
        output_format: OutputFormat,
    ) -> Result<Vec<BenchmarkResult>> {
        let batch_capability = self.batch_capability.ok_or_else(|| {
            Error::Config(format!(
                "framework '{}' does not expose a verified native batch API",
                self.name
            ))
        })?;
        if force_ocr.len() != file_paths.len() {
            return Err(Error::Benchmark(format!(
                "batch force_ocr cardinality mismatch: received {} flags for {} files",
                force_ocr.len(),
                file_paths.len()
            )));
        }
        if ocr_languages.len() != file_paths.len() {
            return Err(Error::Benchmark(format!(
                "batch ocr_languages cardinality mismatch: received {} values for {} files",
                ocr_languages.len(),
                file_paths.len()
            )));
        }
        if file_paths.is_empty() {
            return Ok(Vec::new());
        }

        let batch_force_ocr = force_ocr.first().copied().unwrap_or(false);
        if force_ocr.iter().any(|flag| *flag != batch_force_ocr) {
            return Err(Error::Config(
                "native batch extraction requires a homogeneous OCR cohort; select fixtures/shard with either all \
                 force-OCR or all non-force-OCR documents"
                    .to_string(),
            ));
        }
        let batch_sample_id = self.batch_sample_id(file_paths, batch_force_ocr, output_format);

        let timeout = self
            .effective_timeout(timeout)
            .checked_mul(file_paths.len() as u32)
            .unwrap_or(Duration::MAX);

        let execution = match self
            .execute_subprocess_batch(file_paths, timeout, batch_force_ocr, ocr_languages, output_format)
            .await
        {
            Ok(result) => result,
            Err(e) => {
                // Xberg's batch CLI uses fail_if_errors: a failed item makes
                // the process fail, so there is no honest partial envelope to
                // synthesize into per-file benchmark rows. ~keep
                return Err(e);
            }
        };
        let SubprocessExecution {
            stdout,
            duration,
            resource_stats,
            error,
            ..
        } = execution;
        if let Some(error) = error {
            let results = file_paths
                .iter()
                .map(|file_path| {
                    let file_size = std::fs::metadata(file_path).map_or(0, |metadata| metadata.len());
                    self.build_failure_result(file_path, file_size, duration, &resource_stats, &error, output_format)
                })
                .collect();
            return Ok(results);
        }

        let parsed_batch = parse_batch_output(&stdout)?;

        if parsed_batch.items.len() != file_paths.len() {
            return Err(Error::Benchmark(format!(
                "batch output cardinality mismatch: received {} results for {} files",
                parsed_batch.items.len(),
                file_paths.len()
            )));
        }
        if parsed_batch.per_file_durations.len() != file_paths.len() {
            return Err(Error::Benchmark(format!(
                "batch timing cardinality mismatch: received {} per-file durations for {} files",
                parsed_batch.per_file_durations.len(),
                file_paths.len()
            )));
        }
        if batch_capability.per_item_timing {
            if parsed_batch.per_file_durations.iter().any(Option::is_none) {
                return Err(Error::Benchmark(format!(
                    "framework '{}' declares per-item batch timing but returned unavailable timing values",
                    self.name
                )));
            }
        } else if parsed_batch.per_file_durations.iter().any(Option::is_some) {
            return Err(Error::Benchmark(format!(
                "framework '{}' declares per-item batch timing unavailable but returned numeric timing values",
                self.name
            )));
        }

        // Use the slower of process-wall time and an adapter-reported batch
        // makespan. This consumes Xberg's `total_ms` without allowing a
        // self-reported inner timer to inflate cross-framework throughput. ~keep
        let batch_makespan = parsed_batch
            .reported_total_duration
            .map_or(duration, |reported| duration.max(reported));
        let batch_subprocess_overhead = parsed_batch
            .reported_total_duration
            .map(|reported| duration.saturating_sub(reported));

        let batch_ocr_statuses: Vec<OcrStatus> = parsed_batch
            .items
            .iter()
            .map(|item| {
                self.resolve_ocr_status(
                    item.get("_ocr_used")
                        .or_else(|| item.get("metadata").and_then(|metadata| metadata.get("ocr_used"))),
                    batch_force_ocr,
                )
            })
            .collect();

        let batch_contents: Vec<Option<String>> = parsed_batch
            .items
            .iter()
            .map(|item| item.get("content").and_then(|value| value.as_str()).map(str::to_string))
            .collect();

        let batch_validations: Vec<(bool, Option<String>, ErrorKind)> = parsed_batch
            .items
            .iter()
            .map(|item| {
                if let Some(error_val) = item.get("error") {
                    let error_msg = error_val.as_str().unwrap_or("unknown error");
                    if !error_msg.is_empty() {
                        let kind = if error_msg.contains("timed out") {
                            ErrorKind::Timeout
                        } else {
                            ErrorKind::FrameworkError
                        };
                        return (false, Some(error_msg.to_string()), kind);
                    }
                }
                match item.get("content").and_then(|value| value.as_str()) {
                    Some(content) if !content.trim().is_empty() => (true, None, ErrorKind::None),
                    Some(_) => (
                        false,
                        Some("Framework returned empty content".to_string()),
                        ErrorKind::EmptyContent,
                    ),
                    None => (
                        false,
                        Some("No content extracted (unsupported format or empty result)".to_string()),
                        ErrorKind::EmptyContent,
                    ),
                }
            })
            .collect();

        if let Some((index, (_, error, _))) = batch_validations
            .iter()
            .enumerate()
            .find(|(_, validation)| !validation.0)
        {
            return Err(Error::Benchmark(format!(
                "framework '{}' returned a partial batch failure for {}: {}",
                self.name,
                file_paths[index].display(),
                error.as_deref().unwrap_or("unspecified extraction failure")
            )));
        }

        let successful_bytes: u64 = file_paths
            .iter()
            .zip(&batch_validations)
            .filter(|(_, validation)| validation.0)
            .filter_map(|(path, _)| std::fs::metadata(path).ok().map(|metadata| metadata.len()))
            .sum();
        let throughput_anchor = batch_validations.iter().position(|validation| validation.0);
        let batch_throughput = bytes_per_second(successful_bytes, batch_makespan);

        let framework_capabilities = FrameworkCapabilities {
            supported_extensions: self.supported_formats.clone(),
            ocr_support: Self::framework_supports_ocr(&self.name),
            batch_support: self.batch_capability.is_some(),
            batch_capability: self.batch_capability,
            batch_sample_id: Some(batch_sample_id),
            ..Default::default()
        };

        let results: Vec<BenchmarkResult> = file_paths
            .iter()
            .enumerate()
            .map(|(idx, file_path)| {
                let file_size = std::fs::metadata(file_path).map(|m| m.len()).unwrap_or(0);

                let file_extension = file_path.extension().and_then(|e| e.to_str()).unwrap_or("").to_string();

                let ocr_status = batch_ocr_statuses.get(idx).copied().unwrap_or(OcrStatus::Unknown);

                let extraction_duration = parsed_batch.per_file_durations[idx];

                let (item_success, item_error, item_error_kind) = batch_validations.get(idx).cloned().unwrap_or((
                    false,
                    Some("Missing validation for batch item".to_string()),
                    ErrorKind::HarnessError,
                ));
                let mut item_capabilities = framework_capabilities.clone();
                item_capabilities.batch_performance_sample = Some(throughput_anchor == Some(idx));

                let pdf_metadata = if file_extension.eq_ignore_ascii_case("pdf") {
                    Some(crate::types::PdfMetadata {
                        has_text_layer: false,
                        detection_method: "unknown".to_string(),
                        page_count: detect_pdf_page_count(file_path),
                        ocr_enabled: ocr_status == OcrStatus::Used,
                        text_quality_score: None,
                    })
                } else {
                    None
                };

                BenchmarkResult {
                    framework: self.name.clone(),
                    output_format,
                    file_path: file_path.to_path_buf(),
                    file_size,
                    success: item_success,
                    error_message: item_error,
                    error_kind: item_error_kind,
                    duration: batch_makespan,
                    extraction_duration,
                    subprocess_overhead: batch_subprocess_overhead,
                    metrics: PerformanceMetrics {
                        baseline_memory_bytes: resource_stats.baseline_memory_bytes,
                        peak_memory_bytes: resource_stats.peak_memory_bytes,
                        peak_memory_delta_bytes: resource_stats.peak_memory_delta_bytes,
                        avg_cpu_percent: resource_stats.avg_cpu_percent,
                        cpu_seconds: resource_stats.cpu_seconds,
                        // Every sibling carries the same process sample so each
                        // reporting bucket can recover it; aggregation deduplicates
                        // by `batch_sample_id`. ~keep
                        throughput_bytes_per_sec: batch_throughput,
                        p50_memory_bytes: resource_stats.p50_memory_bytes,
                        p95_memory_bytes: resource_stats.p95_memory_bytes,
                        p99_memory_bytes: resource_stats.p99_memory_bytes,
                    },
                    quality: None,
                    iterations: vec![],
                    statistics: None,
                    cold_start_duration: None,
                    file_extension,
                    framework_capabilities: item_capabilities,
                    pdf_metadata,
                    ocr_status,
                    extracted_text: batch_contents.get(idx).cloned().flatten(),
                    system_load: None,
                }
            })
            .collect();

        Ok(results)
    }

    async fn setup(&self) -> Result<()> {
        which::which(&self.command)
            .map_err(|e| Error::Benchmark(format!("Command '{}' not found: {}", self.command.display(), e)))?;
        Ok(())
    }

    async fn teardown(&self) -> Result<()> {
        Ok(())
    }
}

impl Default for PerformanceMetrics {
    fn default() -> Self {
        Self {
            baseline_memory_bytes: 0,
            peak_memory_bytes: 0,
            peak_memory_delta_bytes: 0,
            avg_cpu_percent: 0.0,
            cpu_seconds: 0.0,
            throughput_bytes_per_sec: 0.0,
            p50_memory_bytes: 0,
            p95_memory_bytes: 0,
            p99_memory_bytes: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_batch_capability(per_item_timing: bool) -> BatchCapability {
        BatchCapability {
            entry_point: BatchEntryPoint::XbergCliExtractBatch,
            timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing,
        }
    }

    #[cfg(unix)]
    fn fake_docling_site(
        docling_distribution: Option<&str>,
        slim_distribution: Option<&str>,
        module_version: Option<&str>,
    ) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let site = tempfile::tempdir().unwrap();
        let package_dir = site.path().join("docling");
        std::fs::create_dir(&package_dir).unwrap();
        let module = module_version
            .map(|version| format!("__version__ = {version:?}\n"))
            .unwrap_or_default();
        std::fs::write(package_dir.join("__init__.py"), module).unwrap();

        for (name, normalized_name, version) in [
            ("docling", "docling", docling_distribution),
            ("docling-slim", "docling_slim", slim_distribution),
        ] {
            let Some(version) = version else {
                continue;
            };
            let metadata_dir = site.path().join(format!("{normalized_name}-{version}.dist-info"));
            std::fs::create_dir(&metadata_dir).unwrap();
            std::fs::write(
                metadata_dir.join("METADATA"),
                format!("Metadata-Version: 2.1\nName: {name}\nVersion: {version}\n"),
            )
            .unwrap();
        }

        let python = which::which("python3").unwrap();
        let wrapper = site.path().join("isolated-python");
        std::fs::write(
            &wrapper,
            format!("#!/bin/sh\nexec \"{}\" -S \"$@\"\n", python.display()),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&wrapper).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&wrapper, permissions).unwrap();
        (site, wrapper)
    }

    #[cfg(unix)]
    fn docling_version_from_site(
        docling_distribution: Option<&str>,
        slim_distribution: Option<&str>,
        module_version: Option<&str>,
    ) -> String {
        let (site, python) = fake_docling_site(docling_distribution, slim_distribution, module_version);
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            python,
            vec![],
            vec![("PYTHONPATH".to_string(), site.path().to_string_lossy().into_owned())],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::DoclingJobkit,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        );
        adapter.version()
    }

    #[cfg(unix)]
    #[test]
    fn docling_version_prefers_full_distribution() {
        assert_eq!(
            docling_version_from_site(Some("1.2.3"), Some("2.3.4"), Some("3.4.5")),
            "1.2.3"
        );
    }

    #[cfg(unix)]
    #[test]
    fn docling_version_falls_back_to_slim_distribution() {
        assert_eq!(docling_version_from_site(None, Some("2.3.4"), Some("3.4.5")), "2.3.4");
    }

    #[cfg(unix)]
    #[test]
    fn docling_version_falls_back_to_module_version() {
        assert_eq!(docling_version_from_site(None, None, Some("3.4.5")), "3.4.5");
    }

    #[cfg(unix)]
    #[test]
    fn docling_version_is_unknown_when_all_sources_are_empty() {
        assert_eq!(docling_version_from_site(None, None, Some("")), "unknown");
    }

    #[test]
    fn test_subprocess_adapter_creation() {
        let adapter = SubprocessAdapter::new(
            "test-adapter",
            "echo",
            vec!["test".to_string()],
            vec![],
            vec!["pdf".to_string(), "docx".to_string()],
        );
        assert_eq!(adapter.name(), "test-adapter");
    }

    #[test]
    fn test_supports_format() {
        let adapter = SubprocessAdapter::new(
            "test",
            "echo",
            vec![],
            vec![],
            vec!["pdf".to_string(), "docx".to_string()],
        );
        assert!(adapter.supports_format("pdf"));
        assert!(adapter.supports_format("docx"));
        assert!(!adapter.supports_format("unknown"));
    }

    #[test]
    fn ocr_language_forward_arg_only_when_flag_and_language_present() {
        let base = || SubprocessAdapter::new("ext", "echo", vec![], vec![], vec!["png".to_string()]);

        // No flag configured: never forwards, even with a fixture language.
        assert_eq!(base().ocr_language_forward_arg(Some("eng+kor")), None);

        let forwarding = base().with_ocr_language_arg("--ocr-lang");
        // Flag configured but fixture pins no language: nothing forwarded.
        assert_eq!(forwarding.ocr_language_forward_arg(None), None);
        // Emitted as a single `--flag=value` token in canonical Tesseract form.
        assert_eq!(
            forwarding.ocr_language_forward_arg(Some("eng+kor")).as_deref(),
            Some("--ocr-lang=eng+kor")
        );
        // Whitespace/formatting is canonicalized, matching the xberg path.
        assert_eq!(
            forwarding.ocr_language_forward_arg(Some(" jpn_vert ")).as_deref(),
            Some("--ocr-lang=jpn_vert")
        );
    }

    #[test]
    fn native_batch_language_forwarding_requires_one_global_language() {
        let adapter = SubprocessAdapter::new("docling", "echo", vec![], vec![], vec!["png".to_string()])
            .with_ocr_language_arg("--ocr-lang")
            .with_ocr_language_policy(crate::adapter::OcrLanguagePolicy::AnyBatchGlobal);
        assert_eq!(
            adapter
                .batch_ocr_language_forward_arg(&[Some(" eng ".to_string()), Some("eng".to_string())])
                .unwrap()
                .as_deref(),
            Some("--ocr-lang=eng")
        );
        assert!(
            adapter
                .batch_ocr_language_forward_arg(&[Some("eng".to_string()), Some("deu".to_string())])
                .is_err()
        );
    }

    #[test]
    fn test_parse_output_empty_error_no_content() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = r#"{"error": "", "_extraction_time_ms": 0}"#;
        let result = adapter.parse_output(output);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::EmptyContent(_)),
            "Expected EmptyContent, got: {:?}",
            err
        );
        assert!(err.to_string().contains("No content extracted"));
    }

    #[test]
    fn test_parse_output_nonempty_error() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = r#"{"error": "something went wrong"}"#;
        let result = adapter.parse_output(output);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::FrameworkError(_)),
            "Expected FrameworkError, got: {:?}",
            err
        );
        assert!(err.to_string().contains("something went wrong"));
    }

    #[test]
    fn test_parse_output_valid_content() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = r#"{"content": "Hello, world!", "_extraction_time_ms": 42.5}"#;
        let result = adapter.parse_output(output);
        assert!(result.is_ok());
        let parsed = result.unwrap();
        assert_eq!(parsed["content"], "Hello, world!");
        assert_eq!(parsed["_extraction_time_ms"], 42.5);
    }

    #[test]
    fn test_parse_output_missing_content_nonzero_time() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = r#"{"_extraction_time_ms": 150.0}"#;
        let result = adapter.parse_output(output);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::Benchmark(_)),
            "Expected Benchmark error, got: {:?}",
            err
        );
        assert!(err.to_string().contains("missing required 'content' field"));
    }

    #[test]
    fn test_max_timeout_clamps_config_timeout() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()])
            .with_max_timeout(Duration::from_secs(120));
        let effective = adapter.effective_timeout(Duration::from_secs(900));
        assert_eq!(effective, Duration::from_secs(120));
    }

    #[test]
    fn test_max_timeout_passes_lower_config() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()])
            .with_max_timeout(Duration::from_secs(120));
        let effective = adapter.effective_timeout(Duration::from_secs(60));
        assert_eq!(effective, Duration::from_secs(60));
    }

    #[test]
    fn test_max_timeout_none_uses_config() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let effective = adapter.effective_timeout(Duration::from_secs(900));
        assert_eq!(effective, Duration::from_secs(900));
    }

    #[test]
    fn test_with_max_timeout_builder() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()])
            .with_max_timeout(Duration::from_secs(300));
        assert_eq!(adapter.max_timeout, Some(Duration::from_secs(300)));
    }

    #[test]
    fn test_parse_output_empty_string_content() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = r#"{"content": "", "_extraction_time_ms": 5.0}"#;
        let result = adapter.parse_output(output);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::EmptyContent(_)),
            "Expected EmptyContent, got: {:?}",
            err
        );
        assert!(err.to_string().contains("empty content"));
    }

    #[test]
    fn test_parse_output_whitespace_only_content() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = "{\"content\": \"  \\n  \", \"_extraction_time_ms\": 10.0}";
        let result = adapter.parse_output(output);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(
            matches!(err, Error::EmptyContent(_)),
            "Expected EmptyContent, got: {:?}",
            err
        );
    }

    #[test]
    fn test_parse_output_python_side_timeout() {
        let adapter = SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]);
        let output = r#"{"error": "extraction timed out after 150s", "_extraction_time_ms": 150000.0}"#;
        let result = adapter.parse_output(output);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, Error::Timeout(_)), "Expected Timeout, got: {:?}", err);
        assert!(err.to_string().contains("timed out"));
    }

    #[test]
    fn test_error_to_error_kind_mapping() {
        assert_eq!(error_to_error_kind(&Error::Timeout("test".into())), ErrorKind::Timeout);
        assert_eq!(
            error_to_error_kind(&Error::FrameworkError("test".into())),
            ErrorKind::FrameworkError
        );
        assert_eq!(
            error_to_error_kind(&Error::EmptyContent("test".into())),
            ErrorKind::EmptyContent
        );
        assert_eq!(
            error_to_error_kind(&Error::Benchmark("test".into())),
            ErrorKind::HarnessError
        );

        assert_eq!(
            error_to_error_kind(&Error::Benchmark("torch.PP-OCRv6 not found".into())),
            ErrorKind::ConfigSetupError
        );
        assert_eq!(
            error_to_error_kind(&Error::Benchmark("partition_X not available".into())),
            ErrorKind::ConfigSetupError
        );
        assert_eq!(
            error_to_error_kind(&Error::Benchmark("tessdata not found".into())),
            ErrorKind::ConfigSetupError
        );
        assert_eq!(
            error_to_error_kind(&Error::Config("Module not installed".into())),
            ErrorKind::ConfigSetupError
        );
    }

    /// Regression test for Bug C: a framework-emitted crash (captured in the subprocess's
    /// stderr and embedded into the `Error::Benchmark` message by `execute_subprocess`) must be
    /// classified as `FrameworkError`, not `HarnessError` — the framework failed, not the
    /// harness. Uses the exact stderr shape `docling_extract.py` produces on an uncaught
    /// exception: `Error extracting with Docling: Unsupported configuration: ...`.
    #[test]
    fn test_error_to_error_kind_framework_crash_stderr_is_framework_error() {
        let msg = "Subprocess failed with exit code Some(1)\nstderr: Error extracting with Docling: \
                    Unsupported configuration: torch.PP-OCRv6.det.small"
            .to_string();
        assert_eq!(error_to_error_kind(&Error::Benchmark(msg)), ErrorKind::FrameworkError);
    }

    /// A genuine harness-side failure (e.g. we failed to spawn the subprocess at all) must stay
    /// `HarnessError` — the framework-crash heuristic must not over-reach.
    #[test]
    fn test_error_to_error_kind_harness_spawn_failure_stays_harness_error() {
        let msg = "Failed to spawn subprocess 'docling-cli' with args []: No such file or directory".to_string();
        assert_eq!(error_to_error_kind(&Error::Benchmark(msg)), ErrorKind::HarnessError);
    }

    #[test]
    fn test_format_aware_builder() {
        let adapter =
            SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]).with_format_aware(true);
        assert!(adapter.format_aware);
        assert!(adapter.batch_capability.is_none());
        assert_eq!(
            adapter.supported_output_formats(),
            vec![OutputFormat::Plaintext, OutputFormat::Markdown]
        );
    }

    #[test]
    fn test_native_batch_builder() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "echo",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::LiteparseBatchParse,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        )
        .with_format_aware(true);
        assert!(adapter.batch_capability.is_some());
        assert!(adapter.format_aware);
        assert_eq!(
            adapter.batch_capability.map(|capability| capability.entry_point),
            Some(BatchEntryPoint::LiteparseBatchParse)
        );
    }

    #[test]
    fn liteparse_batch_provenance_hashes_normalized_semantic_arguments() {
        let command = PathBuf::from("/opt/liteparse/bin/lit");
        let adapter = SubprocessAdapter::with_batch_capability(
            "liteparse",
            "bash",
            vec!["liteparse_extract.sh".to_string(), "--no-ocr".to_string()],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::LiteparseBatchParse,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        )
        .with_batch_workers(4)
        .with_native_batch_command(command.clone());
        let expected_args = vec![
            "batch-parse".to_string(),
            "<input-dir>".to_string(),
            "<output-dir>".to_string(),
            "--format".to_string(),
            "<output-format>".to_string(),
            "--num-workers".to_string(),
            "4".to_string(),
            "--quiet".to_string(),
            "--no-ocr".to_string(),
        ];

        assert_eq!(
            adapter.liteparse_batch_args("<input-dir>", "<output-dir>", "<output-format>", true),
            expected_args
        );
        assert_eq!(
            adapter.executable_provenance_for_mode(crate::config::BenchmarkMode::Batch),
            Some(crate::provenance::ExecutableProvenance::from_invocation(
                &command,
                &expected_args
            ))
        );
    }

    #[test]
    fn repeated_identical_batch_invocations_receive_distinct_sample_ids() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            "python",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::DoclingJobkit,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        )
        .with_batch_workers(4);
        let input = Path::new("/tmp/identical.pdf");
        let paths = [input];

        let first = adapter.batch_sample_id(&paths, false, OutputFormat::Markdown);
        let second = adapter.batch_sample_id(&paths, false, OutputFormat::Markdown);

        assert_ne!(first, second);
    }

    #[test]
    fn generic_batch_builder_preserves_separate_single_file_command() {
        let batch_args = vec!["docling_extract.py".to_string(), "batch".to_string()];
        let single_file_args = vec!["docling_extract.py".to_string(), "sync".to_string()];
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            "python",
            batch_args.clone(),
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::DoclingJobkit,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        )
        .with_single_file_args(single_file_args.clone());

        assert!(adapter.batch_capability.is_some());
        assert_eq!(adapter.args.last().map(String::as_str), Some("batch"));
        assert_eq!(
            adapter
                .single_file_args
                .as_ref()
                .and_then(|args| args.last())
                .map(String::as_str),
            Some("sync")
        );
        assert_eq!(
            adapter.executable_provenance_for_mode(crate::config::BenchmarkMode::SingleFile),
            Some(crate::provenance::ExecutableProvenance::from_invocation(
                Path::new("python"),
                &single_file_args,
            ))
        );
        assert_eq!(
            adapter.executable_provenance_for_mode(crate::config::BenchmarkMode::Batch),
            Some(crate::provenance::ExecutableProvenance::from_invocation(
                Path::new("python"),
                &batch_args,
            ))
        );
    }

    #[test]
    fn liteparse_single_mode_records_wrapper_invocation() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "liteparse",
            "bash",
            vec!["liteparse_extract.sh".to_string()],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::LiteparseBatchParse,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        );

        let provenance = adapter
            .executable_provenance_for_mode(crate::config::BenchmarkMode::SingleFile)
            .unwrap();
        assert_eq!(provenance.name, "bash");
        assert!(!provenance.invocation_blake3.is_empty());
    }

    #[test]
    fn configured_external_ocr_status_is_used_when_output_has_no_metadata() {
        let enabled = SubprocessAdapter::new("docling", "echo", vec![], vec![], vec!["pdf".to_string()])
            .with_configured_ocr(true);
        let disabled = SubprocessAdapter::new("docling", "echo", vec![], vec![], vec!["pdf".to_string()])
            .with_configured_ocr(false);

        assert_eq!(enabled.resolve_ocr_status(None, false), OcrStatus::Used);
        assert_eq!(disabled.resolve_ocr_status(None, false), OcrStatus::NotUsed);
        assert_eq!(disabled.resolve_ocr_status(None, true), OcrStatus::Used);
    }

    #[test]
    fn batch_worker_builder_uses_requested_nonzero_limit() {
        let requested =
            SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]).with_batch_workers(7);
        let zero =
            SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]).with_batch_workers(0);

        assert_eq!(requested.batch_workers, 7);
        assert_eq!(zero.batch_workers, 1);
    }

    #[test]
    fn xberg_thread_budget_builder_uses_requested_nonzero_limit() {
        let requested =
            SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]).with_xberg_max_threads(9);
        let zero =
            SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]).with_xberg_max_threads(0);

        assert_eq!(requested.xberg_max_threads, Some(9));
        assert_eq!(zero.xberg_max_threads, Some(1));
        assert_eq!(requested.configured_thread_budget(), None);
    }

    #[test]
    fn xberg_thread_budget_defaults_to_batch_workers() {
        let adapter =
            SubprocessAdapter::new("test", "echo", vec![], vec![], vec!["pdf".to_string()]).with_batch_workers(7);

        assert_eq!(adapter.xberg_max_threads, None);
        assert_eq!(adapter.effective_xberg_max_threads(), 7);
        assert_eq!(adapter.configured_thread_budget(), None);
    }

    #[test]
    fn xberg_worker_provenance_does_not_guess_dynamic_document_concurrency() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "xberg-test",
            "echo",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::XbergCliExtractBatch,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: true,
            },
        )
        .with_batch_workers(4);

        assert_eq!(adapter.worker_provenance(4), (Some(4), None));
        assert_eq!(adapter.configured_thread_budget(), Some(4));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn xberg_batch_passes_distinct_concurrency_and_thread_limits() {
        let script = r#"
            concurrent=""
            threads=""
            while [ "$#" -gt 0 ]; do
                case "$1" in
                    --max-concurrent) concurrent="$2"; shift 2 ;;
                    --max-threads) threads="$2"; shift 2 ;;
                    *) shift ;;
                esac
            done
            [ "$concurrent" = "3" ] && [ "$threads" = "7" ] || exit 64
            sleep 0.02
            printf '{"results":[{"content":"ok"}],"total_ms":0,"per_file_ms":[1]}'
        "#;
        let adapter = SubprocessAdapter::with_batch_capability(
            "xberg-test",
            "sh",
            vec!["-c".to_string(), script.to_string(), "worker-budget-probe".to_string()],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        )
        .with_batch_workers(3)
        .with_xberg_max_threads(7);
        let file = tempfile::NamedTempFile::new().unwrap();

        let results = adapter
            .extract_batch(
                &[file.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn xberg_batch_defaults_thread_limit_to_worker_limit() {
        let script = r#"
            concurrent=""
            threads=""
            while [ "$#" -gt 0 ]; do
                case "$1" in
                    --max-concurrent) concurrent="$2"; shift 2 ;;
                    --max-threads) threads="$2"; shift 2 ;;
                    *) shift ;;
                esac
            done
            [ "$concurrent" = "7" ] && [ "$threads" = "7" ] || exit 64
            sleep 0.02
            printf '{"results":[{"content":"ok"}],"total_ms":0,"per_file_ms":[1]}'
        "#;
        let adapter = SubprocessAdapter::with_batch_capability(
            "xberg-test",
            "sh",
            vec!["-c".to_string(), script.to_string(), "legacy-budget-probe".to_string()],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        )
        .with_batch_workers(7);
        let file = tempfile::NamedTempFile::new().unwrap();

        let results = adapter
            .extract_batch(
                &[file.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn xberg_single_passes_explicit_thread_limit() {
        let script = r#"
            threads=""
            while [ "$#" -gt 1 ]; do
                case "$1" in
                    --max-threads) threads="$2"; shift 2 ;;
                    *) shift ;;
                esac
            done
            [ "$threads" = "7" ] || exit 64
            sleep 0.02
            printf '{"content":"ok"}'
        "#;
        let adapter = SubprocessAdapter::new(
            "xberg-test",
            "sh",
            vec!["-c".to_string(), script.to_string(), "single-budget-probe".to_string()],
            vec![],
            vec!["pdf".to_string()],
        )
        .with_xberg_max_threads(7);
        let file = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract(file.path(), Duration::from_secs(1), false, None, OutputFormat::Markdown)
            .await
            .unwrap();

        assert!(result.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn xberg_single_without_explicit_budget_preserves_cli_auto_threads() {
        let script = r#"
            while [ "$#" -gt 1 ]; do
                [ "$1" != "--max-threads" ] || exit 64
                shift
            done
            printf '{"content":"ok"}'
        "#;
        let adapter = SubprocessAdapter::new(
            "xberg-test",
            "sh",
            vec![
                "-c".to_string(),
                script.to_string(),
                "single-auto-budget-probe".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
        )
        .with_batch_workers(7);
        let file = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract(file.path(), Duration::from_secs(1), false, None, OutputFormat::Markdown)
            .await
            .unwrap();

        assert!(result.success);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn non_xberg_single_ignores_xberg_thread_limit() {
        let script = r#"
            while [ "$#" -gt 1 ]; do
                [ "$1" != "--max-threads" ] || exit 64
                shift
            done
            printf '{"content":"ok"}'
        "#;
        let adapter = SubprocessAdapter::new(
            "docling",
            "sh",
            vec![
                "-c".to_string(),
                script.to_string(),
                "non-xberg-budget-probe".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
        )
        .with_xberg_max_threads(7);
        let file = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract(file.path(), Duration::from_secs(1), false, None, OutputFormat::Markdown)
            .await
            .unwrap();

        assert!(result.success);
    }

    #[test]
    fn forced_ocr_upgrades_external_no_ocr_flag() {
        let adapter = SubprocessAdapter::new(
            "docling",
            "echo",
            vec!["--no-ocr".to_string(), "sync".to_string()],
            vec![],
            vec!["pdf".to_string()],
        );

        assert_eq!(adapter.request_args(false)[0], "--no-ocr");
        assert_eq!(adapter.request_args(true)[0], "--ocr");
    }

    #[test]
    fn tesseract_file_config_preserves_effective_backend_and_cache_settings() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"ocr":{"enabled":true,"backend":"tesseract","tesseract_config":{"use_cache":false}}}"#.to_string(),
        ];
        let base_ocr = effective_ocr_config_from_args(&args).expect("Tesseract OCR config");
        let cwd = tempfile::tempdir().unwrap();
        let input = Path::new("sample.pdf");
        let configs = build_batch_file_configs(
            &[input],
            &[Some(" deu + eng ".to_string())],
            cwd.path(),
            Some(&base_ocr),
        );
        let config = configs
            .get(&cwd.path().join(input).to_string_lossy().into_owned())
            .expect("file config");

        assert_eq!(
            config.pointer("/ocr/backend").and_then(serde_json::Value::as_str),
            Some("tesseract")
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            config.pointer("/ocr/language"),
            Some(&serde_json::json!(["deu", "eng"]))
        );
        assert_eq!(
            config.pointer("/ocr/tesseract_config/language"),
            Some(&serde_json::json!(["deu", "eng"]))
        );
    }

    #[test]
    fn tesseract_file_config_updates_pipeline_stage_languages() {
        let config = serde_json::json!({
            "ocr": {
                "enabled": true,
                "backend": "tesseract",
                "pipeline": {
                    "stages": [
                        {
                            "backend": "tesseract",
                            "language": ["eng"],
                            "tesseract_config": {"language": ["eng"], "use_cache": false}
                        },
                        {"backend": "paddle-ocr", "language": ["en"]}
                    ]
                }
            }
        });
        let args = vec!["--config-json".to_string(), config.to_string()];
        let base_ocr = effective_ocr_config_from_args(&args).expect("Tesseract OCR config");
        let cwd = tempfile::tempdir().unwrap();
        let input = Path::new("sample.pdf");
        let configs = build_batch_file_configs(&[input], &[Some("deu".to_string())], cwd.path(), Some(&base_ocr));
        let config = configs
            .get(&cwd.path().join(input).to_string_lossy().into_owned())
            .expect("file config");

        assert_eq!(
            config.pointer("/ocr/pipeline/stages/0/language"),
            Some(&serde_json::json!(["deu"]))
        );
        assert_eq!(
            config.pointer("/ocr/pipeline/stages/0/tesseract_config/language"),
            Some(&serde_json::json!(["deu"]))
        );
        assert_eq!(
            config.pointer("/ocr/pipeline/stages/1"),
            Some(&serde_json::json!({
                "backend": "paddle-ocr",
                "language": ["en"]
            }))
        );
    }

    #[test]
    fn tesseract_file_config_materializes_whole_image_psm_without_fixture_language() {
        let base_ocr = serde_json::json!({"enabled": true, "backend": "tesseract"});
        let cwd = tempfile::tempdir().unwrap();
        let input = Path::new("sample.pdf");
        let configs = build_batch_file_configs(&[input], &[None], cwd.path(), Some(&base_ocr));
        let config = configs
            .get(&cwd.path().join(input).to_string_lossy().into_owned())
            .expect("every Tesseract fixture must get a per-file override, even without a fixture language");

        assert_eq!(config.pointer("/ocr/language"), Some(&serde_json::json!(["eng"])));
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM as i64)
        );
    }

    #[test]
    fn tesseract_file_config_materializes_vertical_psm_for_vert_fixture_language() {
        let base_ocr = serde_json::json!({"enabled": true, "backend": "tesseract"});
        let cwd = tempfile::tempdir().unwrap();
        let input = Path::new("sample.jpeg");
        let configs = build_batch_file_configs(&[input], &[Some("jpn_vert".to_string())], cwd.path(), Some(&base_ocr));
        let config = configs
            .get(&cwd.path().join(input).to_string_lossy().into_owned())
            .expect("file config");

        assert_eq!(config.pointer("/ocr/language"), Some(&serde_json::json!(["jpn_vert"])));
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(crate::adapter::XBERG_VERTICAL_BLOCK_TESSERACT_PSM as i64)
        );
    }

    #[test]
    fn non_tesseract_file_config_without_fixture_language_produces_no_override() {
        let base_ocr = serde_json::json!({"enabled": true, "backend": "paddle-ocr"});
        let cwd = tempfile::tempdir().unwrap();
        let configs = build_batch_file_configs(&[Path::new("sample.pdf")], &[None], cwd.path(), Some(&base_ocr));

        assert!(
            configs.is_empty(),
            "a non-Tesseract fixture without an explicit language needs no per-file override"
        );
    }

    #[test]
    fn single_file_tesseract_override_materializes_whole_image_psm_without_fixture_language() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"ocr":{"enabled":true,"backend":"tesseract"}}"#.to_string(),
        ];

        let rewritten = apply_tesseract_ocr_override_to_args(&args, None).expect("tesseract config to rewrite");
        let config: serde_json::Value = serde_json::from_str(&rewritten[1]).unwrap();

        assert_eq!(config.pointer("/ocr/language"), Some(&serde_json::json!(["eng"])));
        assert_eq!(
            config.pointer("/ocr/tesseract_config/language"),
            Some(&serde_json::json!(["eng"])),
            "language must land on both ocr.language and the materialized tesseract_config.language"
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM as i64)
        );
    }

    #[test]
    fn single_file_tesseract_override_materializes_vertical_psm_for_vert_fixture_language() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"ocr":{"enabled":true,"backend":"tesseract"}}"#.to_string(),
        ];

        let rewritten =
            apply_tesseract_ocr_override_to_args(&args, Some("jpn_vert")).expect("tesseract config to rewrite");
        let config: serde_json::Value = serde_json::from_str(&rewritten[1]).unwrap();

        assert_eq!(config.pointer("/ocr/language"), Some(&serde_json::json!(["jpn_vert"])));
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(crate::adapter::XBERG_VERTICAL_BLOCK_TESSERACT_PSM as i64)
        );
    }

    #[test]
    fn single_file_tesseract_override_preserves_explicit_psm() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"ocr":{"enabled":true,"backend":"tesseract","tesseract_config":{"psm":6}}}"#.to_string(),
        ];

        let rewritten =
            apply_tesseract_ocr_override_to_args(&args, Some("jpn_vert")).expect("tesseract config to rewrite");
        let config: serde_json::Value = serde_json::from_str(&rewritten[1]).unwrap();

        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(6),
            "an explicit PSM must survive a language update untouched"
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
    }

    #[test]
    fn single_file_tesseract_override_is_none_for_non_tesseract_backend() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"ocr":{"enabled":true,"backend":"paddle-ocr"}}"#.to_string(),
        ];

        assert_eq!(apply_tesseract_ocr_override_to_args(&args, Some("deu")), None);
    }

    #[test]
    fn single_file_tesseract_override_synthesizes_config_json_for_cli_only_ocr() {
        // BLOCKER 1 regression: OCR enabled purely via `--ocr true` (e.g. `request_args_from`'s
        // force-OCR upgrade path for an adapter whose base `--config-json` carries no `ocr` key
        // at all — see `forced_ocr_adds_cli_ocr_for_native_xberg_config`) must still get its
        // result cache disabled and PSM materialized, not just its language forwarded via
        // `--ocr-language`.
        let args = vec!["--ocr".to_string(), "true".to_string()];

        let rewritten =
            apply_tesseract_ocr_override_to_args(&args, Some("deu")).expect("CLI-only tesseract OCR must be rewritten");

        assert!(rewritten.iter().any(|arg| arg == "--config-json"));
        let config_index = rewritten.iter().position(|arg| arg == "--config-json").unwrap();
        let config: serde_json::Value = serde_json::from_str(&rewritten[config_index + 1]).unwrap();

        assert_eq!(config.pointer("/ocr/backend"), Some(&serde_json::json!("tesseract")));
        assert_eq!(config.pointer("/ocr/language"), Some(&serde_json::json!(["deu"])));
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM as i64)
        );
    }

    #[test]
    fn single_file_tesseract_override_rewrites_config_json_with_no_preexisting_ocr_key() {
        // BLOCKER 1 regression: the base `--config-json` (e.g. `NATIVE_BENCHMARK_CONFIG_JSON`)
        // has no `ocr` key at all, but `--ocr true` was force-upgraded in; the existing
        // `use_cache` field must survive the rewrite alongside the new materialized `ocr` key.
        let args = vec![
            "--config-json".to_string(),
            r#"{"extraction_timeout_secs":1740,"use_cache":false}"#.to_string(),
            "--ocr".to_string(),
            "true".to_string(),
            "--force-ocr".to_string(),
            "true".to_string(),
        ];

        let rewritten = apply_tesseract_ocr_override_to_args(&args, None).expect("CLI-only tesseract OCR to rewrite");
        let config_index = rewritten.iter().position(|arg| arg == "--config-json").unwrap();
        let config: serde_json::Value = serde_json::from_str(&rewritten[config_index + 1]).unwrap();

        assert_eq!(config.pointer("/use_cache"), Some(&serde_json::json!(false)));
        assert_eq!(config.pointer("/ocr/backend"), Some(&serde_json::json!("tesseract")));
        assert_eq!(config.pointer("/ocr/language"), Some(&serde_json::json!(["eng"])));
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/use_cache")
                .and_then(serde_json::Value::as_bool),
            Some(false)
        );
        assert_eq!(
            config
                .pointer("/ocr/tesseract_config/psm")
                .and_then(serde_json::Value::as_i64),
            Some(crate::adapter::XBERG_WHOLE_IMAGE_TESSERACT_PSM as i64)
        );
    }

    #[test]
    fn forced_ocr_without_nested_config_leaves_tesseract_config_for_downstream_materialization() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"use_cache":false}"#.to_string(),
            "--ocr".to_string(),
            "--force-ocr".to_string(),
            "true".to_string(),
        ];
        let ocr = effective_ocr_config_from_args(&args).expect("forced Tesseract OCR config");

        assert_eq!(ocr.pointer("/enabled"), Some(&serde_json::json!(true)));
        assert_eq!(ocr.pointer("/backend"), Some(&serde_json::json!("tesseract")));
        // `tesseract_config` must stay absent here: this raw config feeds `build_batch_file_configs`
        // (via `ocr_uses_tesseract` + `materialize_tesseract_ocr`), which is where the result cache
        // is genuinely disabled with the PSM matching the effective language. Synthesizing a bare
        // `{"use_cache": false}` here would deserialize with the Tesseract PSM default (3),
        // silently defeating xberg's own auto-PSM selection.
        assert_eq!(ocr.pointer("/tesseract_config"), None);
    }

    #[test]
    fn paddle_file_config_receives_fixture_language_without_tesseract_config() {
        let args = vec![
            "--config-json".to_string(),
            r#"{"ocr":{"enabled":true,"backend":"tesseract"}}"#.to_string(),
            "--ocr-backend".to_string(),
            "paddle-ocr".to_string(),
        ];

        let base_ocr = effective_ocr_config_from_args(&args).expect("Paddle OCR config");
        let configs = build_batch_file_configs(
            &[Path::new("sample.pdf")],
            &[Some(" deu + eng ".to_string())],
            Path::new("/tmp"),
            Some(&base_ocr),
        );
        let config = configs.get("/tmp/sample.pdf").expect("file config");

        assert_eq!(config.pointer("/ocr/backend"), Some(&serde_json::json!("paddle-ocr")));
        assert_eq!(
            config.pointer("/ocr/language"),
            Some(&serde_json::json!(["deu", "eng"]))
        );
        assert_eq!(config.pointer("/ocr/tesseract_config"), None);
    }

    #[test]
    fn paddle_single_file_receives_fixture_language() {
        let args = vec![
            "--ocr".to_string(),
            "true".to_string(),
            "--ocr-backend".to_string(),
            "paddle-ocr".to_string(),
        ];

        assert_eq!(
            xberg_ocr_language_args(&args, Some(" deu + eng ")),
            Some(["--ocr-language".to_string(), "deu+eng".to_string()])
        );
    }

    #[test]
    fn tesseract_single_file_language_forwarding_is_unchanged() {
        let args = vec![
            "--ocr".to_string(),
            "true".to_string(),
            "--ocr-backend".to_string(),
            "tesseract".to_string(),
        ];

        assert_eq!(
            xberg_ocr_language_args(&args, Some("jpn_vert")),
            Some(["--ocr-language".to_string(), "jpn_vert".to_string()])
        );
        // No `--config-json` is present, so `apply_tesseract_ocr_override_to_args` has nothing to
        // rewrite; `effective_ocr_config_from_args`'s CLI-flags-only synthesis leaves
        // `tesseract_config` absent for the same reason as the batch case (see
        // `forced_ocr_without_nested_config_leaves_tesseract_config_for_downstream_materialization`).
        let ocr = effective_ocr_config_from_args(&args).expect("Tesseract OCR config");
        assert_eq!(ocr.pointer("/tesseract_config"), None);
    }

    #[test]
    fn forced_ocr_upgrades_xberg_boolean_args() {
        let adapter = SubprocessAdapter::new(
            "xberg-markdown-baseline",
            "echo",
            vec!["--ocr".to_string(), "false".to_string()],
            vec![],
            vec!["pdf".to_string()],
        );

        let args = adapter.request_args(true);
        assert_eq!(&args[..2], ["--ocr", "true"]);
        assert!(args.windows(2).any(|pair| pair == ["--force-ocr", "true"]));
    }

    #[test]
    fn forced_ocr_preserves_enabled_xberg_json_config_without_cli_ocr_override() {
        let config = r#"{"use_cache":false,"ocr":{"enabled":true,"backend":"tesseract","tesseract_config":{"use_cache":false}}}"#;
        let adapter = SubprocessAdapter::new(
            "xberg-markdown-baseline",
            "echo",
            vec!["--config-json".to_string(), config.to_string()],
            vec![],
            vec!["pdf".to_string()],
        );

        let args = adapter.request_args(true);
        assert_eq!(args[1], config);
        assert!(!args.iter().any(|arg| arg == "--ocr"));
        assert!(args.windows(2).any(|pair| pair == ["--force-ocr", "true"]));
    }

    #[test]
    fn forced_ocr_adds_force_flag_to_xberg_json_ocr_config() {
        let adapter = SubprocessAdapter::new(
            "xberg-markdown-baseline",
            "echo",
            vec!["--config-json".to_string(), r#"{"ocr":{"enabled":true}}"#.to_string()],
            vec![],
            vec!["pdf".to_string()],
        );

        let args = adapter.request_args(true);
        assert!(args.windows(2).any(|pair| pair == ["--force-ocr", "true"]));
    }

    #[test]
    fn forced_ocr_upgrades_explicit_xberg_cli_override_despite_json_config() {
        let adapter = SubprocessAdapter::new(
            "xberg-markdown-baseline",
            "echo",
            vec![
                "--config-json".to_string(),
                r#"{"ocr":{"enabled":true}}"#.to_string(),
                "--ocr".to_string(),
                "false".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
        );

        let args = adapter.request_args(true);
        let ocr_index = args.iter().position(|arg| arg == "--ocr").unwrap();
        assert_eq!(args[ocr_index + 1], "true");
        assert!(args.windows(2).any(|pair| pair == ["--force-ocr", "true"]));
    }

    #[test]
    fn forced_ocr_adds_cli_ocr_for_native_xberg_config() {
        let adapter = SubprocessAdapter::new(
            "xberg-markdown-baseline",
            "echo",
            vec!["--config-json".to_string(), r#"{"use_cache":false}"#.to_string()],
            vec![],
            vec!["pdf".to_string()],
        );

        let args = adapter.request_args(true);
        assert!(args.iter().any(|arg| arg == "--ocr"));
        assert!(args.windows(2).any(|pair| pair == ["--force-ocr", "true"]));
    }

    #[test]
    fn forced_ocr_uses_existing_behavior_for_malformed_xberg_config_json() {
        let adapter = SubprocessAdapter::new(
            "xberg-markdown-baseline",
            "echo",
            vec!["--config-json".to_string(), "{malformed".to_string()],
            vec![],
            vec!["pdf".to_string()],
        );

        let args = adapter.request_args(true);
        assert!(args.iter().any(|arg| arg == "--ocr"));
        assert!(args.windows(2).any(|pair| pair == ["--force-ocr", "true"]));
    }

    #[test]
    fn throughput_uses_total_bytes_over_makespan() {
        assert_eq!(bytes_per_second(4_000, Duration::from_secs(2)), 2_000.0);
        assert_eq!(bytes_per_second(4_000, Duration::ZERO), 0.0);
    }

    #[tokio::test]
    async fn batch_rejects_force_ocr_cardinality_mismatch() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "echo",
            vec![],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        );
        let input = tempfile::NamedTempFile::new().unwrap();
        let error = adapter
            .extract_batch(
                &[input.path()],
                Duration::from_secs(1),
                &[],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("force_ocr cardinality mismatch"));
    }

    #[tokio::test]
    async fn batch_rejects_non_native_adapter_without_spawning_single_file_commands() {
        let adapter = SubprocessAdapter::new(
            "single-only",
            "command-that-must-not-run",
            vec![],
            vec![],
            vec!["pdf".to_string()],
        );
        let input = tempfile::NamedTempFile::new().unwrap();

        let error = adapter
            .extract_batch(
                &[input.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("verified native batch API"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn single_throughput_uses_wall_duration() {
        let adapter = SubprocessAdapter::new(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.05; printf '{\"content\":\"ok\",\"_extraction_time_ms\":1}'".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
        );
        let mut input = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut input, &[0; 100]).unwrap();

        let result = adapter
            .extract(
                input.path(),
                Duration::from_secs(1),
                false,
                None,
                OutputFormat::Markdown,
            )
            .await
            .unwrap();
        let expected = bytes_per_second(result.file_size, result.duration);
        assert!((result.metrics.throughput_bytes_per_sec - expected).abs() < f64::EPSILON);
        assert_eq!(result.extraction_duration, Some(Duration::from_millis(1)));
        assert_eq!(
            result.framework_capabilities.supported_extensions,
            vec!["pdf".to_string()]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_capable_single_zero_byte_result_is_marked_as_a_process_sample() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            "sh",
            vec!["-c".to_string(), "printf '{\"content\":\"ok\"}'".to_string()],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::DoclingJobkit,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        );
        let input = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract(
                input.path(),
                Duration::from_secs(1),
                false,
                None,
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert_eq!(result.metrics.throughput_bytes_per_sec, 0.0);
        assert_eq!(result.framework_capabilities.batch_performance_sample, Some(true));
        assert!(result.is_performance_sample());
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "current_thread")]
    async fn measured_command_drains_output_larger_than_pipe_capacity() {
        const CONTENT_BYTES: usize = 300_000;

        let adapter = SubprocessAdapter::new(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "printf '{\"content\":\"'; yes x | head -c 600000 | tr -d '\\n'; printf '\"}'".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
        );
        let input = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract(
                input.path(),
                Duration::from_secs(5),
                false,
                None,
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert!(result.success);
        assert_eq!(result.extracted_text.as_deref().map(str::len), Some(CONTENT_BYTES));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_rejects_output_cardinality_mismatch() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf '[{\"content\":\"only one\"}]'".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(false),
        );
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();
        let error = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, false],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("batch output cardinality mismatch"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn xberg_batch_envelope_uses_reported_timings_ocr_and_honest_throughput() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf '{\"results\":[{\"content\":\"one\",\"metadata\":{\"ocr_used\":false}},{\"content\":\"two\",\"metadata\":{\"ocr_used\":true}}],\"total_ms\":2000,\"per_file_ms\":[100,200]}'"
                    .to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        );
        let mut first = tempfile::NamedTempFile::new().unwrap();
        let mut second = tempfile::NamedTempFile::new().unwrap();
        std::io::Write::write_all(&mut first, &[0; 100]).unwrap();
        std::io::Write::write_all(&mut second, &[0; 300]).unwrap();
        let results = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, false],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| result.duration == Duration::from_secs(2)));
        assert_eq!(results[0].extraction_duration, Some(Duration::from_millis(100)));
        assert_eq!(results[1].extraction_duration, Some(Duration::from_millis(200)));
        assert!(
            results
                .iter()
                .all(|result| result.subprocess_overhead == Some(Duration::ZERO))
        );
        assert_eq!(results[0].ocr_status, OcrStatus::NotUsed);
        assert_eq!(results[1].ocr_status, OcrStatus::Used);
        assert_eq!(results[0].extracted_text.as_deref(), Some("one"));
        assert_eq!(results[1].extracted_text.as_deref(), Some("two"));
        assert_eq!(results[0].metrics.throughput_bytes_per_sec, 200.0);
        assert_eq!(results[1].metrics.throughput_bytes_per_sec, 200.0);
        assert!(
            results
                .iter()
                .all(|result| result.framework_capabilities.supported_extensions == ["pdf".to_string()])
        );
        assert_eq!(results[0].framework_capabilities.batch_performance_sample, Some(true));
        assert_eq!(results[1].framework_capabilities.batch_performance_sample, Some(false));
        assert_eq!(
            results[0].framework_capabilities.batch_sample_id,
            results[1].framework_capabilities.batch_sample_id
        );
        assert!(results[0].framework_capabilities.batch_sample_id.is_some());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_overhead_uses_process_wall_minus_reported_total() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.05; printf '{\"results\":[{\"content\":\"ok\"}],\"total_ms\":1,\"per_file_ms\":[1]}'"
                    .to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        );
        let file = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract_batch(
                &[file.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap()
            .remove(0);

        assert_eq!(
            result.subprocess_overhead,
            Some(result.duration.saturating_sub(Duration::from_millis(1)))
        );
        assert!(result.subprocess_overhead > Some(Duration::ZERO));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_array_does_not_infer_overhead_from_per_item_timing() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "printf '[{\"content\":\"ok\",\"_extraction_time_ms\":1}]'".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        );
        let file = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract_batch(
                &[file.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap()
            .remove(0);

        assert_eq!(result.extraction_duration, Some(Duration::from_millis(1)));
        assert_eq!(result.subprocess_overhead, None);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_envelope_preserves_unavailable_per_item_timings() {
        let capability = BatchCapability {
            entry_point: BatchEntryPoint::DoclingJobkit,
            timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: false,
        };
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf '{\"results\":[{\"content\":\"one\"},{\"content\":\"two\"}],\"total_ms\":10,\"per_file_ms\":[null,null]}'"
                    .to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            capability,
        );
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();

        let results = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, false],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert!(results.iter().all(|result| result.extraction_duration.is_none()));
        assert!(
            results
                .iter()
                .all(|result| { result.framework_capabilities.batch_capability == Some(capability) })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_rejects_numeric_timing_when_capability_declares_unavailable() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "docling",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf '{\"results\":[{\"content\":\"one\"}],\"total_ms\":10,\"per_file_ms\":[1]}'"
                    .to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            BatchCapability {
                entry_point: BatchEntryPoint::DoclingJobkit,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        );
        let input = tempfile::NamedTempFile::new().unwrap();

        let error = adapter
            .extract_batch(
                &[input.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("unavailable but returned numeric"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_requires_numeric_timing_when_capability_declares_per_item() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "xberg-test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf '{\"results\":[{\"content\":\"one\"}],\"total_ms\":10,\"per_file_ms\":[null]}'"
                    .to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        );
        let input = tempfile::NamedTempFile::new().unwrap();

        let error = adapter
            .extract_batch(
                &[input.path()],
                Duration::from_secs(1),
                &[false],
                &[None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("declares per-item batch timing"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_process_failure_preserves_measured_resource_stats() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf 'batch failed' >&2; exit 9".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(false),
        );
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();

        let results = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, false],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert_eq!(results.len(), 2);
        assert!(results.iter().all(|result| !result.success));
        assert!(results.iter().all(|result| {
            result
                .error_message
                .as_deref()
                .is_some_and(|error| error.contains("batch failed"))
        }));
        assert!(results.iter().all(|result| result.metrics.baseline_memory_bytes > 0));
        assert!(
            results
                .iter()
                .all(|result| { result.metrics.peak_memory_bytes >= result.metrics.baseline_memory_bytes })
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn partial_batch_item_failure_rejects_entire_batch() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "printf '[{\"content\":\"ok\"},{\"error\":\"failed item\"}]'".to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(false),
        );
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();

        let error = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, false],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("partial batch failure"));
        assert!(error.to_string().contains("failed item"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn batch_rejects_envelope_timing_cardinality_mismatch() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec![
                "-c".to_string(),
                "sleep 0.02; printf '{\"results\":[{\"content\":\"one\"},{\"content\":\"two\"}],\"total_ms\":10,\"per_file_ms\":[1]}'"
                    .to_string(),
            ],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(true),
        );
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();

        let error = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, false],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("batch timing cardinality mismatch"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn mixed_ocr_batch_is_rejected_as_non_comparable() {
        let adapter = SubprocessAdapter::with_batch_capability(
            "test",
            "sh",
            vec!["-c".to_string(), "exit 99".to_string()],
            vec![],
            vec!["pdf".to_string()],
            test_batch_capability(false),
        );
        let first = tempfile::NamedTempFile::new().unwrap();
        let second = tempfile::NamedTempFile::new().unwrap();

        let error = adapter
            .extract_batch(
                &[first.path(), second.path()],
                Duration::from_secs(1),
                &[false, true],
                &[None, None],
                OutputFormat::Markdown,
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("homogeneous OCR cohort"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn subprocess_timeout_kills_and_reaps_process_group() {
        let adapter = SubprocessAdapter::new(
            "timeout-test",
            "sh",
            vec!["-c".to_string(), "sleep 30 & wait".to_string()],
            vec![],
            vec!["pdf".to_string()],
        );
        let input = tempfile::NamedTempFile::new().unwrap();
        let start = Instant::now();

        let execution = adapter
            .execute_subprocess(
                input.path(),
                Duration::from_millis(50),
                false,
                None,
                OutputFormat::Markdown,
            )
            .await
            .unwrap();

        assert!(matches!(execution.error, Some(Error::Timeout(_))));
        assert!(execution.resource_stats.baseline_memory_bytes > 0);
        assert!(execution.resource_stats.peak_memory_bytes > 0);
        assert!(execution.resource_stats.sample_count > 0);
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn measured_command_timer_excludes_pre_spawn_staging() {
        let wall_start = Instant::now();
        tokio::time::sleep(Duration::from_millis(60)).await;
        let mut cmd = SubprocessAdapter::measured_command("sh");
        cmd.args(["-c", "sleep 0.02; printf ok"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SubprocessAdapter::configure_measured_stdin(&mut cmd);
        SubprocessAdapter::configure_child_process(&mut cmd);

        let outcome = SubprocessAdapter::execute_measured_command(
            &mut cmd,
            Duration::from_secs(1),
            "timer test",
            Duration::from_millis(1),
        )
        .await
        .unwrap();

        assert!(outcome.error.is_none());
        assert!(outcome.output.unwrap().status.success());
        assert!(outcome.resource_stats.baseline_memory_bytes > 0);
        assert!(outcome.resource_stats.peak_memory_bytes >= outcome.resource_stats.baseline_memory_bytes);
        assert!(outcome.resource_stats.sample_count > 0);
        assert!(wall_start.elapsed().saturating_sub(outcome.duration) >= Duration::from_millis(40));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn measured_ultrashort_command_is_not_measurable_from_blocked_shell_rss() {
        let mut cmd = SubprocessAdapter::measured_command("sh");
        cmd.args(["-c", "printf ok"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SubprocessAdapter::configure_measured_stdin(&mut cmd);
        SubprocessAdapter::configure_child_process(&mut cmd);

        let outcome = SubprocessAdapter::execute_measured_command(
            &mut cmd,
            Duration::from_secs(1),
            "ultrashort command",
            Duration::from_millis(100),
        )
        .await
        .unwrap();

        assert!(
            matches!(&outcome.error, Some(Error::Benchmark(message)) if message.contains("target sample")),
            "ultrashort command must fail RSS measurability: {:?}",
            outcome.error
        );
        assert!(outcome.output.unwrap().status.success());
        assert!(outcome.resource_stats.baseline_memory_bytes > 0);
        assert_eq!(outcome.resource_stats.peak_memory_bytes, 0);
        assert_eq!(outcome.resource_stats.sample_count, 0);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn timeout_error_surfaces_child_stderr_tail() {
        let mut cmd = SubprocessAdapter::measured_command("sh");
        cmd.args(["-c", "echo XBERG_HANG_SENTINEL 1>&2; sleep 5"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SubprocessAdapter::configure_measured_stdin(&mut cmd);
        SubprocessAdapter::configure_child_process(&mut cmd);

        let outcome = SubprocessAdapter::execute_measured_command(
            &mut cmd,
            Duration::from_millis(300),
            "hang probe",
            Duration::from_millis(20),
        )
        .await
        .unwrap();

        let error = outcome.error.expect("a timed-out subprocess must produce an error");
        let Error::Timeout(message) = &error else {
            panic!("expected Error::Timeout, got: {error:?}");
        };
        assert!(
            message.contains("XBERG_HANG_SENTINEL"),
            "timeout error must surface the hung child's stderr tail: {message}"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn measured_nonzero_exit_preserves_resource_stats() {
        let mut cmd = SubprocessAdapter::measured_command("sh");
        cmd.args(["-c", "sleep 0.02; exit 7"])
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        SubprocessAdapter::configure_measured_stdin(&mut cmd);
        SubprocessAdapter::configure_child_process(&mut cmd);

        let measured = SubprocessAdapter::execute_measured_command(
            &mut cmd,
            Duration::from_secs(1),
            "failing command",
            Duration::from_millis(1),
        )
        .await
        .unwrap();
        let execution = SubprocessAdapter::finish_measured_command(measured, "Failing command");

        assert!(matches!(execution.error, Some(Error::Benchmark(_))));
        assert!(execution.resource_stats.baseline_memory_bytes > 0);
        assert!(execution.resource_stats.peak_memory_bytes >= execution.resource_stats.baseline_memory_bytes);
        assert!(execution.resource_stats.sample_count > 0);
    }

    #[test]
    fn liteparse_staging_produces_a_readable_input() {
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"staged input").unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let destination = destination_dir.path().join("input.pdf");

        SubprocessAdapter::stage_liteparse_input(source.path(), &destination).unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"staged input");
    }

    #[cfg(unix)]
    #[test]
    fn liteparse_staging_uses_symlink_on_unix() {
        let source = tempfile::NamedTempFile::new().unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let destination = destination_dir.path().join("input.pdf");

        SubprocessAdapter::stage_liteparse_input(source.path(), &destination).unwrap();

        assert!(std::fs::symlink_metadata(destination).unwrap().file_type().is_symlink());
    }

    #[cfg(windows)]
    #[test]
    fn liteparse_staging_uses_windows_safe_link_or_copy() {
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"windows input").unwrap();
        let destination_dir = tempfile::tempdir().unwrap();
        let destination = destination_dir.path().join("input.pdf");

        SubprocessAdapter::stage_liteparse_input(source.path(), &destination).unwrap();

        assert_eq!(std::fs::read(destination).unwrap(), b"windows input");
    }
}
