#!/usr/bin/env python3
"""Unified Docker image test script for all variants (core, full, cli)."""

from __future__ import annotations

import argparse
import json
import os
import random
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path

BLUE = "\033[0;34m"
GREEN = "\033[0;32m"
RED = "\033[0;31m"
YELLOW = "\033[1;33m"
NC = "\033[0m"

REPO_ROOT = Path(__file__).resolve().parents[3]
TEST_DOCS_DIR = REPO_ROOT / "test_documents"
RESULTS_FILE = Path("/tmp/xberg-docker-test-results.json")


@dataclass
class TestRunner:
    image: str
    variant: str
    verbose: bool = False
    total: int = 0
    passed: int = 0
    failed: int = 0
    failed_names: list[str] = field(default_factory=list)
    containers: list[str] = field(default_factory=list)

    def log(self, level: str, color: str, msg: str) -> None:
        print(f"{color}[{level}]{NC} {msg}", flush=True)

    def info(self, msg: str) -> None:
        self.log("INFO", BLUE, msg)

    def ok(self, msg: str = "PASS") -> None:
        self.log("SUCCESS", GREEN, msg)

    def error(self, msg: str) -> None:
        self.log("ERROR", RED, msg)

    def warn(self, msg: str) -> None:
        self.log("WARNING", YELLOW, msg)

    def debug(self, msg: str) -> None:
        if self.verbose:
            self.log("VERBOSE", YELLOW, msg)

    def start(self, name: str) -> None:
        self.total += 1
        self.info(f"Test {self.total}: {name}")

    def pass_test(self) -> None:
        self.passed += 1
        self.ok()

    def fail_test(self, name: str, details: str = "") -> None:
        self.failed += 1
        self.failed_names.append(name)
        msg = f"FAIL: {name}"
        if details:
            msg += f"\n  Details: {details}"
        self.error(msg)

    def container_name(self) -> str:
        name = f"xberg-test-{int(time.time())}-{random.randint(0, 99999)}"
        self.containers.append(name)
        return name

    def docker_run(self, *args: str, capture: bool = True) -> subprocess.CompletedProcess[str]:
        cmd = ["docker", "run", "--rm", *args]
        return subprocess.run(cmd, check=False, capture_output=capture, text=True, timeout=120)

    def docker_run_detached(self, *args: str) -> str:
        name = self.container_name()
        cmd = ["docker", "run", "-d", "--name", name, *args]
        subprocess.run(cmd, capture_output=True, text=True, check=True, timeout=60)
        return name

    def docker_rm(self, name: str) -> None:
        subprocess.run(["docker", "rm", "-f", name], check=False, capture_output=True, timeout=30)

    def cleanup(self) -> None:
        for c in self.containers:
            self.docker_rm(c)

    def run_cli_output(self, *extra_args: str, volumes: bool = False) -> str:
        """Run a CLI command against the image and return combined stdout+stderr."""
        args: list[str] = ["--name", self.container_name()]
        if volumes:
            args += ["-v", f"{TEST_DOCS_DIR}:/data:ro"]
        args.append(self.image)
        args.extend(extra_args)
        r = self.docker_run(*args)
        return (r.stdout + r.stderr).strip()

    def write_results(self) -> None:
        rate = (self.passed * 100 // self.total) if self.total else 0
        data = {
            "image": self.image,
            "variant": self.variant,
            "total_tests": self.total,
            "passed": self.passed,
            "failed": self.failed,
            "success_rate": rate,
            "failed_tests": self.failed_names,
        }
        RESULTS_FILE.write_text(json.dumps(data, indent=2))
        self.info(f"Results written to {RESULTS_FILE}")


def test_image_exists(t: TestRunner) -> None:
    t.start("Docker image exists")
    r = subprocess.run(["docker", "inspect", t.image], check=False, capture_output=True, timeout=30)
    if r.returncode == 0:
        t.pass_test()
    else:
        t.fail_test("Image does not exist", t.image)


def test_version(t: TestRunner) -> None:
    t.start("CLI --version command")
    out = t.run_cli_output("--version")
    t.debug(f"Version output: {out}")
    if "xberg" in out.lower():
        t.pass_test()
    else:
        t.fail_test("CLI version", f"Expected 'xberg' in output, got: {out}")


def test_help(t: TestRunner) -> None:
    t.start("CLI --help command")
    out = t.run_cli_output("--help")
    if "extract" in out.lower():
        t.pass_test()
    else:
        t.fail_test("CLI help", "Expected 'extract' in help output")


def test_mime_detection(t: TestRunner) -> None:
    t.start("MIME type detection (detect command)")
    out = t.run_cli_output("detect", "/data/pdf/searchable.pdf", volumes=True)
    t.debug(f"MIME detection output: {out}")
    if "application/pdf" in out.lower():
        t.pass_test()
    else:
        t.fail_test("MIME detection", f"Expected 'application/pdf', got: {out}")


def test_extract_text(t: TestRunner) -> None:
    t.start("Extract plain text file")
    out = t.run_cli_output("extract", "/data/text/contract.txt", volumes=True)
    t.debug(f"Text extraction output (first 100 chars): {out[:100]}")
    if len(out) > 15 and "contract" in out.lower():
        t.pass_test()
    else:
        t.fail_test("Text extraction", f"Output too short ({len(out)} chars) or missing expected keywords")


def test_extract_pdf(t: TestRunner) -> None:
    t.start("Extract searchable PDF")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "-v",
            f"{TEST_DOCS_DIR}:/data:ro",
            t.image,
            "extract",
            "/data/pdf/searchable.pdf",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"PDF extraction output (first 200 chars): {out[:200]}")
    if r.returncode != 0:
        t.fail_test("Searchable PDF extraction", f"Exit code {r.returncode}: {out[:300]}")
    elif len(out) > 50:
        t.pass_test()
    else:
        t.fail_test("Searchable PDF extraction", f"Output too short: {len(out)} chars")


