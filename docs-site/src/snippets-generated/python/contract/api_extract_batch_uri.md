---
id: fixture_python_api_extract_batch_uri
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction API (extract_batch)

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(kind="uri", uri="https://example.com/pdf/fake_memo.pdf")]
    await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
