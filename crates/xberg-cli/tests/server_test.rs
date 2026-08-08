//! Integration tests for server commands (serve and mcp).

#[cfg(not(coverage))]
use std::path::PathBuf;
#[cfg(not(coverage))]
use std::process::{Command, Stdio};
#[cfg(not(coverage))]
use std::thread;
#[cfg(not(coverage))]
use std::time::Duration;

/// Resolves the target directory used by these tests' explicit `cargo build` invocations.
///
/// These tests rebuild the binary with `--features all` (a superset of the default features
/// this test target itself is compiled with, needed for the `serve`/`mcp` subcommands), so
/// `env!("CARGO_BIN_EXE_xberg")` cannot be used here — it points at the default-feature binary.
/// `CARGO_TARGET_DIR`, when set (the standing workaround for concurrent-agent target-dir
/// contention in this repo), is honored so the rebuilt binary is found where cargo actually
/// placed it.
#[cfg(not(coverage))]
fn target_dir() -> PathBuf {
    std::env::var("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"))
}

#[cfg(not(coverage))]
fn xberg_debug_binary() -> PathBuf {
    target_dir().join("debug").join("xberg")
}

#[cfg(not(coverage))]
#[test]
#[ignore]
fn test_serve_command_starts() {
    let status = Command::new("cargo")
        .args(["build", "--bin", "xberg", "--features", "all"])
        .status()
        .expect("Failed to build binary");

    assert!(status.success(), "Failed to build xberg binary");

    let mut child = Command::new(xberg_debug_binary())
        .args(["serve", "-H", "127.0.0.1", "-p", "18000"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start server");

    thread::sleep(Duration::from_secs(3));

    let mut health_response = ureq::get("http://127.0.0.1:18000/health")
        .call()
        .expect("Failed to call health endpoint");

    assert_eq!(health_response.status(), 200);

    let health_json: serde_json::Value = health_response
        .body_mut()
        .read_json()
        .expect("Failed to parse health response");

    assert_eq!(health_json["status"], "healthy");
    assert!(health_json["version"].is_string());

    let mut info_response = ureq::get("http://127.0.0.1:18000/info")
        .call()
        .expect("Failed to call info endpoint");

    assert_eq!(info_response.status(), 200);

    let info_json: serde_json::Value = info_response
        .body_mut()
        .read_json()
        .expect("Failed to parse info response");

    assert!(info_json["rust_backend"].as_bool().unwrap_or(false));

    child.kill().expect("Failed to kill server");
    child.wait().expect("Failed to wait for server");
}

#[cfg(not(coverage))]
#[test]
#[ignore]
fn test_serve_command_with_config() {
    use std::fs;

    let config_content = r#"
use_cache = true
enable_quality_processing = true

[ocr]
backend = "tesseract"
language = "eng"
"#;

    fs::write("test_config.toml", config_content).expect("Failed to write test config");

    let mut child = Command::new(xberg_debug_binary())
        .args(["serve", "-H", "127.0.0.1", "-p", "18001", "-c", "test_config.toml"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("Failed to start server");

    thread::sleep(Duration::from_secs(3));

    let health_response = ureq::get("http://127.0.0.1:18001/health").call();

    assert!(health_response.is_ok(), "Server should be running with custom config");

    child.kill().expect("Failed to kill server");
    child.wait().expect("Failed to wait for server");

    fs::remove_file("test_config.toml").ok();
}

#[cfg(not(coverage))]
#[test]
fn test_serve_command_help() {
    let build_status = Command::new("cargo")
        .args(["build", "--bin", "xberg", "--features", "all"])
        .status()
        .expect("Failed to build binary");

    assert!(build_status.success(), "Failed to build xberg binary");

    let output = Command::new(xberg_debug_binary())
        .args(["serve", "--help"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Start the API server"));
    assert!(stdout.contains("--host"));
    assert!(stdout.contains("--port"));
    assert!(stdout.contains("--config"));
}

#[cfg(not(coverage))]
#[test]
fn test_mcp_command_help() {
    let build_status = Command::new("cargo")
        .args(["build", "--bin", "xberg", "--features", "all"])
        .status()
        .expect("Failed to build binary");

    assert!(build_status.success(), "Failed to build xberg binary");

    let output = Command::new(xberg_debug_binary())
        .args(["mcp", "--help"])
        .output()
        .expect("Failed to execute command");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Start the MCP (Model Context Protocol) server"));
    assert!(stdout.contains("--config"));
}