def test_extract_html(t: TestRunner) -> None:
    t.start("Extract HTML file")
    out = t.run_cli_output("extract", "/data/html/simple_table.html", volumes=True)
    t.debug(f"HTML extraction output (first 100 chars): {out[:100]}")
    if len(out) > 10:
        t.pass_test()
    else:
        t.fail_test("HTML extraction", f"Output too short: {len(out)} chars")


def test_extract_docx(t: TestRunner) -> None:
    t.start("Extract DOCX file")
    out = t.run_cli_output("extract", "/data/docx/extraction_test.docx", volumes=True)
    t.debug(f"DOCX extraction output (first 100 chars): {out[:100]}")
    if len(out) > 100:
        t.pass_test()
    else:
        t.fail_test("DOCX extraction", f"Output too short ({len(out)} chars)")


def test_batch_cli(t: TestRunner) -> None:
    t.start("CLI batch extraction (multiple files)")
    out = t.run_cli_output(
        "batch",
        "/data/text/contract.txt",
        "/data/html/simple_table.html",
        volumes=True,
    )
    t.debug(f"Batch output (first 200 chars): {out[:200]}")
    if len(out) > 20:
        t.pass_test()
    else:
        t.fail_test("Batch extraction", f"Output too short: {len(out)} chars")


def test_nonexistent_file(t: TestRunner) -> None:
    t.start("Non-existent file returns error")
    r = subprocess.run(
        ["docker", "run", "--rm", t.image, "extract", "/nonexistent/file.pdf"],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    if r.returncode != 0:
        t.pass_test()
    else:
        t.fail_test("Error on missing file", "Expected non-zero exit code for missing file")


def test_readonly_mount(t: TestRunner) -> None:
    t.start("Read-only volume mount works")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "-v",
            f"{TEST_DOCS_DIR}:/data:ro",
            "--read-only",
            "--tmpfs",
            "/tmp",
            t.image,
            "extract",
            "/data/text/simple.txt",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    out = (r.stdout + r.stderr).strip()
    if len(out) > 5:
        t.pass_test()
    else:
        t.fail_test("Read-only mount", "Failed to extract with read-only filesystem")


def _wait_for_api(port: int, retries: int = 10) -> bool:
    import urllib.request

    for _ in range(retries):
        try:
            urllib.request.urlopen(f"http://localhost:{port}/health", timeout=3)
            return True
        except Exception:
            time.sleep(2)
    return False


def _api_get(port: int, path: str) -> str | None:
    import urllib.request

    try:
        with urllib.request.urlopen(f"http://localhost:{port}{path}", timeout=10) as resp:
            return resp.read().decode()
    except Exception:
        return None


def _api_post_file(port: int, path: str, filepath: str) -> str | None:
    """POST a file using curl (simplest multipart approach)."""
    r = subprocess.run(
        ["curl", "-f", "-s", "-X", "POST", f"http://localhost:{port}{path}", "-F", f"files=@{filepath}"],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    return r.stdout if r.returncode == 0 else None


def _api_put_body(port: int, path: str, filepath: str, extra_headers: dict[str, str] | None = None) -> str | None:
    """PUT a raw binary body (OpenWebUI External engine) with an optional set of extra headers."""
    cmd = [
        "curl",
        "-f",
        "-s",
        "-X",
        "PUT",
        f"http://localhost:{port}{path}",
        "-H",
        "Content-Type: text/plain",
        "-H",
        "X-Filename: openweb.txt",
        "--data-binary",
        f"@{filepath}",
    ]
    for key, value in (extra_headers or {}).items():
        cmd += ["-H", f"{key}: {value}"]
    r = subprocess.run(cmd, check=False, capture_output=True, text=True, timeout=30)
    return r.stdout if r.returncode == 0 else None


def _api_post_multipart(port: int, path: str, filepath: str, fields: dict[str, str] | None = None) -> str | None:
    """POST a multipart form (OpenWebUI Docling engine): a `files` part plus optional text fields."""
    cmd = ["curl", "-f", "-s", "-X", "POST", f"http://localhost:{port}{path}", "-F", f"files=@{filepath}"]
    for key, value in (fields or {}).items():
        cmd += ["-F", f"{key}={value}"]
    r = subprocess.run(cmd, check=False, capture_output=True, text=True, timeout=30)
    return r.stdout if r.returncode == 0 else None


def test_ocr_extraction(t: TestRunner) -> None:
    t.start("OCR extraction with Tesseract")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "--memory",
            "1g",
            "-v",
            f"{TEST_DOCS_DIR}:/data:ro",
            t.image,
            "extract",
            "/data/images/ocr_image.jpg",
            "--ocr",
            "true",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"OCR extraction output (first 100 chars): {out[:100]}")
    if len(out) > 10:
        t.pass_test()
    else:
        t.fail_test("OCR extraction", "Output too short or OCR failed")


def test_paddle_ocr_extraction(t: TestRunner) -> None:
    t.start("PaddleOCR extraction (pre-loaded models)")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "--memory",
            "2g",
            "-v",
            f"{TEST_DOCS_DIR}:/data:ro",
            t.image,
            "extract",
            "/data/images/ocr_image.jpg",
            "--ocr",
            "true",
            "--ocr-backend",
            "paddle-ocr",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"PaddleOCR extraction output (first 200 chars): {out[:200]}")
    if r.returncode == 0 and len(out) > 10:
        t.pass_test()
    else:
        t.fail_test("PaddleOCR extraction", f"Exit code: {r.returncode}, output length: {len(out)}")


