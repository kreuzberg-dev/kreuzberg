---
id: fixture_python_extract_batch_uri_not_found
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(kind="uri", uri="/nonexistent/a.pdf")]
    await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
