# Copyright (c) 2026 Xberg. All rights reserved.
"""MinerU 3.4.4 extraction wrapper for the benchmark harness.

Supports three modes:
- sync: process one file through the pipeline
- batch: process multiple files through the native multi-document pipeline
- server: persistent mode reading paths from stdin

The batch path intentionally calls MinerU's public ``do_parse`` entry point once.
In MinerU 3.4.4, that delegates to ``doc_analyze_streaming``, whose processing
windows batch model inference across document boundaries.
"""

from __future__ import annotations

import os

os.environ.setdefault("CUDA_VISIBLE_DEVICES", "")
os.environ.setdefault("ONNXRUNTIME_PROVIDERS", "CPUExecutionProvider")
os.environ.setdefault("MINERU_DEVICE_MODE", "cpu")

import json
import multiprocessing as _mp
import platform
import resource
import sys
import tempfile
import time
from importlib import metadata as importlib_metadata
from pathlib import Path
from typing import Any

PINNED_MINERU_VERSION = "3.4.4"
DEFAULT_OCR_LANGUAGE = "ch"
PIPELINE_BACKEND = "pipeline"


def _load_mineru_api():
    """Load the API whose call contract is pinned by the benchmark environment."""
    installed_version = importlib_metadata.version("mineru")
    if installed_version != PINNED_MINERU_VERSION:
        raise RuntimeError(
            f"MinerU {PINNED_MINERU_VERSION} is required by the benchmark harness; found {installed_version}"
        )

    from mineru.cli.common import do_parse, read_fn

    return do_parse, read_fn


def _get_peak_memory_bytes() -> int:
    """Get peak memory usage in bytes using resource module."""
    usage = resource.getrusage(resource.RUSAGE_SELF)
    if platform.system() == "Linux":
        return usage.ru_maxrss * 1024
    return usage.ru_maxrss


def _native_pipeline_markdown(file_paths: list[str], ocr_enabled: bool) -> tuple[list[str], float]:
    """Run one MinerU 3.4.4 multi-document pipeline invocation."""
    if not file_paths:
        return [], 0.0

    do_parse, read_fn = _load_mineru_api()
    task_stems = [f"benchmark_document_{index:08d}" for index in range(len(file_paths))]
    document_bytes = [read_fn(Path(file_path)) for file_path in file_paths]
    parse_method = "ocr" if ocr_enabled else "txt"

    with tempfile.TemporaryDirectory() as output_dir:
        output_root = Path(output_dir)
        expected_markdown_paths = [Path(task_stem) / parse_method / f"{task_stem}.md" for task_stem in task_stems]
        start = time.perf_counter()
        do_parse(
            output_dir=output_dir,
            pdf_file_names=list(task_stems),
            pdf_bytes_list=list(document_bytes),
            p_lang_list=[DEFAULT_OCR_LANGUAGE] * len(file_paths),
            backend=PIPELINE_BACKEND,
            parse_method=parse_method,
            f_draw_layout_bbox=False,
            f_draw_span_bbox=False,
            f_dump_md=True,
            f_dump_middle_json=False,
            f_dump_model_output=False,
            f_dump_orig_pdf=False,
            f_dump_content_list=False,
        )

        produced_markdown_paths = sorted(path.relative_to(output_root) for path in output_root.rglob("*.md"))
        expected_markdown_paths.sort()
        if produced_markdown_paths != expected_markdown_paths:
            missing = sorted(set(expected_markdown_paths) - set(produced_markdown_paths))
            unexpected = sorted(set(produced_markdown_paths) - set(expected_markdown_paths))
            raise RuntimeError(
                "MinerU native batch output cardinality mismatch: "
                f"expected {len(expected_markdown_paths)} Markdown outputs, "
                f"found {len(produced_markdown_paths)}; "
                f"missing={[path.as_posix() for path in missing]}; "
                f"unexpected={[path.as_posix() for path in unexpected]}"
            )

        markdown_outputs = [
            (output_root / markdown_path).read_text(encoding="utf-8") for markdown_path in expected_markdown_paths
        ]
        total_duration_ms = (time.perf_counter() - start) * 1000.0

    return markdown_outputs, total_duration_ms