def test_doc_extraction(t: TestRunner) -> None:
    t.start("Legacy DOC extraction (native OLE/CFB)")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "--memory",
            "1g",
            "-v",
            f"{TEST_DOCS_DIR}:/data:ro",
            t.image,
            "extract",
            "/data/doc/unit_test_lists.doc",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"DOC extraction output (first 100 chars): {out[:100]}")
    if len(out) > 20:
        t.pass_test()
    else:
        t.fail_test("DOC extraction", f"Output too short: {len(out)} chars")


def test_api_health(t: TestRunner) -> None:
    t.start("API server startup and health check")
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached(
        "--memory",
        "2g",
        "--cpus",
        "2",
        "-p",
        f"{port}:8000",
        t.image,
    )
    if not _wait_for_api(port):
        t.fail_test("API health check", f"Health endpoint not responding on port {port}")
        t.docker_rm(name)
        return

    health = _api_get(port, "/health")
    t.debug(f"Health response: {health}")
    if health:
        t.pass_test()
    else:
        t.fail_test("API health check", "No response from /health")

    t.start("Plugin initialization validation")
    if health and "plugins" in health:
        import re

        ocr_m = re.search(r'"ocr_backends_count":(\d+)', health)
        ext_m = re.search(r'"extractors_count":(\d+)', health)
        ocr_count = int(ocr_m.group(1)) if ocr_m else 0
        ext_count = int(ext_m.group(1)) if ext_m else 0
        t.debug(f"OCR backends: {ocr_count}, Extractors: {ext_count}")

        if t.variant == "full":
            if ocr_count > 0:
                t.info(f"Full variant: {ocr_count} OCR backend(s) registered")
                t.pass_test()
            else:
                t.fail_test("Plugin initialization", "Full variant: No OCR backends registered")
                t.docker_rm(name)
                return
        else:
            t.pass_test()

        if ext_count == 0:
            t.fail_test("Plugin initialization", "No document extractors registered")
            t.docker_rm(name)
            return
    else:
        t.warn("Health response missing 'plugins' field")
        t.pass_test()

    t.docker_rm(name)


