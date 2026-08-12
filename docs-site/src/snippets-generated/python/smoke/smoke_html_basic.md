---
id: fixture_python_smoke_html_basic
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="text/html", uri="https://example.com/html/simple_table.html")
    config = ExtractionConfig()
    result = await extract(input, config)
    for table in result.results[0].tables:
        print(table.rows)

asyncio.run(main())

```
