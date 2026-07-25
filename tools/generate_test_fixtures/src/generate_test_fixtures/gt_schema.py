"""Ground-truth sidecar schema.

Every binary fixture produced by this toolkit ships with a JSON sidecar of
the same stem (``foo.docx`` -> ``foo.gt.json``). Integration tests load the
pair and assert ``ExtractionResult`` / ``ExtractionDiff`` fields against the
``expectations`` dict.

The schema is intentionally feature-shaped rather than format-shaped: a
``revisions`` fixture's expectations look the same whether the underlying
file is DOCX, ODT, XLSX, or PPTX. This keeps the integration-test asserter
generic.
"""

from __future__ import annotations

import hashlib
import json
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any

from . import __version__

TOOL_NAME = "generate-test-fixtures"


@dataclass
class GroundTruth:
    """Structured expectations bound to a binary fixture.

    Attributes:
        fixture_path: Path of the binary fixture relative to the xberg
            repository root (e.g. ``test_documents/generated/docx/foo.docx``).
            Integration tests join this with the repo root to load the file.
        format: One of ``"docx" | "odt" | "xlsx" | "pptx" | "pdf"``.
        feature: One of ``"revisions" | "diff" | "security" | "embedded"``.
            Drives which assertion helper the integration test invokes.
        expectations: Feature-specific shape. See ``revisions_expectation``,
            ``diff_expectation``, and ``security_expectation`` helpers below
            for the canonical shapes.
        generated_by: ``"<tool-name> <version> (<generator-module>)"``.
    """

    fixture_path: str
    format: str
    feature: str
    expectations: dict[str, Any]
    generated_by: str = field(default_factory=lambda: f"{TOOL_NAME} {__version__}")


def write_ground_truth(
    sidecar_path: Path,
    fixture_path: Path,
    repo_root: Path,
    *,
    document_format: str,
    feature: str,
    expectations: dict[str, Any],
    generator: str,
) -> None:
    """Serialise a ``GroundTruth`` next to its binary fixture.

    Args:
        sidecar_path: Destination ``*.gt.json`` path.
        fixture_path: Absolute path of the companion binary fixture.
        repo_root: Repository root, used to make ``fixture_path`` relative
            in the sidecar so integration tests can resolve it portably.
        document_format: Canonical format string (see ``GroundTruth.format``).
        feature: Canonical feature string (see ``GroundTruth.feature``).
        expectations: Feature-specific shape.
        generator: Module name that produced the fixture, e.g.
            ``"docx_revisions"``.
    """
    try:
        relative = fixture_path.resolve().relative_to(repo_root.resolve())
    except ValueError:
        relative = fixture_path.resolve()

    gt = GroundTruth(
        fixture_path=str(relative).replace("\\", "/"),
        format=document_format,
        feature=feature,
        expectations=expectations,
        generated_by=f"{TOOL_NAME} {__version__} ({generator})",
    )
    sidecar_path.write_text(json.dumps(asdict(gt), indent=2, sort_keys=True) + "\n", encoding="utf-8")


def revisions_expectation(
    *,
    expected_count: int,
    revisions: list[dict[str, Any]],
    notes: str | None = None,
) -> dict[str, Any]:
    """Shape for ``feature="revisions"`` fixtures.

    Each entry in ``revisions`` mirrors the ``DocumentRevision`` struct
    fields the test should assert: ``kind`` ("Insertion" | "Deletion" |
    "FormatChange" | "Comment"), ``author``, ``timestamp``, ``revision_id``.
    Integration tests assert ``len(result.revisions) == expected_count``
    plus per-entry kind/author matching.
    """
    payload: dict[str, Any] = {"expected_count": expected_count, "revisions": revisions}
    if notes is not None:
        payload["notes"] = notes
    return payload


def diff_expectation(
    *,
    before_path: str,
    after_path: str,
    content_changed: bool,
    expected_added_lines: list[str],
    expected_removed_lines: list[str],
    table_cell_changes: list[dict[str, Any]] | None = None,
    notes: str | None = None,
) -> dict[str, Any]:
    """Shape for ``feature="diff"`` fixtures.

    The pair ``(before_path, after_path)`` are both relative-to-repo-root
    paths that the integration test extracts independently before calling
    ``xberg::diff::compare``. ``expected_added_lines`` / ``…_removed_lines``
    are substrings that MUST appear in some ``DiffLine::Added`` /
    ``DiffLine::Removed`` entry — substring match, not equality, since the
    extractor may add framing whitespace.
    """
    payload: dict[str, Any] = {
        "before_path": before_path,
        "after_path": after_path,
        "content_changed": content_changed,
        "expected_added_lines": expected_added_lines,
        "expected_removed_lines": expected_removed_lines,
        "table_cell_changes": table_cell_changes or [],
    }
    if notes is not None:
        payload["notes"] = notes
    return payload


def security_expectation(
    *,
    should_extract: bool,
    expected_warnings: list[str],
    notes: str | None = None,
) -> dict[str, Any]:
    """Shape for ``feature="security"`` fixtures.

    ``should_extract = False`` means extraction MUST return an error (e.g.
    the zip-bomb guard rejects the file). ``expected_warnings`` is a list
    of case-insensitive substrings; each must match at least one warning
    surfaced by the extractor.
    """
    payload: dict[str, Any] = {
        "should_extract": should_extract,
        "expected_warnings": expected_warnings,
    }
    if notes is not None:
        payload["notes"] = notes
    return payload


def file_sha256(path: Path) -> str:
    """Return the lowercase hex SHA-256 of ``path``.

    Useful when an integration test wants to assert the generator produced
    a byte-identical fixture across runs.
    """
    digest = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(64 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()
