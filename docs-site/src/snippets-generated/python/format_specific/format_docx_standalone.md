---
id: fixture_python_format_docx_standalone
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(filename="fake.docx", kind=ExtractInputKind("uri"), mime_type="application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri="https://example.com/docx/fake.docx")
    _ = await extract(input, None)
    print(result)

asyncio.run(main())

```
