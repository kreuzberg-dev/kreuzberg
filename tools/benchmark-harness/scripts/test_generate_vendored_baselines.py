import json
import sys
import tempfile
import unittest
from contextlib import ExitStack
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock, patch

import numpy as np
from PIL import Image

# ~keep: This script intentionally uses unittest so it can run without pytest.
# ruff: noqa: D101, D102, PT009, PT027

sys.path.insert(0, str(Path(__file__).resolve().parent))
import generate_vendored_baselines as baselines


class VendoredBaselineTests(unittest.TestCase):
    def test_load_ocr_fixture_paths_includes_image_cohort(self):
        names = [path.name for path in baselines.load_ocr_fixture_paths()]

        self.assertEqual(
            names,
            [
                *(f"{name}.json" for name in baselines.PDF_OCR_FIXTURES),
                "cord_receipt_01.json",
                "cord_receipt_02.json",
                "cord_receipt_03.json",
                "cord_receipt_04.json",
                "doclaynet_page_01.json",
                "doclaynet_page_02.json",
                "ndl_meiji_vertical_01.json",
                "ndl_meiji_vertical_02.json",
                "ndl_meiji_vertical_03.json",
                "ndl_meiji_vertical_04.json",
                "ndl_meiji_vertical_05.json",
                "textocr_scene_01.json",
                "textocr_scene_02.json",
                "textocr_scene_03.json",
            ],
        )

    def test_load_ocr_fixture_paths_filters_exact_category_in_filename_order(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixtures_dir = Path(temporary_directory)
            nested_dir = fixtures_dir / "nested"
            nested_dir.mkdir()
            fixtures = {
                "z_match.json": {"metadata": {"category": "image-ocr-realgt"}},
                "a_other.json": {"metadata": {"category": "image-ocr"}},
                "nested/b_match.json": {"metadata": {"category": "image-ocr-realgt"}},
                "c_missing.json": {"metadata": {}},
            }
            for name, fixture in fixtures.items():
                (fixtures_dir / name).write_text(json.dumps(fixture), encoding="utf-8")

            with patch.object(baselines, "FIXTURES_DIR", fixtures_dir):
                paths = baselines.load_ocr_fixture_paths("image-ocr-realgt")

        self.assertEqual(
            [path.relative_to(fixtures_dir).as_posix() for path in paths],
            ["nested/b_match.json", "z_match.json"],
        )

    def test_default_fixture_selection_deduplicates_preserving_order(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            root = Path(temporary_directory)
            fixtures_dir = root / "fixtures"
            fixtures_dir.mkdir()
            cohort_path = root / "cohort.json"
            cohort_path.write_text(
                json.dumps({"fixtures": ["a.json", "b.json", "z.json"]}),
                encoding="utf-8",
            )

            with ExitStack() as patches:
                patches.enter_context(patch.object(baselines, "FIXTURES_DIR", fixtures_dir))
                patches.enter_context(patch.object(baselines, "OCR_IMAGES_COHORT", cohort_path))
                patches.enter_context(patch.object(baselines, "PDF_OCR_FIXTURES", ["z", "a", "z"]))
                paths = baselines.load_ocr_fixture_paths()

        self.assertEqual([path.name for path in paths], ["z.json", "a.json", "b.json"])

    def test_parse_args_requires_pipeline_and_preserves_options(self):
        args = baselines.parse_args(["rapidocr", "--force", "--category", "image-ocr-realgt"])

        self.assertEqual(args.pipeline, "rapidocr")
        self.assertTrue(args.force)
        self.assertEqual(args.category, "image-ocr-realgt")

        with self.assertRaises(SystemExit):
            baselines.parse_args(["--force"])

    def test_resolve_document_path_uses_fixture_directory(self):
        fixture_path = Path("fixtures/nested/example.json")

        document_path = baselines.resolve_document_path(fixture_path, {"document": "../document.png"})

        self.assertEqual(document_path, (Path("fixtures") / "document.png").resolve())

    def test_backend_ocr_language_maps_fixture_codes_per_backend(self):
        cases = [
            ("eng", "en", "en"),
            ("deu", "german", "latin"),
            ("jpn", "japan", "japan"),
            ("jpn_vert", "japan", "japan"),
        ]

        for fixture_language, paddle_language, rapid_language in cases:
            with self.subTest(fixture_language=fixture_language):
                fixture = {"metadata": {"ocr_language": fixture_language}}
                self.assertEqual(baselines.backend_ocr_language("paddleocr-python", fixture), paddle_language)
                self.assertEqual(baselines.backend_ocr_language("rapidocr", fixture), rapid_language)

    def test_backend_ocr_language_defaults_missing_metadata_to_english(self):
        self.assertEqual(baselines.backend_ocr_language("paddleocr-python", {}), "en")
        self.assertEqual(baselines.backend_ocr_language("rapidocr", {"metadata": {}}), "en")

    def test_backend_ocr_language_rejects_unsupported_nonempty_code(self):
        fixture = {"metadata": {"ocr_language": "eng+kor"}}

        with self.assertRaisesRegex(ValueError, "unsupported metadata.ocr_language 'eng\\+kor' for rapidocr"):
            baselines.backend_ocr_language("rapidocr", fixture)

    def test_validate_unique_fixture_names_rejects_output_collisions(self):
        fixture_paths = [Path("first/example.json"), Path("second/example.json")]

        with self.assertRaisesRegex(ValueError, "duplicate output names: example"):
            baselines.validate_unique_fixture_names(fixture_paths)

    def test_document_to_images_loads_all_tiff_frames_as_rgb(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            image_path = Path(temporary_directory) / "multipage.tiff"
            first = Image.new("L", (3, 2), color=10)
            second = Image.new("L", (3, 2), color=20)
            first.save(image_path, save_all=True, append_images=[second])

            frames = baselines.document_to_images(str(image_path))

        self.assertEqual(len(frames), 2)
        self.assertEqual(frames[0].shape, (2, 3, 3))
        self.assertTrue(np.all(frames[1] == 20))

    def test_rapidocr_lines_supports_current_output(self):
        result = SimpleNamespace(txts=(" first ", "", "second"))

        self.assertEqual(baselines.rapidocr_lines(result), ["first", "second"])

    def test_rapidocr_lines_supports_legacy_output(self):
        result = ([[[0, 0], " first ", 0.9], [[0, 0], "", 0.8]], {})

        self.assertEqual(baselines.rapidocr_lines(result), ["first"])

    def test_run_paddleocr_python_passes_fixture_language_to_constructor(self):
        constructor = Mock()
        constructor.return_value.predict.return_value = []
        paddleocr_module = SimpleNamespace(PaddleOCR=constructor)

        with ExitStack() as patches:
            patches.enter_context(patch.dict(sys.modules, {"paddleocr": paddleocr_module}))
            patches.enter_context(patch.object(baselines, "document_to_images", return_value=[np.zeros((1, 1, 3))]))
            baselines.run_paddleocr_python("fixture.png", "german")

        constructor.assert_called_once_with(use_textline_orientation=True, lang="german")

    def test_create_rapidocr_passes_language_to_current_constructor(self):
        constructor = Mock()
        rapidocr_module = SimpleNamespace(RapidOCR=constructor)

        with patch.dict(sys.modules, {"rapidocr": rapidocr_module}):
            baselines.create_rapidocr("japan")

        constructor.assert_called_once_with(params={"Rec.lang_type": "japan"})

    def test_create_rapidocr_rejects_legacy_only_environment(self):
        with (
            patch.dict(
                sys.modules,
                {"rapidocr": None, "rapidocr_onnxruntime": SimpleNamespace(RapidOCR=Mock())},
            ),
            self.assertRaisesRegex(ModuleNotFoundError, "requires rapidocr>=3.0.*reproducible language model"),
        ):
            baselines.create_rapidocr("en")

    def test_main_exits_with_failure_for_unsupported_fixture_language(self):
        with tempfile.TemporaryDirectory() as temporary_directory:
            fixture_dir = Path(temporary_directory)
            fixture_path = fixture_dir / "unsupported.json"
            document_path = fixture_dir / "document.png"
            fixture_path.write_text(
                json.dumps({"document": document_path.name, "metadata": {"ocr_language": "eng+kor"}}),
                encoding="utf-8",
            )
            document_path.touch()
            runner = Mock()

            with ExitStack() as patches:
                patches.enter_context(patch.object(baselines, "load_ocr_fixture_paths", return_value=[fixture_path]))
                patches.enter_context(patch.object(baselines, "run_rapidocr", runner))
                patches.enter_context(patch("traceback.print_exc"))
                with self.assertRaisesRegex(RuntimeError, "1 vendored baseline generation.*eng\\+kor"):
                    baselines.main(["rapidocr", "--force"])

        runner.assert_not_called()


if __name__ == "__main__":
    unittest.main()
