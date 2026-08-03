"""Generate vendored OCR baselines from PaddleOCR Python and RapidOCR.

Usage:
    uv run --locked --isolated --python 3.12 --only-group bench-rapidocr \
        python tools/benchmark-harness/scripts/generate_vendored_baselines.py rapidocr
    uv run --locked --isolated --python 3.12 --only-group bench-paddleocr-python \
        python tools/benchmark-harness/scripts/generate_vendored_baselines.py paddleocr-python
"""

import argparse
import json
import os
import time
from pathlib import Path

import numpy as np

FIXTURES_DIR = Path(__file__).resolve().parent.parent / "fixtures"
VENDORED_DIR = Path(__file__).resolve().parent.parent / "vendored"
COHORTS_DIR = Path(__file__).resolve().parent.parent / "cohorts"
OCR_IMAGES_COHORT = COHORTS_DIR / "ocr-images-fast.json"

PDF_OCR_FIXTURES = [
    "pdf_image_only_german",
    "pdf_non_searchable",
    "pdf_ocr_rotated_270",
    "pdf_ocr_rotated_90",
    "pdf_ocr_rotated",
    "pdf_ocr_test",
    "pdf_scanned_ocr",
]

DEFAULT_FIXTURE_OCR_LANGUAGE = "eng"
BACKEND_LANGUAGES = {
    "paddleocr-python": {
        "eng": "en",
        "deu": "german",
        "jpn": "japan",
        "jpn_vert": "japan",
    },
    "rapidocr": {
        "eng": "en",
        # RapidOCR provides German recognition through its Latin model. ~keep
        "deu": "latin",
        "jpn": "japan",
        "jpn_vert": "japan",
    },
}


def deduplicate_fixture_paths(fixture_paths: list[Path]) -> list[Path]:
    """Remove duplicate paths while preserving their first-seen order."""
    return list(dict.fromkeys(fixture_paths))


def load_ocr_fixture_paths(category: str | None = None) -> list[Path]:
    """Load the default OCR cohort or fixtures in an exact metadata category."""
    if category is not None:
        fixture_paths = []
        candidates = sorted(
            FIXTURES_DIR.rglob("*.json"),
            key=lambda path: path.relative_to(FIXTURES_DIR).as_posix(),
        )
        for fixture_path in candidates:
            fixture = json.loads(fixture_path.read_text(encoding="utf-8"))
            if fixture.get("metadata", {}).get("category") == category:
                fixture_paths.append(fixture_path)
        return deduplicate_fixture_paths(fixture_paths)

    fixture_paths = [FIXTURES_DIR / f"{name}.json" for name in PDF_OCR_FIXTURES]
    cohort = json.loads(OCR_IMAGES_COHORT.read_text(encoding="utf-8"))
    fixture_paths.extend(FIXTURES_DIR / fixture for fixture in cohort["fixtures"])
    return deduplicate_fixture_paths(fixture_paths)


def validate_unique_fixture_names(fixture_paths: list[Path]) -> None:
    """Reject fixture selections that would overwrite stem-keyed vendored outputs."""
    paths_by_name: dict[str, list[Path]] = {}
    for fixture_path in fixture_paths:
        paths_by_name.setdefault(fixture_path.stem, []).append(fixture_path)

    duplicate_names = sorted(name for name, paths in paths_by_name.items() if len(paths) > 1)
    if duplicate_names:
        names = ", ".join(duplicate_names)
        raise ValueError(f"fixture selection contains duplicate output names: {names}")


def resolve_document_path(fixture_path: Path, fixture: dict[str, object]) -> Path:
    """Resolve a fixture document relative to its descriptor."""
    document = fixture.get("document")
    if not isinstance(document, str) or not document:
        raise ValueError(f"fixture has no document path: {fixture_path}")
    return (fixture_path.parent / document).resolve()


def fixture_ocr_language(fixture: dict[str, object]) -> str:
    """Return the fixture's Tesseract language code, defaulting missing metadata to English."""
    metadata = fixture.get("metadata")
    if not isinstance(metadata, dict):
        return DEFAULT_FIXTURE_OCR_LANGUAGE

    language = metadata.get("ocr_language")
    # Older OCR fixtures omitted language metadata and were authored in English. ~keep
    if language is None or language == "":
        return DEFAULT_FIXTURE_OCR_LANGUAGE
    if not isinstance(language, str):
        raise ValueError(f"fixture metadata.ocr_language must be a string, got {type(language).__name__}")
    return language


def backend_ocr_language(pipeline_name: str, fixture: dict[str, object]) -> str:
    """Translate a fixture's Tesseract language code to a backend model language."""
    fixture_language = fixture_ocr_language(fixture)
    language_map = BACKEND_LANGUAGES[pipeline_name]
    try:
        return language_map[fixture_language]
    except KeyError as error:
        supported = ", ".join(sorted(language_map))
        raise ValueError(
            f"unsupported metadata.ocr_language {fixture_language!r} for {pipeline_name}; "
            f"supported Tesseract codes: {supported}"
        ) from error


def pdf_to_images(pdf_path: str, dpi: int = 300) -> list[np.ndarray]:
    """Convert PDF pages to numpy arrays (RGB, HWC)."""
    import io

    import fitz
    from PIL import Image

    doc = fitz.open(pdf_path)
    images = []
    for page in doc:
        mat = fitz.Matrix(dpi / 72, dpi / 72)
        pix = page.get_pixmap(matrix=mat)
        img = Image.open(io.BytesIO(pix.tobytes("png"))).convert("RGB")
        images.append(np.array(img))
    doc.close()
    return images


