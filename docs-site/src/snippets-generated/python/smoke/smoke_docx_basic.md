---
id: fixture_python_smoke_docx_basic
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: DOCX with formatted text

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri="https://example.com/docx/fake.docx")
    config = ExtractionConfig()
    await extract(input, config)
    print(result)

asyncio.run(main())

```
