use crate::{
    adapters::subprocess::SubprocessAdapter,
    error::Result,
    types::{BatchCapability, BatchEntryPoint, BatchTimingScope},
};
use std::time::Duration;
use std::{
    env,
    path::{Path, PathBuf},
};

use super::ocr_flag;

/// Maximum per-extraction timeout for persistent adapters (seconds).
const PERSISTENT_MAX_TIMEOUT_SECS: u64 = 180;

/// Higher timeout for slow ML frameworks (mineru, pymupdf4llm) that load
/// large models and can take significantly longer on first extractions.
const SLOW_ML_TIMEOUT_SECS: u64 = 300;

/// Margin between the Python-side and Rust-side timeouts.
/// The Python script handles timeouts internally (via multiprocessing fork),
/// reporting the result as a JSON error. The Rust-side timeout is a safety net
/// that only fires if the Python side fails to respond.
const PYTHON_TIMEOUT_MARGIN_SECS: u64 = 30;

/// Python-side extraction timeout passed via `--timeout=N` CLI arg.
const PYTHON_EXTRACTION_TIMEOUT_SECS: u64 = PERSISTENT_MAX_TIMEOUT_SECS - PYTHON_TIMEOUT_MARGIN_SECS;
const LITEPARSE_BINARY: &str = "lit";
const LITEPARSE_VERSION_PREFIX: &str = "lit ";
const LITEPARSE_REQUIRED_BATCH_OPTIONS: [&str; 3] = ["--format", "--no-ocr", "--num-workers"];
const MINERU_BATCH_CAPABILITY: BatchCapability = BatchCapability {
    entry_point: BatchEntryPoint::MineruDoParse,
    timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
    per_item_timing: false,
};

/// Helper function to define supported file types for each framework
///
/// Maps framework names to the file extensions they can actually process.
/// This prevents invalid benchmark combinations (e.g., Pandoc cannot read PDFs).
/// Format lists are based on comprehensive research of each framework's actual capabilities.
fn get_supported_formats(framework_name: &str) -> Vec<String> {
    match framework_name {
        "liteparse" => vec!["pdf".to_string()],

        "pymupdf4llm" => vec!["pdf", "epub", "svg", "txt", "png", "jpg", "jpeg", "bmp", "tiff", "tif"]
            .into_iter()
            .map(|s| s.to_string())
            .collect(),

        "docling" => vec![
            "pdf", "docx", "pptx", "xlsx", "html", "htm", "md", "markdown", "asciidoc", "csv", "jats", "vtt", "png",
            "jpg", "jpeg", "tiff", "tif", "bmp", "webp",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),

        "tika" => vec![
            "pdf", "docx", "doc", "pptx", "ppt", "ppsx", "pptm", "xlsx", "xls", "xlsm", "xlsb", "odt", "ods", "rtf",
            "epub", "html", "htm", "xml", "svg", "md", "txt", "csv", "tsv", "json", "yaml", "yml", "toml", "eml",
            "msg", "tex", "latex", "bib", "rst", "org", "ipynb", "png", "jpg", "jpeg", "gif", "bmp", "tiff", "tif",
            "webp", "jp2", "zip", "tar", "gz", "7z",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),

        "markitdown" => vec![
            "pdf", "docx", "pptx", "xlsx", "xls", "html", "htm", "xml", "csv", "json", "epub", "ipynb", "msg", "png",
            "jpg", "jpeg", "bmp", "tiff", "tif", "zip",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),

        "unstructured" => vec![
            "pdf", "docx", "doc", "pptx", "ppt", "xlsx", "xls", "odt", "rtf", "epub", "html", "htm", "xml", "md",
            "rst", "org", "txt", "csv", "tsv", "eml", "msg", "png", "jpg", "jpeg", "tiff", "tif", "bmp",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),

        "mineru" => vec!["pdf", "png", "jpg"].into_iter().map(|s| s.to_string()).collect(),

        _ => vec![
            "pdf", "docx", "doc", "xlsx", "xls", "pptx", "ppt", "txt", "md", "html", "xml", "json",
        ]
        .into_iter()
        .map(|s| s.to_string())
        .collect(),
    }
}

