//! Integration tests for CLI commands (extract, detect, batch).
//!
//! These tests verify that the CLI commands work correctly end-to-end,
//! including input validation, file processing, and output formatting.

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
fn test_extract_text_file() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["extract", test_file.as_str()])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        output.status.success(),
        "Extract command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Extract output should not be empty");
}

#[test]
fn test_extract_with_json_output() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["extract", test_file.as_str(), "--format", "json"])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        output.status.success(),
        "Extract command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    let json_result: serde_json::Result<serde_json::Value> = serde_json::from_str(&stdout);
    assert!(json_result.is_ok(), "Output should be valid JSON, got: {}", stdout);

    let json = json_result.unwrap();
    assert!(json.get("result").is_some(), "JSON envelope should have 'result' field");
    assert!(
        json.get("extraction_time_ms").is_some(),
        "JSON envelope should have 'extraction_time_ms' field"
    );
    assert!(
        json["result"].get("content").is_some(),
        "result should have 'content' field"
    );
    assert!(
        json["result"].get("mime_type").is_some(),
        "result should have 'mime_type' field"
    );
}

#[test]
fn test_extract_with_chunking() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args([
            "extract",
            test_file.as_str(),
            "--chunk",
            "true",
            "--chunk-size",
            "100",
            "--chunk-overlap",
            "20",
            "--format",
            "json",
        ])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        output.status.success(),
        "Extract with chunking failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value = serde_json::from_str(&stdout).expect("Should be valid JSON");

    assert!(
        json["result"].get("chunks").is_some(),
        "result should have 'chunks' field"
    );
    assert!(json["result"]["chunks"].is_array(), "'chunks' should be an array");
}

#[test]
fn test_extract_file_not_found() {
    let output = Command::new(get_binary_path())
        .args(["extract", "/nonexistent/file.txt"])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Extract should fail for nonexistent file");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("File not found"),
        "Error should mention file not found, got: {}",
        stderr
    );
}

#[test]
fn test_extract_directory_not_file() {
    let tmp_dir = tempdir().expect("Failed to create temp dir");
    let dir_path = tmp_dir.path().to_string_lossy().to_string();

    let output = Command::new(get_binary_path())
        .args(["extract", dir_path.as_str()])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Extract should fail for directory");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("not a file") || stderr.contains("regular file"),
        "Error should mention path is not a file, got: {}",
        stderr
    );
}

#[test]
fn test_extract_invalid_chunk_size_zero() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["extract", test_file.as_str(), "--chunk-size", "0"])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Extract should fail for chunk size 0");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid chunk size") || stderr.contains("must be greater than 0"),
        "Error should mention invalid chunk size, got: {}",
        stderr
    );
}

#[test]
fn test_extract_invalid_chunk_size_too_large() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["extract", test_file.as_str(), "--chunk-size", "2000000"])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Extract should fail for chunk size > 1M");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid chunk size") || stderr.contains("1,000,000"),
        "Error should mention chunk size limit, got: {}",
        stderr
    );
}

#[test]
fn test_extract_invalid_overlap_equals_chunk_size() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args([
            "extract",
            test_file.as_str(),
            "--chunk-size",
            "100",
            "--chunk-overlap",
            "100",
        ])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        !output.status.success(),
        "Extract should fail when overlap equals chunk size"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Invalid chunk overlap") || stderr.contains("must be less than chunk size"),
        "Error should mention overlap constraint, got: {}",
        stderr
    );
}

#[test]
fn test_detect_mime_type() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["detect", test_file.as_str()])
        .output()
        .expect("Failed to execute detect command");

    assert!(
        output.status.success(),
        "Detect command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Detect output should not be empty");
    assert!(
        stdout.contains("text/plain") || stdout.contains("text"),
        "Should detect text MIME type, got: {}",
        stdout
    );
}

#[test]
fn test_detect_with_json_output() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        tracing::debug!("Skipping test: {} not found", test_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["detect", test_file.as_str(), "--format", "json"])
        .output()
        .expect("Failed to execute detect command");

    assert!(
        output.status.success(),
        "Detect command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    let json_result: serde_json::Result<serde_json::Value> = serde_json::from_str(&stdout);
    assert!(json_result.is_ok(), "Output should be valid JSON, got: {}", stdout);

    let json = json_result.unwrap();
    assert!(json.get("mime_type").is_some(), "JSON should have 'mime_type' field");
    assert!(json.get("path").is_some(), "JSON should have 'path' field");
}

