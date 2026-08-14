---
id: fixture_python_url_batch_mixed_inputs
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(kind="uri", uri="https://example.com"), ExtractInput(bytes=[66, 97, 116, 99, 104, 32, 98, 121, 116, 101, 115, 32, 99, 111, 110, 116, 101, 110, 116], filename="inline.txt", kind="bytes", mime_type="text/plain")]
    config = ExtractionConfig(url={"mode": "document"})
    await extract_batch(inputs, config)
    print(result)

asyncio.run(main())

```