/// Creates a subprocess adapter for Docling.
///
/// Uses wrapper script approach for extraction.
pub fn create_docling_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let script_path = get_script_path("docling_extract.py")?;
    let (command, mut args) = find_python_with_framework("docling")?;
    args.push(script_path.to_string_lossy().to_string());
    args.push(format!("--timeout={}", PYTHON_EXTRACTION_TIMEOUT_SECS));
    args.push(ocr_flag(ocr_enabled));
    let mut single_file_args = args.clone();
    single_file_args.push("sync".to_string());
    args.push("batch".to_string());

    let supported_formats = get_supported_formats("docling");
    Ok(SubprocessAdapter::with_batch_capability(
        "docling",
        command,
        args,
        vec![],
        supported_formats,
        BatchCapability {
            entry_point: BatchEntryPoint::DoclingConvertAll,
            timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: false,
        },
    )
    .with_configured_ocr(ocr_enabled)
    .with_format_aware(true)
    .with_single_file_args(single_file_args)
    .with_max_timeout(Duration::from_secs(PERSISTENT_MAX_TIMEOUT_SECS)))
}

/// Creates a subprocess adapter for Unstructured.
///
/// Uses wrapper script approach for extraction.
pub fn create_unstructured_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let script_path = get_script_path("unstructured_extract.py")?;
    let (command, mut args) = find_python_with_framework("unstructured")?;
    args.push(script_path.to_string_lossy().to_string());
    args.push(format!("--timeout={}", PYTHON_EXTRACTION_TIMEOUT_SECS));
    args.push(ocr_flag(ocr_enabled));
    args.push("sync".to_string());

    let supported_formats = get_supported_formats("unstructured");
    Ok(
        SubprocessAdapter::new("unstructured", command, args, vec![], supported_formats)
            .with_configured_ocr(ocr_enabled)
            .with_format_aware(true)
            .with_max_timeout(Duration::from_secs(PERSISTENT_MAX_TIMEOUT_SECS)),
    )
}

/// Creates a subprocess adapter for MarkItDown
pub fn create_markitdown_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let script_path = get_script_path("markitdown_extract.py")?;
    let (command, mut args) = find_python_with_framework("markitdown")?;
    args.push(script_path.to_string_lossy().to_string());
    args.push(format!("--timeout={}", PYTHON_EXTRACTION_TIMEOUT_SECS));
    args.push(ocr_flag(ocr_enabled));
    args.push("sync".to_string());

    let supported_formats = get_supported_formats("markitdown");
    Ok(
        SubprocessAdapter::new("markitdown", command, args, vec![], supported_formats)
            .with_configured_ocr(ocr_enabled)
            .with_max_timeout(Duration::from_secs(PERSISTENT_MAX_TIMEOUT_SECS)),
    )
}

/// Creates a subprocess adapter for LiteParse (run-llama/liteparse) Rust CLI.
///
/// Requires the `lit` binary on PATH. Install with `cargo install liteparse`.
///
/// Supports:
/// - Single-file mode: `lit parse <file> --format text|markdown` per file
/// - Batch mode: `lit batch-parse <input_dir> <output_dir> --format text|markdown`
/// - Both plaintext and markdown output formats
pub fn create_liteparse_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let liteparse_command = which::which(LITEPARSE_BINARY).map_err(|_| {
        crate::Error::Config("lit (liteparse) not found. Install with: cargo install liteparse".to_string())
    })?;
    verify_liteparse_cli(&liteparse_command)?;

    let script_path = get_script_path("liteparse_extract.sh")?;
    let command = PathBuf::from("bash");
    let mut args = vec![script_path.to_string_lossy().to_string()];
    args.push(ocr_flag(ocr_enabled));

    let supported_formats = get_supported_formats("liteparse");
    Ok(SubprocessAdapter::with_batch_capability(
        "liteparse",
        command,
        args,
        vec![],
        supported_formats,
        BatchCapability {
            entry_point: BatchEntryPoint::LiteparseBatchParse,
            timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
            per_item_timing: false,
        },
    )
    .with_configured_ocr(ocr_enabled)
    .with_max_timeout(Duration::from_secs(PERSISTENT_MAX_TIMEOUT_SECS))
    .with_native_batch_command(liteparse_command)
    .with_format_aware(true))
}