#[test]
fn test_detect_file_not_found() {
    let output = Command::new(get_binary_path())
        .args(["detect", "/nonexistent/file.txt"])
        .output()
        .expect("Failed to execute detect command");

    assert!(!output.status.success(), "Detect should fail for nonexistent file");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("File not found"),
        "Error should mention file not found, got: {}",
        stderr
    );
}

#[test]
fn test_batch_multiple_files() {
    let file1 = get_test_file("text/simple.txt");
    let file2 = get_test_file("text/simple.txt");

    if !PathBuf::from(&file1).exists() {
        tracing::debug!("Skipping test: {} not found", file1);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["batch", file1.as_str(), file2.as_str(), "--format", "json"])
        .output()
        .expect("Failed to execute batch command");

    assert!(
        output.status.success(),
        "Batch command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);

    let json_result: serde_json::Result<serde_json::Value> = serde_json::from_str(&stdout);
    assert!(json_result.is_ok(), "Output should be valid JSON, got: {}", stdout);

    let json = json_result.unwrap();
    assert!(
        json.get("results").is_some(),
        "Batch envelope should have 'results' field"
    );
    assert!(json["results"].is_array(), "'results' should be a JSON array");
    assert_eq!(json["results"].as_array().unwrap().len(), 2, "Should have 2 results");
}

#[test]
fn test_batch_with_missing_file() {
    let valid_file = get_test_file("text/simple.txt");

    if !PathBuf::from(&valid_file).exists() {
        tracing::debug!("Skipping test: {} not found", valid_file);
        return;
    }

    let output = Command::new(get_binary_path())
        .args(["batch", valid_file.as_str(), "/nonexistent/file.txt"])
        .output()
        .expect("Failed to execute batch command");

    assert!(!output.status.success(), "Batch should fail when one file is missing");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("File not found") || stderr.contains("Invalid file"),
        "Error should mention file not found, got: {}",
        stderr
    );
}

#[test]
fn test_extract_help() {
    let output = Command::new(get_binary_path())
        .args(["extract", "--help"])
        .output()
        .expect("Failed to execute extract --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Extract text from a document"));
    assert!(stdout.contains("--chunk-size"));
    assert!(stdout.contains("--chunk-overlap"));
}

#[test]
fn test_detect_help() {
    let output = Command::new(get_binary_path())
        .args(["detect", "--help"])
        .output()
        .expect("Failed to execute detect --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Detect MIME type"));
}

#[test]
fn test_batch_help() {
    let output = Command::new(get_binary_path())
        .args(["batch", "--help"])
        .output()
        .expect("Failed to execute batch --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Batch extract from multiple documents"));
}

#[test]
fn test_extract_help_shows_all_extraction_override_flags() {
    let output = Command::new(get_binary_path())
        .args(["extract", "--help"])
        .output()
        .expect("Failed to execute extract --help");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);

    let expected_flags = [
        "--ocr",
        "--ocr-backend",
        "--ocr-language",
        "--force-ocr",
        "--no-cache",
        "--ocr-auto-rotate",
        "--chunk",
        "--chunk-size",
        "--chunk-overlap",
        "--chunking-tokenizer",
        "--content-format",
        "--include-structure",
        "--quality",
        "--detect-language",
        "--layout",
        "--layout-confidence",
        "--layout-table-model",
        "--acceleration",
        "--max-concurrent",
        "--max-threads",
        "--extract-pages",
        "--page-markers",
        "--extract-images",
        "--target-dpi",
        "--pdf-password",
        "--token-reduction",
        "--msg-codepage",
    ];

    for flag in &expected_flags {
        assert!(
            stdout.contains(flag),
            "Extract --help should show flag '{}', but it was not found in output:\n{}",
            flag,
            stdout
        );
    }
}

#[test]
fn test_batch_has_same_extraction_flags_as_extract() {
    let extract_output = Command::new(get_binary_path())
        .args(["extract", "--help"])
        .output()
        .expect("Failed to execute extract --help");

    let batch_output = Command::new(get_binary_path())
        .args(["batch", "--help"])
        .output()
        .expect("Failed to execute batch --help");

    assert!(extract_output.status.success());
    assert!(batch_output.status.success());

    let extract_help = String::from_utf8_lossy(&extract_output.stdout);
    let batch_help = String::from_utf8_lossy(&batch_output.stdout);

    let shared_flags = [
        "--ocr",
        "--ocr-backend",
        "--ocr-language",
        "--force-ocr",
        "--no-cache",
        "--chunk",
        "--chunk-size",
        "--chunk-overlap",
        "--content-format",
        "--quality",
        "--detect-language",
        "--layout",
        "--layout-confidence",
        "--layout-table-model",
        "--acceleration",
        "--max-concurrent",
        "--max-threads",
        "--extract-pages",
        "--page-markers",
        "--extract-images",
        "--target-dpi",
        "--pdf-password",
        "--token-reduction",
        "--msg-codepage",
    ];

    for flag in &shared_flags {
        assert!(
            extract_help.contains(flag),
            "Extract should have flag '{}' but it's missing",
            flag
        );
        assert!(
            batch_help.contains(flag),
            "Batch should have flag '{}' (parity with extract) but it's missing",
            flag
        );
    }
}