def test_api_extract(t: TestRunner) -> None:
    t.start("API extraction endpoint")
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached(
        "--memory",
        "2g",
        "--cpus",
        "2",
        "-p",
        f"{port}:8000",
        t.image,
    )
    if not _wait_for_api(port):
        t.fail_test("API extraction", "Server not ready")
        t.docker_rm(name)
        return

    with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as f:
        f.write("Test content for API extraction")
        tmp = f.name

    resp = _api_post_file(port, "/extract", tmp)
    os.unlink(tmp)
    t.debug(f"API response: {resp}")

    if resp and "Test content for API extraction" in resp:
        t.pass_test()
    else:
        t.fail_test("API extraction", "Response missing expected content")
    t.docker_rm(name)


def test_api_info(t: TestRunner) -> None:
    t.start("API /info endpoint")
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached(
        "--memory",
        "2g",
        "--cpus",
        "2",
        "-p",
        f"{port}:8000",
        t.image,
    )
    if not _wait_for_api(port):
        t.fail_test("API /info", "Server not ready")
        t.docker_rm(name)
        return

    resp = _api_get(port, "/info")
    t.debug(f"/info response: {resp}")
    if resp and "version" in resp and "rust_backend" in resp:
        t.pass_test()
    else:
        t.fail_test("API /info endpoint", "Response missing expected fields")
    t.docker_rm(name)


def test_api_openapi(t: TestRunner) -> None:
    t.start("API /openapi.json endpoint")
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached(
        "--memory",
        "2g",
        "--cpus",
        "2",
        "-p",
        f"{port}:8000",
        t.image,
    )
    if not _wait_for_api(port):
        t.fail_test("API /openapi.json", "Server not ready")
        t.docker_rm(name)
        return

    resp = _api_get(port, "/openapi.json")
    t.debug(f"/openapi.json response (first 200 chars): {(resp or '')[:200]}")
    if resp and '"openapi"' in resp and '"paths"' in resp:
        t.pass_test()
    else:
        t.fail_test("API /openapi.json endpoint", "Response missing OpenAPI schema fields")
    t.docker_rm(name)


