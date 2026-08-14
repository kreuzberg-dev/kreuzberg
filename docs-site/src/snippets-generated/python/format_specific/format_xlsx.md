---
id: fixture_python_format_xlsx
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", uri="https://example.com/xlsx/stanley_cups.xlsx")
    _ = await extract(input, None)
    print(result)

asyncio.run(main())

```