// NOTE: The CLI validates file existence *before* override validation,

/// Create a temporary file and return its path as a String.
/// The caller must keep the returned `tempfile::TempDir` alive for the
/// duration of the test so the file is not deleted.
fn create_temp_file() -> (tempfile::TempDir, String) {
    let dir = tempdir().expect("Failed to create temp dir");
    let file_path = dir.path().join("dummy.pdf");
    std::fs::write(&file_path, b"dummy content").expect("Failed to write temp file");
    let path_str = file_path.to_string_lossy().to_string();
    (dir, path_str)
}

#[test]
fn test_extract_chunk_size_zero_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--chunk-size", "0", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Should fail when chunk size is 0");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("chunk size") || stderr.contains("Chunk size") || stderr.contains("Invalid chunk size"),
        "Error should mention chunk size, got: {}",
        stderr
    );
}

#[test]
fn test_extract_chunk_overlap_exceeds_size_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--chunk-size", "10", "--chunk-overlap", "20", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Should fail when overlap exceeds chunk size");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("overlap") || stderr.contains("Overlap") || stderr.contains("Invalid chunk overlap"),
        "Error should mention overlap constraint, got: {}",
        stderr
    );
}

#[test]
fn test_extract_layout_confidence_out_of_range_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--layout-confidence", "2.0", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        !output.status.success(),
        "Should fail for layout confidence out of range"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("confidence") || stderr.contains("layout") || stderr.contains("unexpected argument"),
        "Error should mention confidence or layout, got: {}",
        stderr
    );
}

#[test]
fn test_extract_layout_false_with_confidence_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--layout", "false", "--layout-confidence", "0.5", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        !output.status.success(),
        "Should fail when --layout false is combined with --layout-confidence"
    );
}

#[test]
fn test_extract_target_dpi_zero_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--target-dpi", "0", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Should fail when target DPI is 0");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DPI") || stderr.contains("dpi") || stderr.contains("target") || stderr.contains("Invalid"),
        "Error should mention DPI range, got: {}",
        stderr
    );
}

#[test]
fn test_completions_bash_produces_output() {
    let output = Command::new(get_binary_path())
        .args(["completions", "bash"])
        .output()
        .expect("Failed to execute completions command");

    assert!(
        output.status.success(),
        "Completions command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Completions output should not be empty");
    assert!(
        stdout.contains("xberg"),
        "Bash completions should reference 'xberg', got: {}",
        &stdout[..stdout.len().min(200)]
    );
}

#[test]
fn test_completions_zsh_produces_output() {
    let output = Command::new(get_binary_path())
        .args(["completions", "zsh"])
        .output()
        .expect("Failed to execute completions command");

    assert!(
        output.status.success(),
        "Completions command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Zsh completions output should not be empty");
}

#[test]
fn test_completions_fish_produces_output() {
    let output = Command::new(get_binary_path())
        .args(["completions", "fish"])
        .output()
        .expect("Failed to execute completions command");

    assert!(
        output.status.success(),
        "Completions command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "Fish completions output should not be empty");
}

#[test]
fn test_embed_help_shows_correct_flags() {
    let output = Command::new(get_binary_path())
        .args(["embed", "--help"])
        .output()
        .expect("Failed to execute embed --help");

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.contains("unrecognized subcommand") || stderr.contains("invalid subcommand") {
            return;
        }
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--text"),
        "Embed help should show --text flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--preset"),
        "Embed help should show --preset flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--format"),
        "Embed help should show --format flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("Generate embeddings"),
        "Embed help should describe embedding generation, got: {}",
        stdout
    );
}

#[test]
fn test_chunk_help_shows_correct_flags() {
    let output = Command::new(get_binary_path())
        .args(["chunk", "--help"])
        .output()
        .expect("Failed to execute chunk --help");

    assert!(
        output.status.success(),
        "Chunk --help failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("--text"),
        "Chunk help should show --text flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--chunk-size"),
        "Chunk help should show --chunk-size flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--chunk-overlap"),
        "Chunk help should show --chunk-overlap flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--chunker-type"),
        "Chunk help should show --chunker-type flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("--format"),
        "Chunk help should show --format flag, got: {}",
        stdout
    );
    assert!(
        stdout.contains("Chunk text"),
        "Chunk help should describe text chunking, got: {}",
        stdout
    );
}