def test_api_cache(t: TestRunner) -> None:
    t.start("API /cache/stats endpoint")
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached(
        "--memory",
        "2g",
        "--cpus",
        "2",
        "-p",
        f"{port}:8000",
        t.image,
    )
    if not _wait_for_api(port):
        t.fail_test("API /cache/stats", "Server not ready")
        t.docker_rm(name)
        return

    resp = _api_get(port, "/cache/stats")
    t.debug(f"/cache/stats response: {resp}")
    if resp and "total_files" in resp:
        t.pass_test()
    else:
        t.fail_test("API /cache/stats endpoint", "Response missing expected fields")

    t.start("API /cache/clear endpoint")
    r = subprocess.run(
        ["curl", "-f", "-s", "-X", "DELETE", f"http://localhost:{port}/cache/clear"],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    if r.returncode == 0 and "removed_files" in r.stdout:
        t.pass_test()
    else:
        t.fail_test("API /cache/clear endpoint", "Response missing expected fields")
    t.docker_rm(name)


def test_api_batch(t: TestRunner) -> None:
    t.start("API batch extraction (multiple files)")
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached(
        "--memory",
        "2g",
        "--cpus",
        "2",
        "-p",
        f"{port}:8000",
        t.image,
    )
    if not _wait_for_api(port):
        t.fail_test("API batch extraction", "Server not ready")
        t.docker_rm(name)
        return

    tmp1 = tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False)
    tmp2 = tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False)
    tmp1.write("File one content")
    tmp1.close()
    tmp2.write("File two content")
    tmp2.close()

    r = subprocess.run(
        [
            "curl",
            "-f",
            "-s",
            "-X",
            "POST",
            f"http://localhost:{port}/extract",
            "-F",
            f"files=@{tmp1.name}",
            "-F",
            f"files=@{tmp2.name}",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    os.unlink(tmp1.name)
    os.unlink(tmp2.name)

    t.debug(f"Batch extraction response (first 200 chars): {r.stdout[:200]}")
    if "File one content" in r.stdout and "File two content" in r.stdout:
        t.pass_test()
    else:
        t.fail_test("API batch extraction", "Response missing expected content")
    t.docker_rm(name)


def test_api_openweb(t: TestRunner) -> None:
    """OpenWebUI-compat endpoints: External `PUT /process` and Docling `POST /v1/convert/file`.

    Also asserts a per-request config actually reaches the pipeline (regression guard for the
    OpenWebUI "parameters ignored" bug): the same document rendered with `output_format=json`
    must differ from the default Markdown rendering.
    """
    port = 9000 + random.randint(0, 999)
    name = t.docker_run_detached("--memory", "2g", "--cpus", "2", "-p", f"{port}:8000", t.image)
    if not _wait_for_api(port):
        t.start("OpenWebUI endpoints")
        t.fail_test("OpenWebUI endpoints", "Server not ready")
        t.docker_rm(name)
        return

    with tempfile.NamedTemporaryFile(mode="w", suffix=".txt", delete=False) as f:
        f.write("OpenWebUI compat content")
        tmp = f.name

    try:
        t.start("OpenWebUI External /process returns page_content + source")
        proc = _api_put_body(port, "/process", tmp)
        t.debug(f"/process response: {proc}")
        if proc and "OpenWebUI compat content" in proc and '"page_content"' in proc and '"source"' in proc:
            t.pass_test()
        else:
            t.fail_test("OpenWebUI /process", f"Missing expected fields: {proc}")

        t.start("OpenWebUI External /process honors X-Config")
        proc_json = _api_put_body(port, "/process", tmp, {"X-Config": '{"output_format":"json"}'})
        t.debug(f"/process X-Config response: {proc_json}")
        if proc and proc_json and proc_json != proc:
            t.pass_test()
        else:
            t.fail_test("OpenWebUI /process X-Config", "Config did not change output (params ignored)")

        t.start("OpenWebUI Docling /v1/convert/file returns md_content + status")
        docling = _api_post_multipart(port, "/v1/convert/file", tmp)
        t.debug(f"/v1/convert/file response: {docling}")
        if docling and '"md_content"' in docling and '"status"' in docling and "OpenWebUI compat content" in docling:
            t.pass_test()
        else:
            t.fail_test("OpenWebUI /v1/convert/file", f"Missing expected fields: {docling}")

        t.start("OpenWebUI Docling /v1/convert/file honors config field")
        docling_json = _api_post_multipart(port, "/v1/convert/file", tmp, {"config": '{"output_format":"json"}'})
        t.debug(f"/v1/convert/file config response: {docling_json}")
        if docling and docling_json and docling_json != docling:
            t.pass_test()
        else:
            t.fail_test("OpenWebUI /v1/convert/file config", "Config did not change output (params ignored)")
    finally:
        os.unlink(tmp)
        t.docker_rm(name)


def test_cli_batch_json(t: TestRunner) -> None:
    t.start("CLI batch extraction with JSON format")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "-v",
            f"{TEST_DOCS_DIR}:/data:ro",
            t.image,
            "batch",
            "/data/text/contract.txt",
            "/data/pdf/searchable.pdf",
            "--format",
            "json",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=120,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"Batch command output (first 200 chars): {out[:200]}")
    if len(out) > 100 and "content" in out:
        t.pass_test()
    else:
        t.fail_test("CLI batch command", "Output too short or malformed")


def test_mcp_server(t: TestRunner) -> None:
    t.start("MCP server startup and persistence")
    name = t.docker_run_detached(
        "-i",
        "--memory",
        "1g",
        t.image,
        "mcp",
    )
    time.sleep(3)
    r = subprocess.run(
        ["docker", "ps", "--filter", f"name={name}", "--format", "{{.Names}}"],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    if name in r.stdout:
        t.debug("MCP server is running")
        t.pass_test()
    else:
        t.fail_test("MCP server persistence", "MCP server exited immediately")
    t.docker_rm(name)


def test_cli_cache(t: TestRunner) -> None:
    t.start("CLI cache stats command")
    name = t.container_name()
    r = subprocess.run(
        ["docker", "run", "--rm", "--name", name, t.image, "cache", "stats", "--format", "json"],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"Cache stats output: {out}")
    if "total_files" in out:
        t.pass_test()
    else:
        t.fail_test("CLI cache stats", "Output missing expected fields")

    t.start("CLI cache clear command")
    name = t.container_name()
    r = subprocess.run(
        ["docker", "run", "--rm", "--name", name, t.image, "cache", "clear", "--format", "json"],
        check=False,
        capture_output=True,
        text=True,
        timeout=60,
    )
    out = (r.stdout + r.stderr).strip()
    t.debug(f"Cache clear output: {out}")
    if "removed_files" in out:
        t.pass_test()
    else:
        t.fail_test("CLI cache clear", "Output missing expected fields")


def test_security_nonroot(t: TestRunner) -> None:
    t.start("Security: Container runs as non-root user")
    name = t.container_name()
    r = subprocess.run(
        ["docker", "run", "--rm", "--name", name, "--entrypoint", "/bin/sh", t.image, "-c", "whoami"],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    user = r.stdout.strip()
    if user == "xberg":
        t.pass_test()
    else:
        t.fail_test("Non-root user", f"Container running as: {user} (expected: xberg)")


def test_security_readonly(t: TestRunner) -> None:
    t.start("Security: Read-only volume enforcement")
    with tempfile.TemporaryDirectory() as tmpdir:
        (Path(tmpdir) / "test.txt").write_text("test")
        name = t.container_name()
        r = subprocess.run(
            [
                "docker",
                "run",
                "--rm",
                "--name",
                name,
                "-v",
                f"{tmpdir}:/data:ro",
                "--entrypoint",
                "/bin/sh",
                t.image,
                "-c",
                "echo 'attempt' > /data/test2.txt 2>&1 || echo 'READ_ONLY'",
            ],
            check=False,
            capture_output=True,
            text=True,
            timeout=30,
        )
        out = r.stdout + r.stderr
        if any(s in out for s in ("READ_ONLY", "read-only", "Read-only")):
            t.pass_test()
        else:
            t.fail_test("Read-only volume", "Was able to write to read-only volume")


def test_security_memlimit(t: TestRunner) -> None:
    t.start("Security: Memory limit enforcement")
    name = t.container_name()
    r = subprocess.run(
        [
            "docker",
            "run",
            "--rm",
            "--name",
            name,
            "--memory",
            "128m",
            "--memory-swap",
            "128m",
            "--entrypoint",
            "/bin/sh",
            t.image,
            "-c",
            "echo 'Memory limit test passed'",
        ],
        check=False,
        capture_output=True,
        text=True,
        timeout=30,
    )
    if "Memory limit test passed" in r.stdout:
        t.pass_test()
    else:
        t.fail_test("Memory limit", "Container failed with memory limit")


def test_cli_image_size(t: TestRunner) -> None:
    t.start("Image size is reasonable (< 200MB)")
    r = subprocess.run(
        ["docker", "inspect", t.image, "--format", "{{.Size}}"],
        check=False,
        capture_output=True,
        text=True,
        timeout=10,
    )
    try:
        size_mb = int(r.stdout.strip()) // (1024 * 1024)
    except ValueError:
        size_mb = 0
    t.debug(f"Image size: {size_mb}MB")
    if 0 < size_mb < 200:
        t.pass_test()
    else:
        t.fail_test("Image size", f"Expected < 200MB, got {size_mb}MB")


def run_cli_tests(t: TestRunner) -> None:
    """Tests for the minimal CLI Docker image."""
    test_image_exists(t)
    test_cli_image_size(t)
    test_version(t)
    test_help(t)
    test_mime_detection(t)
    test_extract_text(t)
    test_extract_pdf(t)
    test_extract_html(t)
    test_extract_docx(t)
    test_batch_cli(t)
    test_readonly_mount(t)
    test_nonexistent_file(t)


def run_core_full_tests(t: TestRunner) -> None:
    """Tests for core and full Docker images."""
    test_image_exists(t)
    test_version(t)
    test_help(t)
    test_mime_detection(t)
    test_extract_text(t)
    test_extract_pdf(t)
    test_extract_docx(t)
    test_extract_html(t)
    test_ocr_extraction(t)

    if t.variant == "full":
        test_doc_extraction(t)
        test_paddle_ocr_extraction(t)

    test_api_health(t)
    test_api_extract(t)
    test_api_info(t)
    test_api_openapi(t)
    test_api_cache(t)
    test_api_batch(t)
    test_api_openweb(t)
    test_cli_batch_json(t)
    test_mcp_server(t)
    test_cli_cache(t)
    test_security_nonroot(t)
    test_security_readonly(t)
    test_security_memlimit(t)


def main() -> None:
    parser = argparse.ArgumentParser(description="Docker image tests")
    parser.add_argument("--image", required=True, help="Docker image name")
    parser.add_argument("--variant", required=True, choices=["core", "full", "cli"])
    parser.add_argument("--verbose", action="store_true")
    parser.add_argument("--skip-build", action="store_true", help="(ignored, kept for compat)")
    args = parser.parse_args()

    t = TestRunner(image=args.image, variant=args.variant, verbose=args.verbose)

    print("=" * 72)
    t.info(f"Starting Docker tests for: {args.image} (variant: {args.variant})")
    print("=" * 72)

    try:
        if args.variant == "cli":
            run_cli_tests(t)
        else:
            run_core_full_tests(t)
    finally:
        t.cleanup()

    print()
    print("=" * 72)
    t.info(f"Test Results: {t.passed}/{t.total} passed, {t.failed} failed")
    print("=" * 72)

    if t.failed > 0:
        t.error("Failed tests:")
        for name in t.failed_names:
            print(f"  - {name}")

    t.write_results()

    if t.failed > 0:
        sys.exit(1)
    t.ok("All tests passed!")


if __name__ == "__main__":
    main()
