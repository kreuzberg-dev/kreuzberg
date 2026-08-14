---
id: fixture_python_extract_batch_uri_partial_failure
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(kind="uri", uri="text/plain.txt"), ExtractInput(kind="uri", uri="/nonexistent/missing.pdf")]
    _ = await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
