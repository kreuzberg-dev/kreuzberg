---
id: fixture_python_config_pages
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig(pages={"extract_pages": True, "insert_page_markers": True})
    await extract(input, config)
    print(result)

asyncio.run(main())

```