fn verify_liteparse_cli(command: &Path) -> Result<()> {
    let version = std::process::Command::new(command)
        .arg("--version")
        .output()
        .map_err(|error| crate::Error::Config(format!("failed to run '{} --version': {error}", command.display())))?;
    let version_text = String::from_utf8_lossy(&version.stdout);
    let valid_version = version.status.success()
        && version_text
            .lines()
            .next()
            .is_some_and(|line| line.trim().starts_with(LITEPARSE_VERSION_PREFIX));
    if !valid_version {
        return Err(crate::Error::Config(format!(
            "'{}' is not the LiteParse CLI: expected a successful 'lit <version>' identity probe",
            command.display()
        )));
    }

    let batch_help = std::process::Command::new(command)
        .args(["batch-parse", "--help"])
        .output()
        .map_err(|error| {
            crate::Error::Config(format!(
                "failed to probe LiteParse native batch capability at '{}': {error}",
                command.display()
            ))
        })?;
    let help_text = format!(
        "{}\n{}",
        String::from_utf8_lossy(&batch_help.stdout),
        String::from_utf8_lossy(&batch_help.stderr)
    );
    if !batch_help.status.success()
        || LITEPARSE_REQUIRED_BATCH_OPTIONS
            .iter()
            .any(|required| !help_text.contains(required))
    {
        return Err(crate::Error::Config(format!(
            "LiteParse CLI '{}' does not expose the required native batch-parse interface",
            command.display()
        )));
    }

    Ok(())
}

/// Helper function to get the path to a wrapper script
/// Handles both development (source tree) and CI (downloaded artifact) environments
fn get_script_path(script_name: &str) -> Result<PathBuf> {
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let script_path = PathBuf::from(manifest_dir).join("scripts").join(script_name);
        if script_path.exists() {
            return Ok(script_path);
        }
    }

    let script_path = PathBuf::from("tools/benchmark-harness/scripts").join(script_name);
    if script_path.exists() {
        return Ok(script_path);
    }

    if let Ok(exe_path) = std::env::current_exe()
        && let Some(exe_dir) = exe_path.parent()
    {
        let script_path = exe_dir
            .join("..")
            .join("..")
            .join("tools")
            .join("benchmark-harness")
            .join("scripts")
            .join(script_name);
        if script_path.exists() {
            return Ok(script_path.canonicalize().unwrap_or(script_path));
        }
    }

    if let Ok(scripts_dir) = env::var("BENCHMARK_HARNESS_SCRIPTS_DIR") {
        let script_path = PathBuf::from(scripts_dir).join(script_name);
        if script_path.exists() {
            return Ok(script_path);
        }
    }

    Err(crate::error::Error::Config(format!(
        "Script not found: {}. Checked: CARGO_MANIFEST_DIR/scripts, \
         tools/benchmark-harness/scripts, relative to binary, and BENCHMARK_HARNESS_SCRIPTS_DIR. \
         Ensure the harness is run from the repository root or set BENCHMARK_HARNESS_SCRIPTS_DIR.",
        script_name
    )))
}

/// Environment override for the Python interpreter used by external benchmarks.
const BENCH_PYTHON_ENV: &str = "XBERG_BENCH_PYTHON";

fn python_imports_framework(python: &PathBuf, framework: &str) -> bool {
    std::process::Command::new(python)
        .arg("-c")
        .arg(format!("import {framework}"))
        .output()
        .is_ok_and(|output| output.status.success())
}

fn direct_python_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(virtual_env) = env::var_os("VIRTUAL_ENV") {
        let virtual_env = PathBuf::from(virtual_env);
        candidates.push(virtual_env.join("bin/python"));
        candidates.push(virtual_env.join("Scripts/python.exe"));
    }
    candidates.push(PathBuf::from(".venv/bin/python"));
    candidates.push(PathBuf::from(".venv/Scripts/python.exe"));
    for executable in ["python3", "python"] {
        if let Ok(path) = which::which(executable) {
            candidates.push(path);
        }
    }
    candidates
}

