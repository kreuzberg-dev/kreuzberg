# Copyright (c) 2026 Xberg. All rights reserved.
"""Validate the exact benchmark artifact contract for one fixed cohort."""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path, PurePosixPath
from typing import TYPE_CHECKING, TypeAlias

if TYPE_CHECKING:
    from collections.abc import Iterable

JsonScalar: TypeAlias = bool | int | float | str | None
JsonValue: TypeAlias = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]


@dataclass(frozen=True)
class MatrixEntry:
    artifact: str
    framework: str
    output_format: str
    mode: str

    @property
    def aggregate_key(self) -> str:
        aggregate_mode = "single" if self.mode == "single-file" else "batch"
        if self.framework.startswith("xberg-"):
            return f"{self.framework}:{aggregate_mode}"
        return f"{self.framework}:{self.output_format}:{aggregate_mode}"


@dataclass(frozen=True)
class CohortContract:
    manifest_name: str
    manifest_blake3: str
    batch_size: int
    fixtures: tuple[str, ...]
    document_stems: tuple[str, ...]
    matrix: tuple[MatrixEntry, ...]


@dataclass(frozen=True)
class ExpectedFixture:
    fixture: str
    fixture_blake3: str
    document_blake3: str
    document_bytes: int
    document_name: str


def matrix_entry(
    artifact: str,
    framework: str,
    output_format: str,
    mode: str,
) -> MatrixEntry:
    return MatrixEntry(artifact, framework, output_format, mode)


def xberg_entries(cohort: str) -> list[MatrixEntry]:
    return [
        matrix_entry(
            f"benchmarks-rust-{pipeline}-{output_format}-{mode}-{cohort}",
            f"xberg-{output_format}-{pipeline}",
            output_format,
            mode,
        )
        for pipeline in ("baseline", "layout")
        for output_format in ("markdown", "plaintext")
        for mode in ("single-file", "batch")
    ]


def grid_entries(framework: str, cohort: str) -> list[MatrixEntry]:
    return [
        matrix_entry(
            f"benchmarks-{framework}-{output_format}-{mode}-{cohort}",
            framework,
            output_format,
            mode,
        )
        for output_format in ("markdown", "plaintext")
        for mode in ("single-file", "batch")
    ]


def markdown_single_file_entries(framework: str, cohort: str) -> list[MatrixEntry]:
    """Return the native Markdown single-file cell."""
    return [
        matrix_entry(
            f"benchmarks-{framework}-markdown-single-file-{cohort}",
            framework,
            "markdown",
            "single-file",
        )
    ]


NATIVE_COHORT = "native-pdf-fast-b8"
OCR_COHORT = "ocr-pdf-fast-b4"

