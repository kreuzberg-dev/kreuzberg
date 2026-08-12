---
id: fixture_python_ocr_image_png
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(bytes=Path("test_documents/images/test_hello_world.png").read_bytes(), config={}, filename="test_hello_world.png", kind=ExtractInputKind("bytes"), mime_type="image/png")
    config = ExtractionConfig()
    result = await extract(input, config)
    print(result.results[0].content)

asyncio.run(main())

```
