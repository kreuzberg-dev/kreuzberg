---
id: fixture_python_smoke_image_png
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/images/sample.png")
    config = ExtractionConfig(disable_ocr=True)
    _ = await extract(input, config)
    print(result)

asyncio.run(main())

```