CONTRACTS = {
    "native": CohortContract(
        manifest_name="native-pdf-fast-b8-v1",
        manifest_blake3="c10d1f78d3f9d61070c0d91e7bfe90a904b69c9d3261536eb1dcff2081c73f6b",
        batch_size=8,
        fixtures=(
            "pdf_tiny_memo.json",
            "pdf_tables.json",
            "pdf_embedded.json",
            "pdf_google_docs.json",
            "pdf/681693.json",
            "pdf/ft_ACN_2009_page_102_t0.json",
            "pdf/pb_fqr-retail-blackrock-global-allocation-fund-inc_page4.json",
            "pdf/pb_sample_page_16_page1.json",
        ),
        document_stems=(
            "fake_memo",
            "tiny",
            "embedded_images_tables",
            "google_doc_document",
            "681693",
            "ft_A\x43N_2009_page_102_t0",
            "pb_fqr-retail-blackrock-global-allocation-fund-inc_page4",
            "pb_sample_page_16_page1",
        ),
        matrix=tuple(
            xberg_entries(NATIVE_COHORT)
            + grid_entries("docling", NATIVE_COHORT)
            + [
                matrix_entry(
                    f"benchmarks-markitdown-markdown-single-file-{NATIVE_COHORT}",
                    "markitdown",
                    "markdown",
                    "single-file",
                ),
                matrix_entry(
                    f"benchmarks-unstructured-plaintext-single-file-{NATIVE_COHORT}",
                    "unstructured",
                    "plaintext",
                    "single-file",
                ),
                matrix_entry(
                    f"benchmarks-tika-plaintext-single-file-{NATIVE_COHORT}",
                    "tika",
                    "plaintext",
                    "single-file",
                ),
                matrix_entry(
                    f"benchmarks-pymupdf4llm-markdown-single-file-{NATIVE_COHORT}",
                    "pymupdf4llm",
                    "markdown",
                    "single-file",
                ),
            ]
            + markdown_single_file_entries("mineru", NATIVE_COHORT)
            + grid_entries("liteparse", NATIVE_COHORT)
        ),
    ),
    "ocr": CohortContract(
        manifest_name="ocr-pdf-fast-b4-v1",
        manifest_blake3="c740a9480fd0ad4311a4905206345c58d7ef7a987682cc96644e2f0ffe616a13",
        batch_size=4,
        fixtures=(
            "pdf_non_searchable.json",
            "pdf_ocr_test.json",
            "pdf_scanned_ocr.json",
            "pdf_image_only_german.json",
        ),
        document_stems=("non_searchable", "ocr_test", "scanned", "image_only_german_pdf"),
        matrix=tuple(
            xberg_entries(OCR_COHORT)
            + grid_entries("docling", OCR_COHORT)
            + markdown_single_file_entries("mineru", OCR_COHORT)
            + grid_entries("liteparse", OCR_COHORT)
        ),
    ),
}


class ContractError(ValueError):
    """An artifact violates the benchmark release contract."""


def load_json(path: Path) -> JsonValue:
    try:
        return json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise ContractError(f"{path}: malformed or unreadable JSON: {error}") from error


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ContractError(message)


def blake3_file(path: Path) -> str:
    require(path.is_file(), f"{path}: expected a regular file")
    try:
        result = subprocess.run(
            ["b3sum", str(path)],
            check=True,
            capture_output=True,
            text=True,
        )
    except (OSError, subprocess.CalledProcessError) as error:
        raise ContractError(f"{path}: failed to compute BLAKE3: {error}") from error
    digest = result.stdout.split(maxsplit=1)[0] if result.stdout else ""
    require(
        len(digest) == 64 and all(char in "0123456789abcdef" for char in digest),
        f"{path}: b3sum returned a malformed digest",
    )
    return digest


def validate_manifest(path: Path, contract: CohortContract) -> str:
    manifest = load_json(path)
    require(isinstance(manifest, dict), f"{path}: manifest must be an object")
    require(manifest.get("schema_version") == 1, f"{path}: unexpected schema_version")
    require(manifest.get("name") == contract.manifest_name, f"{path}: unexpected cohort name")
    require(manifest.get("batch_size") == contract.batch_size, f"{path}: unexpected batch_size")
    fixtures = manifest.get("fixtures")
    require(isinstance(fixtures, list), f"{path}: fixtures must be an array")
    require(tuple(fixtures) == contract.fixtures, f"{path}: fixture order/content mismatch")
    digest = blake3_file(path)
    require(digest == contract.manifest_blake3, f"{path}: cohort manifest BLAKE3 mismatch")
    return digest


def expected_fixtures(fixtures_root: Path, contract: CohortContract) -> tuple[ExpectedFixture, ...]:
    expected = []
    for fixture in contract.fixtures:
        descriptor_path = fixtures_root / fixture
        descriptor = load_json(descriptor_path)
        require(isinstance(descriptor, dict), f"{descriptor_path}: fixture descriptor must be an object")
        document = descriptor.get("document")
        require(isinstance(document, str) and document, f"{descriptor_path}: document must be a non-empty string")
        document_path = descriptor_path.parent / document
        document_name = PurePosixPath(document.replace("\\", "/")).name
        expected.append(
            ExpectedFixture(
                fixture=fixture,
                fixture_blake3=blake3_file(descriptor_path),
                document_blake3=blake3_file(document_path),
                document_bytes=document_path.stat().st_size,
                document_name=document_name,
            )
        )
    names = tuple(item.document_name for item in expected)
    require(len(set(names)) == len(names), "cohort document basenames must be unique")
    require(
        tuple(Path(name).stem for name in names) == contract.document_stems,
        "cohort document identities do not match the release contract",
    )
    return tuple(expected)


