---
id: fixture_python_smoke_json_basic
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="application/json", uri="https://example.com/json/simple.json")
    config = ExtractionConfig()
    await extract(input, config)
    print(result)

asyncio.run(main())

```