fn resolve_uv_python(framework: &str) -> Result<Option<PathBuf>> {
    let Ok(uv) = which::which("uv") else {
        return Ok(None);
    };
    let dependency_group = format!("bench-{framework}");
    let probe_script = format!("import {framework}, sys; print(sys.executable)");
    let probe = std::process::Command::new(uv)
        .args([
            "run",
            "--locked",
            "--no-sync",
            "--group",
            &dependency_group,
            "python",
            "-c",
            &probe_script,
        ])
        .output()
        .map_err(|error| crate::error::Error::Config(format!("failed to run uv probe for {framework}: {error}")))?;
    if !probe.status.success() {
        return Err(crate::error::Error::Config(format!(
            "uv environment cannot import {framework}: {}",
            String::from_utf8_lossy(&probe.stderr).trim()
        )));
    }

    let python = String::from_utf8(probe.stdout).map_err(|error| {
        crate::error::Error::Config(format!(
            "uv returned a non-UTF-8 interpreter path for {framework}: {error}"
        ))
    })?;
    let Some(interpreter) = python.lines().last().map(str::trim).filter(|line| !line.is_empty()) else {
        return Err(crate::error::Error::Config(format!(
            "uv returned no interpreter path for {framework}"
        )));
    };
    let path = PathBuf::from(interpreter);
    if !path.is_file() || !python_imports_framework(&path, framework) {
        return Err(crate::error::Error::Config(format!(
            "uv resolved {} but it cannot import {framework}",
            path.display()
        )));
    }
    Ok(Some(path))
}

/// Helper function to find Python interpreter with a specific open source extraction framework installed.
///
/// Returns a direct interpreter path so per-document timings never include `uv`
/// dependency resolution, project synchronization, or editable-wheel builds.
fn find_python_with_framework(framework: &str) -> Result<(PathBuf, Vec<String>)> {
    if let Some(path) = env::var_os(BENCH_PYTHON_ENV) {
        let python = PathBuf::from(path);
        if python_imports_framework(&python, framework) {
            return Ok((python, Vec::new()));
        }
        return Err(crate::error::Error::Config(format!(
            "{BENCH_PYTHON_ENV}={} cannot import {framework}",
            python.display()
        )));
    }

    for python in direct_python_candidates() {
        if python_imports_framework(&python, framework) {
            return Ok((python, Vec::new()));
        }
    }

    if let Some(python) = resolve_uv_python(framework)? {
        return Ok((python, Vec::new()));
    }

    Err(crate::error::Error::Config(format!(
        "No Python interpreter can import {framework}. Run `uv sync --locked --group \
         bench-{framework}` or set {BENCH_PYTHON_ENV} to a prepared interpreter"
    )))
}

/// Helper to find Java runtime
fn find_java() -> Result<PathBuf> {
    which::which("java").map_err(|_| crate::Error::Config("Java runtime not found".to_string()))
}

/// Helper to locate Tika JAR (auto-detect from libs/ or env var)
fn get_tika_jar_path() -> Result<PathBuf> {
    if let Ok(manifest_dir) = env::var("CARGO_MANIFEST_DIR") {
        let lib_dir = PathBuf::from(manifest_dir).join("libs");
        if let Ok(entries) = std::fs::read_dir(&lib_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if let Some(name) = path.file_name().and_then(|n| n.to_str())
                    && name.starts_with("tika-app-")
                    && name.ends_with(".jar")
                {
                    return Ok(path);
                }
            }
        }
    }

    let fallback_lib_dir = PathBuf::from("tools/benchmark-harness/libs");
    if let Ok(entries) = std::fs::read_dir(&fallback_lib_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.starts_with("tika-app-")
                && name.ends_with(".jar")
            {
                return Ok(path);
            }
        }
    }

    if let Ok(jar_path) = env::var("TIKA_JAR") {
        let path = PathBuf::from(jar_path);
        if path.exists() {
            return Ok(path);
        }
    }

    let version = env::var("TIKA_VERSION").unwrap_or_else(|_| "3.2.3".to_string());
    Err(crate::Error::Config(format!(
        "Tika JAR not found. Download: curl -fsSL -o tools/benchmark-harness/libs/tika-app-{version}.jar https://repo1.maven.org/maven2/org/apache/tika/tika-app/{version}/tika-app-{version}.jar"
    )))
}

