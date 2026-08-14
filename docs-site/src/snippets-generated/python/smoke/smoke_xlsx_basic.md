---
id: fixture_python_smoke_xlsx_basic
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", uri="https://example.com/xlsx/stanley_cups.xlsx")
    config = ExtractionConfig()
    await extract(input, config)
    print(result)

asyncio.run(main())

```
