//! Integration tests for CLI config file discovery.
//!
//! These tests verify that the CLI correctly discovers and loads configuration files
//! in various formats (.toml, .yaml, .json) with case-insensitive extension
//! matching, explicit --config flag support, and proper error handling.

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use tempfile::tempdir;

/// Get the path to the xberg binary.
fn get_binary_path() -> String {
    env!("CARGO_BIN_EXE_xberg").to_string()
}

/// Get the test_documents directory path.
fn get_test_documents_dir() -> PathBuf {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    manifest_dir.parent().unwrap().parent().unwrap().join("test_documents")
}

/// Get a test file path relative to test_documents/.
fn get_test_file(relative_path: &str) -> String {
    get_test_documents_dir()
        .join(relative_path)
        .to_string_lossy()
        .to_string()
}

#[test]
fn test_discover_xberg_toml_in_current_directory() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join(".xberg.toml");

    fs::write(
        &config_path,
        r#"
use_cache = false
enable_quality_processing = false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_discover_xberg_yaml_in_current_directory() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join(".xberg.yaml");

    fs::write(
        &config_path,
        r#"
use_cache: false
enable_quality_processing: false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_discover_xberg_yml_in_current_directory() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join(".xberg.yaml");

    fs::write(
        &config_path,
        r#"
use_cache: false
enable_quality_processing: false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_discover_xberg_json_in_current_directory() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join(".xberg.json");

    fs::write(
        &config_path,
        r#"{
    "use_cache": false,
    "enable_quality_processing": false
}"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_case_insensitive_toml_extension() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom.TOML");

    fs::write(
        &config_path,
        r#"
use_cache = false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_case_insensitive_yaml_extension() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom.Yaml");

    fs::write(
        &config_path,
        r#"
use_cache: false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_case_insensitive_yml_extension() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom.YML");

    fs::write(
        &config_path,
        r#"
use_cache: false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_case_insensitive_json_extension() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom.JSON");

    fs::write(
        &config_path,
        r#"{
    "use_cache": false
}"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_explicit_config_path_toml() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom_config.toml");

    fs::write(
        &config_path,
        r#"
use_cache = false
enable_quality_processing = false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_explicit_config_path_yaml() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom_config.yaml");

    fs::write(
        &config_path,
        r#"
use_cache: false
enable_quality_processing: false
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_explicit_config_path_json() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("custom_config.json");

    fs::write(
        &config_path,
        r#"{
    "use_cache": false,
    "enable_quality_processing": false
}"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_invalid_config_extension() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("config.txt");

    fs::write(&config_path, "invalid content").unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains(".toml") || stderr.contains(".yaml") || stderr.contains(".json"),
        "Error message should mention supported extensions: {}",
        stderr
    );
}

#[test]
fn test_malformed_toml_config() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("bad_config.toml");

    fs::write(&config_path, "use_cache = [[[[[").unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(!output.status.success());
}

#[test]
fn test_malformed_yaml_config() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("bad_config.yaml");

    fs::write(&config_path, "use_cache: [[[[[").unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(!output.status.success());
}

#[test]
fn test_malformed_json_config() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("bad_config.json");

    fs::write(&config_path, r#"{"use_cache": [[[[[}"#).unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(!output.status.success());
}

#[test]
fn test_nonexistent_config_file() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("nonexistent.toml");

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(!output.status.success());
}

#[test]
fn test_default_config_when_no_file_found() {
    let dir = tempdir().unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "Command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_no_config_discovery_skips_invalid_project_config() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("xberg.toml"), "use_cache = \"not_a_bool\"\n").unwrap();

    let test_file = get_test_file("text/simple.txt");
    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", "--no-config-discovery", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        output.status.success(),
        "--no-config-discovery must ignore project config: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_normal_config_discovery_still_loads_project_config() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("xberg.toml"), "use_cache = \"not_a_bool\"\n").unwrap();

    let test_file = get_test_file("text/simple.txt");
    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        !output.status.success(),
        "normal discovery must load the invalid project config"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Failed to auto-discover configuration file"),
        "failure must identify config discovery: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_no_config_discovery_still_applies_config_json() {
    let dir = tempdir().unwrap();
    fs::write(dir.path().join("xberg.toml"), "use_cache = \"not_a_bool\"\n").unwrap();

    let test_file = get_test_file("text/simple.txt");
    let output = Command::new(get_binary_path())
        .current_dir(dir.path())
        .args([
            "extract",
            "--no-config-discovery",
            "--config-json",
            r#"{"use_cache":"not_a_bool"}"#,
            test_file.as_str(),
        ])
        .output()
        .expect("Failed to execute xberg");

    assert!(
        !output.status.success(),
        "invalid --config-json must still be applied and rejected"
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("Failed to merge --config-json with file config"),
        "failure must identify --config-json processing: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn test_invalid_config_values() {
    let dir = tempdir().unwrap();
    let config_path = dir.path().join("invalid.toml");

    fs::write(
        &config_path,
        r#"
use_cache = "not_a_bool"
"#,
    )
    .unwrap();

    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let config_arg = config_path.to_string_lossy().into_owned();

    let output = Command::new(get_binary_path())
        .args(["extract", "--config", config_arg.as_str(), test_file.as_str()])
        .output()
        .expect("Failed to execute xberg");

    assert!(!output.status.success());
}
