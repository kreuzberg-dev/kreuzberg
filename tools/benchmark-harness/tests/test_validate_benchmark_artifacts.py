# Copyright (c) 2026 Xberg. All rights reserved.
"""Tests for the remote benchmark artifact contract validator."""
# ruff: noqa: D101, D102, PT009, PT027

from __future__ import annotations

import importlib.util
import json
import shutil
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/ci/benchmarks/validate-benchmark-artifacts.py"
SPEC = importlib.util.spec_from_file_location("validate_benchmark_artifacts", SCRIPT)
assert SPEC and SPEC.loader
validator = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = validator
SPEC.loader.exec_module(validator)


class Args:
    """Minimal argument namespace for validator calls."""

    cohort = "native"
    source_sha = "a" * 40
    run_id = "42"
    iterations = 3
    aggregated_file = None


class ValidateBenchmarkArtifactsTests(unittest.TestCase):
    """Validate strict raw-artifact acceptance and rejection paths."""

    def setUp(self) -> None:
        """Create a complete native cohort artifact tree."""
        self.temp = tempfile.TemporaryDirectory()
        self.root = Path(self.temp.name)
        self.args = Args()
        self.args.artifacts_dir = self.root / "artifacts"
        self.args.cohort_manifest = self.root / "cohort.json"
        self.args.fixtures_root = self.root / "tools/benchmark-harness/fixtures"
        self.args.artifacts_dir.mkdir()
        self.args.fixtures_root.mkdir(parents=True)
        self.contract = validator.CONTRACTS["native"]
        self.write_manifest_and_fixtures()
        self.write_artifacts()

    def tearDown(self) -> None:
        """Remove the temporary artifact tree."""
        self.temp.cleanup()

    def write_json(self, path: Path, value: object) -> None:
        """Write one JSON fixture."""
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(value), encoding="utf-8")

    def write_manifest_and_fixtures(self) -> None:
        """Copy the exact cohort and create real referenced fixture documents."""
        cohort_name = validator.NATIVE_COHORT if self.args.cohort == "native" else validator.OCR_COHORT
        source_manifest = ROOT / "tools/benchmark-harness/cohorts" / f"{cohort_name}.json"
        shutil.copyfile(source_manifest, self.args.cohort_manifest)
        self.fixture_provenance = []
        for fixture, document_stem in zip(self.contract.fixtures, self.contract.document_stems, strict=True):
            source_fixture = ROOT / "tools/benchmark-harness/fixtures" / fixture
            descriptor_path = self.args.fixtures_root / fixture
            descriptor_path.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(source_fixture, descriptor_path)
            descriptor = json.loads(descriptor_path.read_text(encoding="utf-8"))
            document_path = descriptor_path.parent / descriptor["document"]
            document_path.parent.mkdir(parents=True, exist_ok=True)
            document_path.write_bytes(f"temporary benchmark document: {document_stem}\n".encode())
            self.fixture_provenance.append(
                {
                    "fixture": fixture,
                    "fixture_blake3": self.blake3(descriptor_path),
                    "document_blake3": self.blake3(document_path),
                    "document_bytes": document_path.stat().st_size,
                }
            )

    def blake3(self, path: Path) -> str:
        """Return the real BLAKE3 digest produced by b3sum."""
        b3sum = shutil.which("b3sum")
        if b3sum is None:
            self.fail("b3sum is required for benchmark validator tests")
        result = subprocess.run(
            [b3sum, str(path)],
            check=True,
            capture_output=True,
            text=True,
        )
        return result.stdout.split(maxsplit=1)[0]

    def provenance(self, entry: validator.MatrixEntry) -> dict[str, object]:
        """Build valid provenance for one matrix entry."""
        batch = entry.mode == "batch"
        return {
            "schema_version": 2,
            "repository": {"commit": self.args.source_sha, "dirty": False},
            "corpus": {
                "cohort": self.contract.manifest_name,
                "cohort_manifest_blake3": self.blake3(self.args.cohort_manifest),
                "ordered_fixtures": self.fixture_provenance,
            },
            "frameworks": [
                {
                    "name": entry.framework,
                    "eligible_documents": len(self.contract.fixtures),
                    "batch_partitions": len(self.contract.fixtures) // self.contract.batch_size if batch else None,
                }
            ],
            "timing": {
                "mode": "Batch" if batch else "SingleFile",
                "benchmark_iterations": self.args.iterations,
                "output_format": entry.output_format,
            },
            "fixed_batch_size": self.contract.batch_size if batch else None,
        }

    def results(self, entry: validator.MatrixEntry) -> list[dict[str, object]]:
        """Build ordered successful results for one matrix entry."""
        return [
            {
                "framework": entry.framework,
                "output_format": entry.output_format,
                "file_path": f"/workspace/test_documents/{Path(fixture).stem}.pdf",
                "success": True,
                "error_kind": "none",
                "error_message": None,
                "ocr_status": "used" if self.args.cohort == "ocr" else "not_used",
                "iterations": [{"iteration": index} for index in range(self.args.iterations)],
            }
            for fixture in self.contract.document_stems
        ]

    def write_artifacts(self) -> None:
        """Write every expected native matrix artifact."""
        for entry in self.contract.matrix:
            artifact = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run"
            self.write_json(artifact / "provenance.json", self.provenance(entry))
            self.write_json(artifact / "results.json", self.results(entry))

    def select_contract(self, name: str) -> None:
        """Rebuild the fixture tree for another cohort contract."""
        shutil.rmtree(self.args.artifacts_dir)
        shutil.rmtree(self.args.fixtures_root)
        self.args.artifacts_dir.mkdir()
        self.args.fixtures_root.mkdir()
        self.args.cohort = name
        self.contract = validator.CONTRACTS[name]
        self.write_manifest_and_fixtures()
        self.write_artifacts()

    def aggregate(self) -> dict[str, object]:
        """Build an exact aggregate for the selected cohort."""
        expected_ocr = self.args.cohort == "ocr"
        return {
            "schema_version": "2.6.0",
            "by_framework_mode": {
                entry.aggregate_key: {
                    "by_file_type": {
                        "pdf": {
                            "no_ocr": None
                            if expected_ocr
                            else {
                                "total_sample_count": len(self.contract.fixtures),
                                "framework_errors": 0,
                                "harness_errors": 0,
                                "config_setup_errors": 0,
                                "timeouts": 0,
                                "empty_content": 0,
                            },
                            "with_ocr": {
                                "total_sample_count": len(self.contract.fixtures),
                                "framework_errors": 0,
                                "harness_errors": 0,
                                "config_setup_errors": 0,
                                "timeouts": 0,
                                "empty_content": 0,
                            }
                            if expected_ocr
                            else None,
                        }
                    }
                }
                for entry in self.contract.matrix
            },
            "per_fixture_results": [
                {
                    "framework": entry.framework,
                    "output_format": entry.output_format,
                    "execution_mode": "single" if entry.mode == "single-file" else "batch",
                    "fixture_id": fixture,
                    "ocr": expected_ocr,
                    "success": True,
                }
                for entry in self.contract.matrix
                for fixture in self.contract.document_stems
            ],
        }

    def test_accepts_exact_native_contract(self) -> None:
        """The full exact native contract is accepted."""
        validator.validate_artifacts(self.args, self.contract)

    def test_raw_cli_subprocess_accepts_exact_native_contract(self) -> None:
        """The public CLI accepts a complete raw artifact tree."""
        result = subprocess.run(
            [
                sys.executable,
                str(SCRIPT),
                "--cohort",
                self.args.cohort,
                "--artifacts-dir",
                str(self.args.artifacts_dir),
                "--cohort-manifest",
                str(self.args.cohort_manifest),
                "--fixtures-root",
                str(self.args.fixtures_root),
                "--source-sha",
                self.args.source_sha,
                "--run-id",
                self.args.run_id,
                "--iterations",
                str(self.args.iterations),
            ],
            check=False,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, "validated 22 native benchmark artifacts\n")
        self.assertEqual(result.stderr, "")

    def test_accepts_exact_ocr_contract(self) -> None:
        """The full exact OCR contract is accepted."""
        self.select_contract("ocr")
        validator.validate_artifacts(self.args, self.contract)

    def test_rejects_tampered_manifest_bytes(self) -> None:
        """Semantically equivalent manifest bytes cannot bypass the pinned digest."""
        with self.args.cohort_manifest.open("a", encoding="utf-8") as manifest:
            manifest.write(" ")
        with self.assertRaisesRegex(validator.ContractError, "manifest BLAKE3 mismatch"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_wrong_fixture_digest_for_real_descriptor(self) -> None:
        """Fixture provenance must match the temporary descriptor bytes."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/provenance.json"
        provenance = json.loads(path.read_text(encoding="utf-8"))
        provenance["corpus"]["ordered_fixtures"][0]["fixture_blake3"] = "0" * 64
        self.write_json(path, provenance)
        with self.assertRaisesRegex(validator.ContractError, "descriptor BLAKE3 mismatch"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_wrong_document_digest_for_real_document(self) -> None:
        """Document provenance must match the temporary document bytes."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/provenance.json"
        provenance = json.loads(path.read_text(encoding="utf-8"))
        provenance["corpus"]["ordered_fixtures"][0]["document_blake3"] = "0" * 64
        self.write_json(path, provenance)
        with self.assertRaisesRegex(validator.ContractError, "document BLAKE3 mismatch"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_wrong_document_bytes_for_real_document(self) -> None:
        """Document size provenance must match the temporary document bytes."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/provenance.json"
        provenance = json.loads(path.read_text(encoding="utf-8"))
        provenance["corpus"]["ordered_fixtures"][0]["document_bytes"] += 1
        self.write_json(path, provenance)
        with self.assertRaisesRegex(validator.ContractError, "document size mismatch"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_unexpected_artifact(self) -> None:
        """Unexpected artifact names fail closed."""
        (self.args.artifacts_dir / "benchmarks-surprise-42").mkdir()
        with self.assertRaisesRegex(validator.ContractError, "unexpected"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_source_sha_mismatch(self) -> None:
        """A source revision mismatch fails closed."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/provenance.json"
        provenance = json.loads(path.read_text(encoding="utf-8"))
        provenance["repository"]["commit"] = "d" * 40
        self.write_json(path, provenance)
        with self.assertRaisesRegex(validator.ContractError, "source SHA mismatch"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_timeout_result(self) -> None:
        """Timeout rows fail the zero-error release contract."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/results.json"
        results = json.loads(path.read_text(encoding="utf-8"))
        results[0]["success"] = False
        results[0]["error_kind"] = "timeout"
        self.write_json(path, results)
        with self.assertRaisesRegex(validator.ContractError, "failed"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_duplicate_fixture_result(self) -> None:
        """Duplicate or reordered fixture rows fail closed."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/results.json"
        results = json.loads(path.read_text(encoding="utf-8"))
        results[1]["file_path"] = results[0]["file_path"]
        self.write_json(path, results)
        with self.assertRaisesRegex(validator.ContractError, "order/content mismatch"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_malformed_provenance(self) -> None:
        """Malformed provenance JSON fails closed."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/provenance.json"
        path.write_text("{", encoding="utf-8")
        with self.assertRaisesRegex(validator.ContractError, "malformed"):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_manifest_fixtures_when_not_an_array_with_contract_error(self) -> None:
        """A malformed manifest fixture collection fails with ContractError."""
        manifest = json.loads(self.args.cohort_manifest.read_text(encoding="utf-8"))
        manifest["fixtures"] = {}
        self.write_json(self.args.cohort_manifest, manifest)
        with self.assertRaises(validator.ContractError):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_framework_when_not_an_object_with_contract_error(self) -> None:
        """A malformed framework entry fails with ContractError."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/provenance.json"
        provenance = json.loads(path.read_text(encoding="utf-8"))
        provenance["frameworks"] = [None]
        self.write_json(path, provenance)
        with self.assertRaises(validator.ContractError):
            validator.validate_artifacts(self.args, self.contract)

    def test_rejects_result_row_when_not_an_object_with_contract_error(self) -> None:
        """A malformed raw result row fails with ContractError."""
        entry = self.contract.matrix[0]
        path = self.args.artifacts_dir / f"{entry.artifact}-{self.args.run_id}" / "run/results.json"
        results = json.loads(path.read_text(encoding="utf-8"))
        results[0] = None
        self.write_json(path, results)
        with self.assertRaises(validator.ContractError):
            validator.validate_artifacts(self.args, self.contract)

    def test_accepts_exact_aggregate_contract(self) -> None:
        """The consolidated cohort must retain every exact capability key."""
        path = self.root / "aggregated.json"
        self.write_json(path, self.aggregate())
        self.args.aggregated_file = path
        validator.validate_aggregate(self.args, self.contract)

    def test_accepts_exact_ocr_aggregate_contract(self) -> None:
        """The OCR aggregate retains the forced-OCR bucket and exact keys."""
        self.select_contract("ocr")
        path = self.root / "aggregated-ocr.json"
        self.write_json(path, self.aggregate())
        self.args.aggregated_file = path
        validator.validate_aggregate(self.args, self.contract)

    def test_rejects_unexpected_aggregate_key(self) -> None:
        """An unexpected consolidated key fails closed."""
        path = self.root / "aggregated.json"
        self.write_json(
            path,
            {
                "schema_version": "2.6.0",
                "by_framework_mode": {"surprise:markdown:single": {}},
                "per_fixture_results": [],
            },
        )
        self.args.aggregated_file = path
        with self.assertRaisesRegex(validator.ContractError, "unexpected"):
            validator.validate_aggregate(self.args, self.contract)

    def test_rejects_native_aggregate_with_only_with_ocr_bucket(self) -> None:
        """Native aggregates cannot substitute forced-OCR metrics."""
        aggregate = self.aggregate()
        for group in aggregate["by_framework_mode"].values():
            file_group = group["by_file_type"]["pdf"]
            file_group["with_ocr"] = file_group["no_ocr"]
            file_group["no_ocr"] = None
        path = self.root / "aggregated-native-with-ocr.json"
        self.write_json(path, aggregate)
        self.args.aggregated_file = path
        with self.assertRaises(validator.ContractError):
            validator.validate_aggregate(self.args, self.contract)

    def test_rejects_ocr_aggregate_with_only_no_ocr_bucket(self) -> None:
        """OCR aggregates cannot substitute native metrics."""
        self.select_contract("ocr")
        aggregate = self.aggregate()
        for group in aggregate["by_framework_mode"].values():
            file_group = group["by_file_type"]["pdf"]
            file_group["no_ocr"] = file_group["with_ocr"]
            file_group["with_ocr"] = None
        path = self.root / "aggregated-ocr-no-ocr.json"
        self.write_json(path, aggregate)
        self.args.aggregated_file = path
        with self.assertRaises(validator.ContractError):
            validator.validate_aggregate(self.args, self.contract)

    def test_rejects_aggregate_missing_pdf_file_type(self) -> None:
        """Every aggregate group must contain the PDF file type."""
        aggregate = self.aggregate()
        first_group = next(iter(aggregate["by_framework_mode"].values()))
        first_group["by_file_type"].pop("pdf")
        path = self.root / "aggregated-missing-pdf.json"
        self.write_json(path, aggregate)
        self.args.aggregated_file = path
        with self.assertRaises(validator.ContractError):
            validator.validate_aggregate(self.args, self.contract)

    def test_rejects_aggregate_extra_file_type(self) -> None:
        """Aggregate groups cannot contain unrequested file types."""
        aggregate = self.aggregate()
        first_group = next(iter(aggregate["by_framework_mode"].values()))
        first_group["by_file_type"]["docx"] = first_group["by_file_type"]["pdf"]
        path = self.root / "aggregated-extra-docx.json"
        self.write_json(path, aggregate)
        self.args.aggregated_file = path
        with self.assertRaises(validator.ContractError):
            validator.validate_aggregate(self.args, self.contract)

    def test_rejects_aggregate_row_when_not_an_object_with_contract_error(self) -> None:
        """A malformed aggregate fixture row fails with ContractError."""
        aggregate = self.aggregate()
        aggregate["per_fixture_results"][0] = None
        path = self.root / "aggregated-malformed-row.json"
        self.write_json(path, aggregate)
        self.args.aggregated_file = path
        with self.assertRaises(validator.ContractError):
            validator.validate_aggregate(self.args, self.contract)

    def test_contract_key_counts_are_exact(self) -> None:
        """Capability matrices contain the documented unique keys."""
        self.assertEqual(len(validator.CONTRACTS["native"].matrix), 22)
        self.assertEqual(len(validator.CONTRACTS["ocr"].matrix), 18)
        for contract in validator.CONTRACTS.values():
            self.assertEqual(len({entry.artifact for entry in contract.matrix}), len(contract.matrix))
            self.assertEqual(len({entry.aggregate_key for entry in contract.matrix}), len(contract.matrix))


if __name__ == "__main__":
    unittest.main()