_MD_STRIP_RE = None


def _strip_markdown(text: str) -> str:
    """Best-effort markdown→plaintext pass. Drops syntax tokens; preserves text."""
    import re

    global _MD_STRIP_RE
    if _MD_STRIP_RE is None:
        _MD_STRIP_RE = [
            (re.compile(r"^#{1,6}\s+", re.MULTILINE), ""),
            (re.compile(r"^\s*[-*+]\s+", re.MULTILINE), ""),
            (re.compile(r"^\s*\d+\.\s+", re.MULTILINE), ""),
            (re.compile(r"^>\s?", re.MULTILINE), ""),
            (re.compile(r"```[a-zA-Z0-9_-]*\n?"), ""),
            (re.compile(r"`([^`]+)`"), r"\1"),
            (re.compile(r"\*\*([^*]+)\*\*"), r"\1"),
            (re.compile(r"\*([^*]+)\*"), r"\1"),
            (re.compile(r"!\[([^\]]*)\]\([^)]*\)"), r"\1"),
            (re.compile(r"\[([^\]]+)\]\([^)]*\)"), r"\1"),
            (re.compile(r"^\s*\|.*\|\s*$", re.MULTILINE), ""),
        ]
    out = text
    for pattern, repl in _MD_STRIP_RE:
        out = pattern.sub(repl, out)
    return out


def extract_sync(file_path: str, ocr_enabled: bool, output_format: str = "markdown") -> dict[str, Any]:
    """Extract one file using the pinned MinerU pipeline."""
    markdown_outputs, duration_ms = _native_pipeline_markdown([file_path], ocr_enabled)
    markdown = markdown_outputs[0]
    content = _strip_markdown(markdown) if output_format == "plaintext" else markdown

    return {
        "content": content,
        "metadata": {
            "framework": "mineru",
            "output_format": output_format,
            "batch_api": "mineru.cli.common.do_parse",
        },
        "_extraction_time_ms": duration_ms,
        "_peak_memory_bytes": _get_peak_memory_bytes(),
    }


def extract_batch(file_paths: list[str], ocr_enabled: bool, output_format: str = "markdown") -> dict[str, Any]:
    """Extract a strict ordered batch using MinerU's multi-document pipeline."""
    markdown_outputs, total_duration_ms = _native_pipeline_markdown(file_paths, ocr_enabled)
    peak_memory = _get_peak_memory_bytes()
    results = [
        {
            "content": _strip_markdown(markdown) if output_format == "plaintext" else markdown,
            "metadata": {
                "framework": "mineru",
                "output_format": output_format,
                "batch_api": "mineru.cli.common.do_parse",
            },
            "_peak_memory_bytes": peak_memory,
        }
        for markdown in markdown_outputs
    ]
    return {
        "results": results,
        "total_ms": total_duration_ms,
        "per_file_ms": [None] * len(results),
        "metadata": {
            "framework": "mineru",
            "batch_api": "mineru.cli.common.do_parse",
            "model_batching": "cross_document_processing_windows_via_doc_analyze_streaming",
            "reported_total_timing_scope": "do_parse_and_ordered_markdown_materialization",
            "benchmark_timing_scope": "cold_end_to_end_subprocess",
            "per_item_timing": "unavailable",
        },
    }


def _worker(fn, args, conn):
    """Run extraction in a forked child process.

    Closes inherited stdin/stdout so the child cannot corrupt the
    parent's line-based JSON protocol.
    """
    try:
        sys.stdin.close()
        sys.stdout = open(os.devnull, "w")
    except Exception:
        pass
    try:
        result = fn(*args)
        conn.send(result)
    except Exception as e:
        conn.send({"error": str(e), "_extraction_time_ms": 0})
    finally:
        conn.close()