/// Helper to ensure TikaExtract.class is compiled
/// Compiles TikaExtract.java if .class file doesn't exist, and returns the directory containing the class
fn ensure_tika_extract_compiled(java_path: &PathBuf, tika_jar_path: &PathBuf) -> Result<PathBuf> {
    let script_path = get_script_path("TikaExtract.java")?;

    let compile_dir = PathBuf::from("target").join("tika-extract-classes");
    std::fs::create_dir_all(&compile_dir)
        .map_err(|e| crate::Error::Config(format!("Failed to create compile directory: {}", e)))?;

    let class_path = compile_dir
        .join("dev")
        .join("xberg")
        .join("benchmark")
        .join("TikaExtract.class");

    if !class_path.exists() {
        let output = std::process::Command::new(java_path)
            .arg("-version")
            .output()
            .map_err(|e| crate::Error::Config(format!("Failed to check Java version: {}", e)))?;

        if !output.status.success() {
            return Err(crate::Error::Config("Java is not properly installed".to_string()));
        }

        let compile_output = std::process::Command::new("javac")
            .arg("-cp")
            .arg(tika_jar_path)
            .arg("-d")
            .arg(&compile_dir)
            .arg(&script_path)
            .output()
            .map_err(|e| crate::Error::Config(format!("Failed to compile TikaExtract.java: {}", e)))?;

        if !compile_output.status.success() {
            let stderr = String::from_utf8_lossy(&compile_output.stderr);
            return Err(crate::Error::Config(format!(
                "TikaExtract.java compilation failed: {}",
                stderr
            )));
        }
    }

    Ok(compile_dir)
}

/// Creates a subprocess adapter for Apache Tika (persistent server mode)
///
/// Uses Tika via compiled Java class approach for extraction.
pub fn create_tika_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let jar_path = get_tika_jar_path()?;
    let command = find_java()?;
    let compile_dir = ensure_tika_extract_compiled(&command, &jar_path)?;

    #[cfg(target_os = "windows")]
    let classpath = format!("{};{}", compile_dir.display(), jar_path.display());
    #[cfg(not(target_os = "windows"))]
    let classpath = format!("{}:{}", compile_dir.display(), jar_path.display());

    let args = vec![
        "-server".to_string(),
        "-Xms512m".to_string(),
        "-Xmx2g".to_string(),
        "-XX:+UseG1GC".to_string(),
        "-cp".to_string(),
        classpath,
        "io.xberg.benchmark.TikaExtract".to_string(),
        ocr_flag(ocr_enabled),
        "sync".to_string(),
    ];

    let supported_formats = get_supported_formats("tika");
    Ok(SubprocessAdapter::new("tika", command, args, vec![], supported_formats)
        .with_configured_ocr(ocr_enabled)
        .with_supported_output_formats(vec![crate::types::OutputFormat::Plaintext])
        .with_max_timeout(Duration::from_secs(180)))
}

/// Creates a subprocess adapter for PyMuPDF4LLM
pub fn create_pymupdf4llm_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let script_path = get_script_path("pymupdf4llm_extract.py")?;
    let (command, mut args) = find_python_with_framework("pymupdf4llm")?;
    args.push(script_path.to_string_lossy().to_string());
    args.push(format!("--timeout={}", PYTHON_EXTRACTION_TIMEOUT_SECS));
    args.push(ocr_flag(ocr_enabled));
    args.push("sync".to_string());

    let supported_formats = get_supported_formats("pymupdf4llm");
    Ok(
        SubprocessAdapter::new("pymupdf4llm", command, args, vec![], supported_formats)
            .with_configured_ocr(ocr_enabled)
            .with_max_timeout(Duration::from_secs(SLOW_ML_TIMEOUT_SECS)),
    )
}