def only_file(root: Path, filename: str) -> Path:
    matches = sorted(root.rglob(filename))
    require(len(matches) == 1, f"{root}: expected exactly one {filename}, found {len(matches)}")
    return matches[0]


def validate_provenance(
    provenance: JsonValue,
    path: Path,
    *,
    entry: MatrixEntry,
    contract: CohortContract,
    manifest_blake3: str,
    expected: tuple[ExpectedFixture, ...],
    source_sha: str,
    iterations: int,
) -> None:
    require(isinstance(provenance, dict), f"{path}: provenance must be an object")
    require(provenance.get("schema_version") == 2, f"{path}: unexpected provenance schema")
    repository = provenance.get("repository")
    require(isinstance(repository, dict), f"{path}: repository provenance missing")
    require(repository.get("commit") == source_sha, f"{path}: source SHA mismatch")
    require(repository.get("dirty") is False, f"{path}: benchmark checkout was dirty")

    corpus = provenance.get("corpus")
    require(isinstance(corpus, dict), f"{path}: corpus provenance missing")
    require(corpus.get("cohort") == contract.manifest_name, f"{path}: cohort name mismatch")
    require(
        corpus.get("cohort_manifest_blake3") == manifest_blake3,
        f"{path}: cohort manifest hash mismatch",
    )
    ordered = corpus.get("ordered_fixtures")
    require(isinstance(ordered, list), f"{path}: ordered_fixtures must be an array")
    require(len(ordered) == len(expected), f"{path}: fixture count mismatch")
    for index, (item, expected_item) in enumerate(zip(ordered, expected, strict=True)):
        require(isinstance(item, dict), f"{path}: fixture provenance {index} must be an object")
        require(
            item.get("fixture") == expected_item.fixture,
            f"{path}: fixture {index} identity/order mismatch",
        )
        require(
            item.get("fixture_blake3") == expected_item.fixture_blake3,
            f"{path}: fixture {index} descriptor BLAKE3 mismatch",
        )
        require(
            item.get("document_blake3") == expected_item.document_blake3,
            f"{path}: fixture {index} document BLAKE3 mismatch",
        )
        require(
            item.get("document_bytes") == expected_item.document_bytes,
            f"{path}: fixture {index} document size mismatch",
        )

    timing = provenance.get("timing")
    require(isinstance(timing, dict), f"{path}: timing provenance missing")
    expected_mode = "SingleFile" if entry.mode == "single-file" else "Batch"
    require(timing.get("mode") == expected_mode, f"{path}: execution mode mismatch")
    require(timing.get("benchmark_iterations") == iterations, f"{path}: iteration count mismatch")
    require(timing.get("output_format") == entry.output_format, f"{path}: output format mismatch")
    expected_batch = contract.batch_size if entry.mode == "batch" else None
    require(provenance.get("fixed_batch_size") == expected_batch, f"{path}: fixed batch size mismatch")

    frameworks = provenance.get("frameworks")
    require(isinstance(frameworks, list) and len(frameworks) == 1, f"{path}: expected one framework")
    framework = frameworks[0]
    require(isinstance(framework, dict), f"{path}: framework provenance must be an object")
    require(framework.get("name") == entry.framework, f"{path}: framework mismatch")
    require(framework.get("eligible_documents") == len(contract.fixtures), f"{path}: fixture count mismatch")
    expected_partitions = len(contract.fixtures) // contract.batch_size if entry.mode == "batch" else None
    require(framework.get("batch_partitions") == expected_partitions, f"{path}: batch partition mismatch")


