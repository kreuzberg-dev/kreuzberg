---
id: fixture_python_smoke_txt_basic
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="text/plain", uri="https://example.com/text/report.txt")
    config = ExtractionConfig()
    await extract(input, config)
    print(result)

asyncio.run(main())

```
