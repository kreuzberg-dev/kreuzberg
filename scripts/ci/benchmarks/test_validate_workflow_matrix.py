from __future__ import annotations

import importlib.util
import unittest
from pathlib import Path

REPOSITORY_ROOT = Path(__file__).resolve().parents[3]
VALIDATOR_PATH = Path(__file__).with_name("validate-workflow-matrix.py")
SPEC = importlib.util.spec_from_file_location("validate_workflow_matrix", VALIDATOR_PATH)
assert SPEC is not None and SPEC.loader is not None
VALIDATOR = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(VALIDATOR)
WORKFLOW = (REPOSITORY_ROOT / ".github/workflows/benchmarks.yaml").read_text()


class TestWorkflowMatrixValidation(unittest.TestCase):
    def test_expands_every_exact_contract_cell_once(self) -> None:
        cells = VALIDATOR.workflow_cells(WORKFLOW)

        assert len(cells) == 140
        assert len({VALIDATOR.cell_key(cell) for cell in cells}) == 140
        assert sum(not cell["optional"] for cell in cells) == 104

    def test_framework_env_drift_changes_cell_even_when_artifact_name_is_unchanged(self) -> None:
        mutated = WORKFLOW.replace("FRAMEWORK: docling", "FRAMEWORK: docling-wrong", 1)
        original_cells = VALIDATOR.workflow_cells(WORKFLOW)
        mutated_cells = VALIDATOR.workflow_cells(mutated)
        original = next(cell for cell in original_cells if cell["framework"] == "docling")
        changed = next(cell for cell in mutated_cells if cell["artifact"] == original["artifact"])

        assert changed["artifact"] == original["artifact"]
        assert changed["framework"] == "docling-wrong"
        assert VALIDATOR.cell_key(changed) != VALIDATOR.cell_key(original)

    def test_benchmark_steps_cannot_suppress_failures(self) -> None:
        mutated = WORKFLOW.replace(
            "      - name: Run benchmark\n        env:",
            "      - name: Run benchmark\n        continue-on-error: true\n        env:",
            1,
        )

        with self.assertRaisesRegex(ValueError, "must not use continue-on-error"):
            VALIDATOR.workflow_cells(mutated)

    def test_artifacts_must_be_validated_before_consolidation(self) -> None:
        mutated = (
            WORKFLOW.replace(
                "      - name: Validate benchmark artifacts (all cohorts)",
                "      - name: TEMP validation gate",
                1,
            )
            .replace(
                "      - name: Consolidate results (all cohorts)",
                "      - name: Validate benchmark artifacts (all cohorts)",
                1,
            )
            .replace(
                "      - name: TEMP validation gate",
                "      - name: Consolidate results (all cohorts)",
                1,
            )
        )

        with self.assertRaisesRegex(ValueError, "validation gates must precede publication"):
            VALIDATOR.workflow_cells(mutated)


if __name__ == "__main__":
    unittest.main()
