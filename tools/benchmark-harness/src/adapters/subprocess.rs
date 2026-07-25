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
    /// Explicit `--max-threads` budget passed only to Xberg's native batch CLI.
    ///
    /// When unset, Xberg receives [`Self::batch_workers`] to preserve the
    /// legacy coupled behavior.
    xberg_max_threads: Option<usize>,
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

    fn single_file_request_args(&self, force_ocr: bool) -> Vec<String> {
        self.request_args_from(self.single_file_args.as_deref().unwrap_or(&self.args), force_ocr)
    }

    fn resolve_ocr_status(&self, value: Option<&serde_json::Value>, force_ocr: bool) -> OcrStatus {
        value
            .and_then(serde_json::Value::as_bool)
            .map(|used| if used { OcrStatus::Used } else { OcrStatus::NotUsed })
            .or_else(|| force_ocr.then_some(OcrStatus::Used))
            .or(self.configured_ocr_status)
            .unwrap_or(OcrStatus::Unknown)
    }

    fn timeout_error(operation: &str, timeout: Duration) -> Error {
        #[cfg(windows)]
        let cleanup = "; Windows timeout cleanup terminates the direct child only; descendant cleanup is unsupported";
        #[cfg(not(windows))]
        let cleanup = "";
        Error::Timeout(format!("{operation} exceeded {timeout:?}{cleanup}"))
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
                    #[cfg(unix)]
                    {
                        Self::kill_process_group(child_pid);
                        let _ = wait.await;
                    }
                    (None, Some(Self::timeout_error(operation, timeout)), duration)
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
            Some(BatchEntryPoint::DoclingConvertAll) => b"docling-convert-all",
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
        let request_args = self.single_file_request_args(force_ocr);
        cmd.args(&request_args);

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
        cmd.args(self.request_args(force_ocr));

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
            eprintln!(
                "[parse_output:{}] raw_len={} preview={}",
                self.name,
                stdout.len(),
                preview.trim()
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
        let args = match mode {
            crate::config::BenchmarkMode::SingleFile => self.single_file_args.as_deref().unwrap_or(&self.args),
            crate::config::BenchmarkMode::Batch => &self.args,
        };
        Some(crate::provenance::ExecutableProvenance::from_invocation(
            &self.command,
            args,
        ))
    }

    fn worker_provenance(&self, requested: usize) -> (Option<usize>, Option<usize>) {
        match self.batch_capability.map(|capability| capability.entry_point) {
            Some(crate::types::BatchEntryPoint::DoclingConvertAll) => (None, None),
            Some(crate::types::BatchEntryPoint::MineruDoParse) => (None, None),
            Some(crate::types::BatchEntryPoint::XbergCliExtractBatch) => (Some(requested), None),
            Some(crate::types::BatchEntryPoint::LiteparseBatchParse) => (Some(requested), Some(self.batch_workers)),
            None => (Some(requested), Some(requested)),
        }
    }

    fn configured_thread_budget(&self) -> Option<usize> {
        self.batch_capability
            .is_some_and(|capability| capability.entry_point == BatchEntryPoint::XbergCliExtractBatch)
            .then(|| self.effective_xberg_max_threads())
    }

    async fn extract(
        &self,
        file_path: &Path,
        timeout: Duration,
        force_ocr: bool,
        output_format: OutputFormat,
    ) -> Result<BenchmarkResult> {
        let timeout = self.effective_timeout(timeout);
        let file_size = std::fs::metadata(file_path).map_err(Error::Io)?.len();

        let start_time = std::time::Instant::now();

        let execution = match self
            .execute_subprocess(file_path, timeout, force_ocr, output_format)
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
            eprintln!(
                "[extract:{}] _extraction_time_ms raw={:?}, keys={:?}",
                self.name,
                extraction_time_raw,
                parsed.as_object().map(|o| o.keys().collect::<Vec<_>>())
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
                throughput_bytes_per_sec: throughput,
                p50_memory_bytes: resource_stats.p50_memory_bytes,
                p95_memory_bytes: resource_stats.p95_memory_bytes,
                p99_memory_bytes: resource_stats.p99_memory_bytes,
            },
        };

        let ocr_status = self.resolve_ocr_status(parsed.get("_ocr_used"), force_ocr);

        let framework_capabilities = FrameworkCapabilities {
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
                page_count: None,
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
            Some(crate::types::BatchEntryPoint::DoclingConvertAll) => std::process::Command::new(&self.command)
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
            .execute_subprocess_batch(file_paths, timeout, batch_force_ocr, output_format)
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
                    pdf_metadata: None,
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
                entry_point: BatchEntryPoint::DoclingConvertAll,
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
                entry_point: BatchEntryPoint::DoclingConvertAll,
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
                entry_point: BatchEntryPoint::DoclingConvertAll,
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
            .extract_batch(&[file.path()], Duration::from_secs(1), &[false], OutputFormat::Markdown)
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
            .extract_batch(&[file.path()], Duration::from_secs(1), &[false], OutputFormat::Markdown)
            .await
            .unwrap();

        assert_eq!(results.len(), 1);
        assert!(results[0].success);
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
            .extract_batch(&[input.path()], Duration::from_secs(1), &[], OutputFormat::Markdown)
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
            .extract(input.path(), Duration::from_secs(1), false, OutputFormat::Markdown)
            .await
            .unwrap();
        let expected = bytes_per_second(result.file_size, result.duration);
        assert!((result.metrics.throughput_bytes_per_sec - expected).abs() < f64::EPSILON);
        assert_eq!(result.extraction_duration, Some(Duration::from_millis(1)));
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
                entry_point: BatchEntryPoint::DoclingConvertAll,
                timing_scope: crate::types::BatchTimingScope::ColdEndToEndSubprocess,
                per_item_timing: false,
            },
        );
        let input = tempfile::NamedTempFile::new().unwrap();

        let result = adapter
            .extract(input.path(), Duration::from_secs(1), false, OutputFormat::Markdown)
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
            .extract(input.path(), Duration::from_secs(5), false, OutputFormat::Markdown)
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
            .extract_batch(&[file.path()], Duration::from_secs(1), &[false], OutputFormat::Markdown)
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
            .extract_batch(&[file.path()], Duration::from_secs(1), &[false], OutputFormat::Markdown)
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
            entry_point: BatchEntryPoint::DoclingConvertAll,
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
                entry_point: BatchEntryPoint::DoclingConvertAll,
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
            .execute_subprocess(input.path(), Duration::from_millis(50), false, OutputFormat::Markdown)
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
