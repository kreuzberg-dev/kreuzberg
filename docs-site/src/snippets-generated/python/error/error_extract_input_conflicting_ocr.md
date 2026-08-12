---
id: fixture_python_error_extract_input_conflicting_ocr
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract force+disable OCR

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    try:
        input = ExtractInput(bytes=Path("test_documents/text/fake_text.txt").read_bytes(), config={"disable_ocr": True, "force_ocr": True}, filename="fake_text.txt", kind=ExtractInputKind("bytes"), mime_type="text/plain")
        config = ExtractionConfig(disable_ocr=True, force_ocr=True)
        _ = await extract(input, config)
    except Exception as error:
        print(f"Call failed as expected: {error}")
    else:
        raise AssertionError("expected call to fail")

asyncio.run(main())

```
