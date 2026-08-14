---
id: fixture_python_extract_batch_uri_all_missing
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(kind="uri", uri="/nonexistent/a.pdf"), ExtractInput(kind="uri", uri="/nonexistent/b.txt")]
    _ = await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