def _run_with_timeout(fn, args, timeout):
    """Execute fn(*args) in a forked child with a timeout.

    On timeout the child is killed but the parent stays alive —
    no expensive process restart is needed.
    """
    try:
        ctx = _mp.get_context("fork")
        parent_conn, child_conn = ctx.Pipe(duplex=False)
        p = ctx.Process(target=_worker, args=(fn, args, child_conn))
        p.start()
        child_conn.close()

        if parent_conn.poll(timeout=timeout):
            try:
                result = parent_conn.recv()
            except Exception:
                result = {"error": "worker process crashed", "_extraction_time_ms": 0}
        else:
            p.kill()
            result = {
                "error": f"extraction timed out after {timeout}s",
                "_extraction_time_ms": timeout * 1000.0,
            }

        p.join(timeout=5)
        if p.is_alive():
            p.kill()
            p.join()
        parent_conn.close()
        return result
    except Exception:
        try:
            return fn(*args)
        except Exception as e:
            return {"error": str(e), "_extraction_time_ms": 0}


def _parse_path(line: str) -> str:
    """Parse a request line: JSON object with path field, or plain file path."""
    stripped = line.strip()
    if stripped.startswith("{"):
        try:
            return json.loads(stripped).get("path", "")
        except (json.JSONDecodeError, ValueError):
            pass
    return stripped


def run_server(ocr_enabled: bool, output_format: str, timeout=None) -> None:
    """Persistent server mode: read paths from stdin, write JSON to stdout."""
    print("READY", flush=True)
    for line in sys.stdin:
        file_path = _parse_path(line)
        if not file_path:
            continue
        if timeout is not None:
            result = _run_with_timeout(extract_sync, (file_path, ocr_enabled, output_format), timeout)
        else:
            try:
                result = extract_sync(file_path, ocr_enabled, output_format)
            except Exception as e:
                result = {"error": str(e), "_extraction_time_ms": 0}
        print(json.dumps(result), flush=True)


def main() -> None:
    ocr_enabled = False
    timeout = None
    output_format = "markdown"
    args = []
    for arg in sys.argv[1:]:
        if arg == "--ocr":
            ocr_enabled = True
        elif arg == "--no-ocr":
            ocr_enabled = False
        elif arg.startswith("--timeout="):
            timeout = int(arg.split("=", 1)[1])
        elif arg.startswith("--format="):
            output_format = arg.split("=", 1)[1]
        else:
            args.append(arg)

    if output_format not in ("markdown", "plaintext"):
        print(f"Error: --format must be 'markdown' or 'plaintext'; got '{output_format}'", file=sys.stderr)
        sys.exit(64)

    if len(args) < 1:
        print(
            "Usage: mineru_extract.py [--ocr|--no-ocr] [--timeout=SECS] [--format=markdown|plaintext] <mode> <file_path> [additional_files...]",
            file=sys.stderr,
        )
        print("Modes: sync, batch, server", file=sys.stderr)
        sys.exit(1)

    mode = args[0]
    file_paths = args[1:]

    try:
        if mode == "server":
            run_server(ocr_enabled, output_format, timeout=timeout)

        elif mode == "sync":
            if len(file_paths) != 1:
                print("Error: sync mode requires exactly one file", file=sys.stderr)
                sys.exit(1)
            if timeout is not None:
                payload = _run_with_timeout(extract_sync, (file_paths[0], ocr_enabled, output_format), timeout)
            else:
                payload = extract_sync(file_paths[0], ocr_enabled, output_format)
            print(json.dumps(payload), end="")

        elif mode == "batch":
            if len(file_paths) < 1:
                print("Error: batch mode requires at least one file", file=sys.stderr)
                sys.exit(1)

            payload = extract_batch(file_paths, ocr_enabled, output_format)
            print(json.dumps(payload), end="")

        else:
            print(f"Error: Unknown mode '{mode}'. Use sync, batch, or server", file=sys.stderr)
            sys.exit(1)

    except Exception as e:
        print(f"Error extracting with MinerU: {e}", file=sys.stderr)
        sys.exit(1)


if __name__ == "__main__":
    main()
