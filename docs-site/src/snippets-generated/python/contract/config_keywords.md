---
id: fixture_python_config_keywords
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig(keywords={"algorithm": "yake", "max_keywords": 10})
    result = await extract(input, config)
    for keyword in result.results[0].keywords:
        print(keyword.text)
        print(keyword.score)

asyncio.run(main())

```
