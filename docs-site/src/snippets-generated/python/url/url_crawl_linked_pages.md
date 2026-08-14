---
id: fixture_python_url_crawl_linked_pages
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com")
    config = ExtractionConfig(url={"crawl": {"max_depth": 1, "max_pages": 4, "respect_robots_txt": False}, "mode": "crawl"})
    await extract(input, config)
    print(result)

asyncio.run(main())

```
