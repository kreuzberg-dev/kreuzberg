---
id: fixture_python_url_html_page_extract
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com")
    config = ExtractionConfig(url={"mode": "document"})
    _ = await extract(input, config)
    print(result)

asyncio.run(main())

```
