# Copyright (c) 2026 Xberg. All rights reserved.
"""Conformance tests for the pinned MinerU benchmark wrapper."""

from __future__ import annotations

import importlib.util
import tempfile
import types
import unittest
from pathlib import Path
from unittest import mock


def _load_wrapper() -> types.ModuleType:
    script = Path(__file__).parents[1] / "scripts" / "mineru_extract.py"
    spec = importlib.util.spec_from_file_location("mineru_extract_under_test", script)
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


class MineruBatchConformanceTest(unittest.TestCase):
    """Validate native batch behavior without importing the real MinerU package."""

    def test_native_batch_uses_one_do_parse_call_and_preserves_input_order(self) -> None:
        """Pass all inputs through one ordered native pipeline invocation."""
        wrapper = _load_wrapper()
        input_paths = ["/fixtures/second.pdf", "/fixtures/first.pdf"]
        observed: dict[str, object] = {}

        def read_fn(path: Path) -> bytes:
            return f"bytes:{path.name}".encode()

        def do_parse(**kwargs: object) -> None:
            observed.update(kwargs)
            output_dir = Path(str(kwargs["output_dir"]))
            task_stems = list(kwargs["pdf_file_names"])
            parse_method = str(kwargs["parse_method"])
            for index, task_stem in enumerate(task_stems):
                target = output_dir / task_stem / parse_method / f"{task_stem}.md"
                target.parent.mkdir(parents=True)
                target.write_text(f"markdown:{input_paths[index]}", encoding="utf-8")

        with mock.patch.object(wrapper, "_load_mineru_api", return_value=(do_parse, read_fn)):
            outputs, total_ms = wrapper._native_pipeline_markdown(input_paths, ocr_enabled=False)

        assert outputs == [f"markdown:{path}" for path in input_paths]
        assert total_ms >= 0.0
        assert observed["backend"] == "pipeline"
        assert observed["parse_method"] == "txt"
        assert observed["pdf_bytes_list"] == [b"bytes:second.pdf", b"bytes:first.pdf"]
        assert observed["pdf_file_names"] == [
            "benchmark_document_00000000",
            "benchmark_document_00000001",
        ]

    def test_batch_reports_only_makespan_and_unavailable_item_timings(self) -> None:
        """Expose a measured makespan without inventing per-document durations."""
        wrapper = _load_wrapper()
        with mock.patch.object(
            wrapper,
            "_native_pipeline_markdown",
            return_value=(["first", "second"], 42.5),
        ):
            payload = wrapper.extract_batch(["a.pdf", "b.pdf"], False)

        assert [item["content"] for item in payload["results"]] == ["first", "second"]
        assert payload["total_ms"] == 42.5
        assert payload["per_file_ms"] == [None, None]
        assert all("_extraction_time_ms" not in item for item in payload["results"])
        assert payload["metadata"]["model_batching"] == "cross_document_processing_windows_via_doc_analyze_streaming"

    def test_missing_native_output_fails_strict_cardinality(self) -> None:
        """Reject a batch when MinerU omits any ordered output."""
        wrapper = _load_wrapper()

        def do_parse(**kwargs: object) -> None:
            output_dir = Path(str(kwargs["output_dir"]))
            task_stem = next(iter(kwargs["pdf_file_names"]))
            parse_method = str(kwargs["parse_method"])
            target = output_dir / task_stem / parse_method / f"{task_stem}.md"
            target.parent.mkdir(parents=True)
            target.write_text("first", encoding="utf-8")

        with tempfile.TemporaryDirectory() as fixture_dir:
            paths = [str(Path(fixture_dir) / "a.pdf"), str(Path(fixture_dir) / "b.pdf")]
            read_fn = mock.Mock(return_value=b"pdf")
            with mock.patch.object(wrapper, "_load_mineru_api", return_value=(do_parse, read_fn)):
                error: RuntimeError | None = None
                try:
                    wrapper._native_pipeline_markdown(paths, ocr_enabled=True)
                except RuntimeError as caught:
                    error = caught
                else:
                    self.fail("missing native output unexpectedly succeeded")

        assert error is not None
        assert "cardinality mismatch" in str(error)

    def test_unexpected_native_markdown_output_fails_strict_cardinality(self) -> None:
        """Reject a batch when MinerU emits an unexpected Markdown output."""
        wrapper = _load_wrapper()

        def do_parse(**kwargs: object) -> None:
            output_dir = Path(str(kwargs["output_dir"]))
            parse_method = str(kwargs["parse_method"])
            for task_stem in kwargs["pdf_file_names"]:
                target = output_dir / task_stem / parse_method / f"{task_stem}.md"
                target.parent.mkdir(parents=True)
                target.write_text(task_stem, encoding="utf-8")
            extra = output_dir / "unexpected" / parse_method / "extra.md"
            extra.parent.mkdir(parents=True)
            extra.write_text("extra", encoding="utf-8")

        read_fn = mock.Mock(return_value=b"pdf")
        with mock.patch.object(wrapper, "_load_mineru_api", return_value=(do_parse, read_fn)):
            error: RuntimeError | None = None
            try:
                wrapper._native_pipeline_markdown(["a.pdf", "b.pdf"], ocr_enabled=False)
            except RuntimeError as caught:
                error = caught
            else:
                self.fail("unexpected native Markdown output was accepted")

        assert error is not None
        assert "unexpected=['unexpected/txt/extra.md']" in str(error)


if __name__ == "__main__":
    unittest.main()
