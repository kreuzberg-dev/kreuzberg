---
id: fixture_python_format_pptx
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="application/vnd.openxmlformats-officedocument.presentationml.presentation", uri="https://example.com/pptx/simple.pptx")
    await extract(input, None)
    print(result)

asyncio.run(main())

```