def document_to_images(document_path: str) -> list[np.ndarray]:
    """Load PDF pages or raster image frames as RGB numpy arrays."""
    if Path(document_path).suffix.lower() == ".pdf":
        return pdf_to_images(document_path)

    from PIL import Image, ImageSequence

    with Image.open(document_path) as image:
        return [np.array(frame.convert("RGB")) for frame in ImageSequence.Iterator(image)]


def lines_to_markdown(lines: list[str]) -> str:
    """Each OCR text line becomes a markdown paragraph."""
    paragraphs = [line.strip() for line in lines if line.strip()]
    return "\n\n".join(paragraphs) + "\n" if paragraphs else ""


def run_paddleocr_python(document_path: str, language: str) -> tuple[str, float]:
    """Run PaddleOCR Python v3.4+ using the predict() API."""
    os.environ["PADDLE_PDX_DISABLE_MODEL_SOURCE_CHECK"] = "True"
    from paddleocr import PaddleOCR

    ocr = PaddleOCR(use_textline_orientation=True, lang=language)
    images = document_to_images(document_path)

    start = time.monotonic()
    all_lines: list[str] = []
    for img in images:
        for result in ocr.predict(img):
            rec_texts = result.get("rec_text", [])
            if isinstance(rec_texts, (list, tuple)):
                for t in rec_texts:
                    text = str(t).strip()
                    if text:
                        all_lines.append(text)
    elapsed_ms = (time.monotonic() - start) * 1000

    return lines_to_markdown(all_lines), elapsed_ms


def rapidocr_lines(result: object) -> list[str]:
    """Return recognized text from current or legacy RapidOCR output."""
    texts = getattr(result, "txts", None)
    if texts is not None:
        return [str(text).strip() for text in texts if str(text).strip()]

    legacy_result = result[0] if isinstance(result, tuple) else result
    if not legacy_result:
        return []
    return [str(line[1]).strip() for line in legacy_result if line and len(line) >= 2 and str(line[1]).strip()]


def create_rapidocr(language: str):
    """Create a language-specific RapidOCR engine with reproducible model selection."""
    try:
        from rapidocr import RapidOCR
    except ModuleNotFoundError as error:
        if error.name != "rapidocr":
            raise
        raise ModuleNotFoundError(
            "RapidOCR baseline generation requires rapidocr>=3.0; the legacy "
            "rapidocr_onnxruntime package does not provide reproducible language model selection"
        ) from error

    return RapidOCR(params={"Rec.lang_type": language})


def run_rapidocr(document_path: str, language: str) -> tuple[str, float]:
    """Run RapidOCR."""
    ocr = create_rapidocr(language)
    images = document_to_images(document_path)

    start = time.monotonic()
    all_lines: list[str] = []
    for img in images:
        all_lines.extend(rapidocr_lines(ocr(img)))
    elapsed_ms = (time.monotonic() - start) * 1000

    return lines_to_markdown(all_lines), elapsed_ms


def save_vendored(pipeline_name: str, fixture_name: str, md: str, time_ms: float):
    md_dir = VENDORED_DIR / pipeline_name / "md"
    timing_dir = VENDORED_DIR / pipeline_name / "timing"
    md_dir.mkdir(parents=True, exist_ok=True)
    timing_dir.mkdir(parents=True, exist_ok=True)
    (md_dir / f"{fixture_name}.md").write_text(md)
    (timing_dir / f"{fixture_name}.ms").write_text(f"{time_ms:.1f}\n")


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    """Parse the baseline generator command line."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pipeline", choices=("paddleocr-python", "rapidocr"))
    parser.add_argument("--force", action="store_true", help="replace existing non-empty outputs")
    parser.add_argument("--category", help="select fixtures with this exact metadata.category")
    return parser.parse_args(argv)


def main(argv: list[str] | None = None) -> None:
    args = parse_args(argv)
    pipelines = {
        "paddleocr-python": run_paddleocr_python,
        "rapidocr": run_rapidocr,
    }

    pipelines = {args.pipeline: pipelines[args.pipeline]}

    fixture_paths = load_ocr_fixture_paths(args.category)
    validate_unique_fixture_names(fixture_paths)
    failures: list[str] = []
    for fixture_path in fixture_paths:
        fixture_name = fixture_path.stem
        if not fixture_path.exists():
            print(f"  SKIP {fixture_name}: fixture not found")
            continue

        with open(fixture_path) as f:
            fixture = json.load(f)

        doc_path = resolve_document_path(fixture_path, fixture)
        if not doc_path.exists():
            print(f"  SKIP {fixture_name}: document not found")
            continue

        for pipeline_name, run_fn in pipelines.items():
            existing = VENDORED_DIR / pipeline_name / "md" / f"{fixture_name}.md"
            if not args.force and existing.exists() and existing.stat().st_size > 0:
                print(f"  CACHED {pipeline_name}/{fixture_name}")
                continue

            print(f"  RUN {pipeline_name}/{fixture_name} ...", end="", flush=True)
            try:
                language = backend_ocr_language(pipeline_name, fixture)
                md, time_ms = run_fn(str(doc_path), language)
                save_vendored(pipeline_name, fixture_name, md, time_ms)
                print(f" {time_ms:.0f}ms, {len(md)} chars")
            except Exception as e:
                print(f" ERROR: {e}")
                failures.append(f"{pipeline_name}/{fixture_name}: {type(e).__name__}: {e}")
                import traceback

                traceback.print_exc()

    if failures:
        raise RuntimeError(f"{len(failures)} vendored baseline generation(s) failed: {'; '.join(failures)}")


if __name__ == "__main__":
    main()
