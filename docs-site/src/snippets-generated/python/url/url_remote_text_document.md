---
id: fixture_python_url_remote_text_document
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

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