#[test]
fn test_no_color_env_disables_ansi_in_output() {
    let test_file = get_test_file("text/simple.txt");
    if !PathBuf::from(&test_file).exists() {
        return;
    }

    let output = Command::new(get_binary_path())
        .env("NO_COLOR", "1")
        .args(["detect", &test_file])
        .output()
        .expect("Failed to execute detect command");

    assert!(
        output.status.success(),
        "Detect failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !stdout.contains("\x1b["),
        "Output should not contain ANSI escape sequences when NO_COLOR is set, got: {:?}",
        stdout
    );
}

#[test]
fn test_extract_chunk_size_too_large_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--chunk-size", "2000000", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Should fail when chunk size exceeds limit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("chunk size") || stderr.contains("Chunk size") || stderr.contains("1,000,000"),
        "Error should mention chunk size limit, got: {}",
        stderr
    );
}

#[test]
#[ignore = "requires test_documents/docx/word_image_anchors.docx fixture"]
fn test_extract_images_written_to_output_dir() {
    let test_file = get_test_file("docx/word_image_anchors.docx");
    assert!(
        PathBuf::from(&test_file).exists(),
        "Test fixture not found: {}",
        test_file
    );

    let output_dir = tempdir().expect("Failed to create temp dir");

    let output = Command::new(get_binary_path())
        .args([
            "extract",
            &test_file,
            "--extract-images",
            "true",
            "--content-format",
            "markdown",
            "--output-dir",
            output_dir.path().to_str().unwrap(),
        ])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        output.status.success(),
        "Extract command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("image_0."),
        "Expected image reference in markdown, got: {}",
        stdout
    );

    let image_files: Vec<_> = std::fs::read_dir(output_dir.path())
        .expect("Failed to read output dir")
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name();
            let s = name.to_string_lossy();
            s.starts_with("image_")
                && (s.ends_with(".png") || s.ends_with(".jpeg") || s.ends_with(".jpg") || s.ends_with(".webp"))
        })
        .collect();

    assert!(
        !image_files.is_empty(),
        "Expected image files in output dir, but none were written. Markdown was:\n{}",
        stdout
    );

    for entry in &image_files {
        let meta = entry.metadata().expect("Failed to stat image file");
        assert!(
            meta.len() > 0,
            "Image file '{}' is empty",
            entry.file_name().to_string_lossy()
        );
    }
}

#[test]
#[ignore = "requires test_documents/docx/word_image_anchors.docx fixture"]
fn test_extract_images_default_to_cwd_when_no_output_dir() {
    let test_file = get_test_file("docx/word_image_anchors.docx");
    assert!(
        PathBuf::from(&test_file).exists(),
        "Test fixture not found: {}",
        test_file
    );

    let work_dir = tempdir().expect("Failed to create temp dir");

    let output = Command::new(get_binary_path())
        .args([
            "extract",
            &test_file,
            "--extract-images",
            "true",
            "--content-format",
            "markdown",
        ])
        .current_dir(work_dir.path())
        .output()
        .expect("Failed to execute extract command");

    assert!(
        output.status.success(),
        "Extract command failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("image_0."), "Expected image reference in markdown");

    let image_files: Vec<_> = std::fs::read_dir(work_dir.path())
        .expect("Failed to read work dir")
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().starts_with("image_"))
        .collect();

    assert!(
        !image_files.is_empty(),
        "Expected image files in cwd, but none were written"
    );
}

#[test]
fn test_extract_target_dpi_too_high_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args(["extract", "--target-dpi", "5000", &file_path])
        .output()
        .expect("Failed to execute extract command");

    assert!(!output.status.success(), "Should fail when target DPI exceeds limit");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("DPI") || stderr.contains("dpi") || stderr.contains("2400") || stderr.contains("Invalid"),
        "Error should mention DPI range, got: {}",
        stderr
    );
}

#[test]
fn test_extract_output_dir_nonexistent_error() {
    let (_dir, file_path) = create_temp_file();

    let output = Command::new(get_binary_path())
        .args([
            "extract",
            &file_path,
            "--output-dir",
            "/nonexistent/directory/that/does/not/exist",
        ])
        .output()
        .expect("Failed to execute extract command");

    assert!(
        !output.status.success(),
        "Extract should fail when --output-dir does not exist"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Output directory not found") || stderr.contains("not found") || stderr.contains("directory"),
        "Error should mention directory not found, got: {}",
        stderr
    );
}
