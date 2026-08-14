---
id: fixture_python_extract_batch_empty_inputs
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = []
    await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