def validate_results(
    results: JsonValue,
    path: Path,
    *,
    entry: MatrixEntry,
    contract: CohortContract,
    document_names: tuple[str, ...],
    iterations: int,
) -> None:
    require(isinstance(results, list), f"{path}: results must be an array")
    require(len(results) == len(contract.fixtures), f"{path}: result fixture count mismatch")
    actual_names = tuple(
        PurePosixPath(str(result.get("file_path", "")).replace("\\", "/")).name
        for result in results
        if isinstance(result, dict)
    )
    require(actual_names == document_names, f"{path}: result fixture order/content mismatch")
    require(len(set(actual_names)) == len(actual_names), f"{path}: duplicate fixture results")

    expected_ocr = "used" if contract is CONTRACTS["ocr"] else "not_used"
    for index, result in enumerate(results):
        require(isinstance(result, dict), f"{path}: result {index} must be an object")
        require(result.get("framework") == entry.framework, f"{path}: result {index} framework mismatch")
        require(result.get("output_format") == entry.output_format, f"{path}: result {index} format mismatch")
        require(result.get("success") is True, f"{path}: result {index} failed")
        require(result.get("error_kind") == "none", f"{path}: result {index} has an error")
        require(result.get("error_message") is None, f"{path}: result {index} has an error message")
        require(result.get("ocr_status") == expected_ocr, f"{path}: result {index} OCR status mismatch")
        run_iterations = result.get("iterations")
        require(isinstance(run_iterations, list), f"{path}: result {index} iterations must be an array")
        require(len(run_iterations) == iterations, f"{path}: result {index} iteration count mismatch")
        require(
            [item.get("iteration") for item in run_iterations if isinstance(item, dict)] == list(range(iterations)),
            f"{path}: result {index} iteration order/duplicates mismatch",
        )


def validate_artifacts(args: argparse.Namespace, contract: CohortContract) -> None:
    manifest_blake3 = validate_manifest(args.cohort_manifest, contract)
    fixtures = expected_fixtures(args.fixtures_root, contract)
    documents = tuple(item.document_name for item in fixtures)
    expected_names = {f"{entry.artifact}-{args.run_id}": entry for entry in contract.matrix}
    actual_dirs = {path.name: path for path in args.artifacts_dir.iterdir() if path.is_dir()}
    require(set(actual_dirs) == set(expected_names), describe_set_mismatch("artifacts", expected_names, actual_dirs))

    for artifact_name, entry in expected_names.items():
        artifact_dir = actual_dirs[artifact_name]
        results_path = only_file(artifact_dir, "results.json")
        provenance_path = only_file(artifact_dir, "provenance.json")
        validate_provenance(
            load_json(provenance_path),
            provenance_path,
            entry=entry,
            contract=contract,
            manifest_blake3=manifest_blake3,
            expected=fixtures,
            source_sha=args.source_sha,
            iterations=args.iterations,
        )
        validate_results(
            load_json(results_path),
            results_path,
            entry=entry,
            contract=contract,
            document_names=documents,
            iterations=args.iterations,
        )
    print(f"validated {len(expected_names)} {args.cohort} benchmark artifacts")


def describe_set_mismatch(label: str, expected: Iterable[object], actual: Iterable[object]) -> str:
    missing = sorted(set(expected) - set(actual))
    unexpected = sorted(set(actual) - set(expected))
    return f"{label} mismatch; missing={missing}, unexpected={unexpected}"


