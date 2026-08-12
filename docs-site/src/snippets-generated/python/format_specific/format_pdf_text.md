---
id: fixture_python_format_pdf_text
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(filename="fake_memo.pdf", kind=ExtractInputKind("uri"), mime_type="application/pdf", uri="https://example.com/pdf/fake_memo.pdf")
    result = await extract(input, None)
    print(result.results[0].metadata)

asyncio.run(main())

```
