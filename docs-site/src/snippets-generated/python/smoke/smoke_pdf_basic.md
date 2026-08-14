---
id: fixture_python_smoke_pdf_basic
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="application/pdf", uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig()
    _ = await extract(input, config)
    print(result)

asyncio.run(main())

```