def validate_aggregate(args: argparse.Namespace, contract: CohortContract) -> None:
    aggregate = load_json(args.aggregated_file)
    require(isinstance(aggregate, dict), f"{args.aggregated_file}: aggregate must be an object")
    require(aggregate.get("schema_version") == "2.6.0", f"{args.aggregated_file}: unexpected schema")
    groups = aggregate.get("by_framework_mode")
    require(isinstance(groups, dict), f"{args.aggregated_file}: by_framework_mode missing")
    expected_keys = {entry.aggregate_key for entry in contract.matrix}
    require(set(groups) == expected_keys, describe_set_mismatch("aggregate keys", expected_keys, groups))
    for key, group in groups.items():
        require(isinstance(group, dict), f"{args.aggregated_file}: group {key} must be an object")
        by_file_type = group.get("by_file_type")
        require(isinstance(by_file_type, dict), f"{args.aggregated_file}: group {key} missing by_file_type")
        require(set(by_file_type) == {"pdf"}, f"{args.aggregated_file}: group {key} must contain only pdf metrics")
        file_group = by_file_type["pdf"]
        require(isinstance(file_group, dict), f"{args.aggregated_file}: malformed file group in {key}")
        expected_bucket = "with_ocr" if args.cohort == "ocr" else "no_ocr"
        opposite_bucket = "no_ocr" if args.cohort == "ocr" else "with_ocr"
        require(file_group.get(opposite_bucket) is None, f"{args.aggregated_file}: group {key} has wrong OCR bucket")
        bucket = file_group.get(expected_bucket)
        require(isinstance(bucket, dict), f"{args.aggregated_file}: group {key} missing {expected_bucket}")
        require(bucket.get("total_sample_count") == len(contract.fixtures), f"{key}: fixture count mismatch")
        for error_field in (
            "framework_errors",
            "harness_errors",
            "config_setup_errors",
            "timeouts",
            "empty_content",
        ):
            require(bucket.get(error_field) == 0, f"{key}: nonzero {error_field}")

    rows = aggregate.get("per_fixture_results")
    require(isinstance(rows, list), f"{args.aggregated_file}: per_fixture_results missing")
    expected_rows = len(contract.matrix) * len(contract.fixtures)
    require(len(rows) == expected_rows, f"{args.aggregated_file}: expected {expected_rows} fixture rows")
    require(
        all(isinstance(row, dict) for row in rows),
        f"{args.aggregated_file}: every fixture row must be an object",
    )
    identities = {
        (
            row.get("framework"),
            row.get("output_format"),
            row.get("execution_mode"),
            row.get("fixture_id"),
            row.get("ocr"),
        )
        for row in rows
        if isinstance(row, dict)
    }
    expected_ocr = args.cohort == "ocr"
    expected_identities = {
        (
            entry.framework,
            entry.output_format,
            "single" if entry.mode == "single-file" else "batch",
            fixture_id,
            expected_ocr,
        )
        for entry in contract.matrix
        for fixture_id in contract.document_stems
    }
    require(
        identities == expected_identities,
        describe_set_mismatch("aggregate fixture rows", expected_identities, identities),
    )
    require(
        all(isinstance(row, dict) and row.get("success") is True for row in rows),
        f"{args.aggregated_file}: failed fixture rows",
    )
    print(f"validated {len(expected_keys)} {args.cohort} aggregate keys and {len(rows)} fixture rows")


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser()
    parser.add_argument("--cohort", choices=sorted(CONTRACTS), required=True)
    parser.add_argument("--aggregated-file", type=Path)
    parser.add_argument("--artifacts-dir", type=Path)
    parser.add_argument("--cohort-manifest", type=Path)
    parser.add_argument("--fixtures-root", type=Path)
    parser.add_argument("--source-sha")
    parser.add_argument("--run-id")
    parser.add_argument("--iterations", type=int, default=3)
    args = parser.parse_args()
    if args.aggregated_file is None:
        required = ("artifacts_dir", "cohort_manifest", "fixtures_root", "source_sha", "run_id")
        missing = [name.replace("_", "-") for name in required if getattr(args, name) in (None, "")]
        if missing:
            parser.error(f"artifact validation requires: {', '.join(missing)}")
    return args


def main() -> int:
    args = parse_args()
    try:
        contract = CONTRACTS[args.cohort]
        if args.aggregated_file is not None:
            validate_aggregate(args, contract)
        else:
            validate_artifacts(args, contract)
    except (ContractError, OSError) as error:
        print(f"benchmark artifact validation failed: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
