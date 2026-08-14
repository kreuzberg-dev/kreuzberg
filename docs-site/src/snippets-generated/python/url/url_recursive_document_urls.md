---
id: fixture_python_url_recursive_document_urls
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com")
    config = ExtractionConfig(url={"crawl": {"document_url_depth": 1, "follow_document_urls": True, "respect_robots_txt": False}, "mode": "document"})
    await extract(input, config)
    print(result)

asyncio.run(main())

```