/// Creates a subprocess adapter for MinerU 3.4.4.
///
/// Batch mode invokes `mineru.cli.common.do_parse` once, which delegates to
/// `doc_analyze_streaming` for cross-document model batching.
pub fn create_mineru_adapter(ocr_enabled: bool) -> Result<SubprocessAdapter> {
    let script_path = get_script_path("mineru_extract.py")?;
    let (command, mut args) = find_python_with_framework("mineru")?;
    args.push(script_path.to_string_lossy().to_string());
    args.push(format!("--timeout={}", PYTHON_EXTRACTION_TIMEOUT_SECS));
    args.push(ocr_flag(ocr_enabled));
    let mut single_file_args = args.clone();
    single_file_args.push("sync".to_string());
    args.push("batch".to_string());

    let supported_formats = get_supported_formats("mineru");
    Ok(SubprocessAdapter::with_batch_capability(
        "mineru",
        command,
        args,
        vec![],
        supported_formats,
        MINERU_BATCH_CAPABILITY,
    )
    .with_configured_ocr(ocr_enabled)
    .with_format_aware(true)
    .with_single_file_args(single_file_args)
    .with_max_timeout(Duration::from_secs(SLOW_ML_TIMEOUT_SECS)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adapter::FrameworkAdapter;

    #[cfg(unix)]
    fn fake_liteparse(script_body: &str) -> (tempfile::TempDir, PathBuf) {
        use std::os::unix::fs::PermissionsExt;

        let temp = tempfile::tempdir().expect("temporary directory");
        let command = temp.path().join("lit");
        std::fs::write(&command, format!("#!/bin/sh\n{script_body}\n")).expect("write fake LiteParse CLI");
        let mut permissions = std::fs::metadata(&command).expect("fake CLI metadata").permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&command, permissions).expect("make fake CLI executable");
        (temp, command)
    }

    #[cfg(unix)]
    #[test]
    fn liteparse_probe_requires_identity_and_native_batch_options() {
        let (_temp, command) = fake_liteparse(
            r#"
if [ "$1" = "--version" ]; then
  echo "lit 2.8.0"
  exit 0
fi
if [ "$1" = "batch-parse" ] && [ "$2" = "--help" ]; then
  echo "--format --no-ocr --num-workers"
  exit 0
fi
exit 2
"#,
        );

        assert!(verify_liteparse_cli(&command).is_ok());
    }

    #[cfg(unix)]
    #[test]
    fn liteparse_probe_rejects_version_only_cli() {
        let (_temp, command) = fake_liteparse(
            r#"
if [ "$1" = "--version" ]; then
  echo "lit 2.8.0"
  exit 0
fi
exit 2
"#,
        );

        let error = verify_liteparse_cli(&command).expect_err("batch capability must be verified");
        assert!(error.to_string().contains("native batch-parse interface"));
    }

    #[cfg(unix)]
    #[test]
    fn liteparse_probe_rejects_unrelated_lit_binary() {
        let (_temp, command) = fake_liteparse(
            r#"
if [ "$1" = "--version" ]; then
  echo "literal-tool 1.0.0"
  exit 0
fi
exit 2
"#,
        );

        let error = verify_liteparse_cli(&command).expect_err("identity must be verified");
        assert!(error.to_string().contains("not the LiteParse CLI"));
    }

    #[test]
    fn docling_batch_wrapper_conformance_runs_with_cargo_tests() {
        let python = std::env::var_os("PYTHON").unwrap_or_else(|| "python3".into());
        let script = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("test_docling_extract.py");
        let output = std::process::Command::new(python)
            .arg(script)
            .output()
            .expect("Python is required to validate the Docling benchmark wrapper");

        assert!(
            output.status.success(),
            "Docling wrapper conformance failed:\nstdout:\n{}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[test]
    fn test_get_script_path() {
        let result = get_script_path("docling_extract.py");
        assert!(result.is_ok() || result.is_err());
    }

    #[tokio::test]
    async fn test_adapter_creation() {
        if let Ok(docling) = create_docling_adapter(true) {
            assert_eq!(
                docling.batch_capability(),
                Some(BatchCapability {
                    entry_point: BatchEntryPoint::DoclingConvertAll,
                    timing_scope: BatchTimingScope::ColdEndToEndSubprocess,
                    per_item_timing: false,
                })
            );
        }
        if let Ok(mineru) = create_mineru_adapter(true) {
            assert_eq!(mineru.batch_capability(), Some(MINERU_BATCH_CAPABILITY));
        }
        let _ = create_unstructured_adapter(true);
        let _ = create_markitdown_adapter(true);
        let _ = create_tika_adapter(true);
        let _ = create_pymupdf4llm_adapter(true);
        let _ = create_liteparse_adapter(true);
    }

    #[test]
    fn python_resolver_returns_direct_importing_interpreter() {
        let (python, args) = find_python_with_framework("sys").expect("Python must import sys");
        assert_ne!(python.file_name().and_then(|name| name.to_str()), Some("uv"));
        assert!(args.is_empty());
        assert!(python_imports_framework(&python, "sys"));
    }

    #[test]
    fn python_resolver_rejects_missing_framework() {
        let result = find_python_with_framework("xberg_benchmark_missing_framework");
        assert!(result.is_err());
    }
}
