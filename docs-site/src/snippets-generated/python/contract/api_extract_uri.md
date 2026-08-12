---
id: fixture_python_api_extract_uri
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    result = await extract(input, None)
    print(result.results[0].content)

asyncio.run(main())

```
