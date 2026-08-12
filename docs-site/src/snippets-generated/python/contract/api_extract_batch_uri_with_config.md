---
id: fixture_python_api_extract_batch_uri_with_config
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(config={"output_format": "markdown"}, kind="uri", uri="https://example.com/pdf/fake_memo.pdf")]
    result = await extract_batch(inputs, None)
    for result in result.results:
        print(result.content)

asyncio.run(main())

```
