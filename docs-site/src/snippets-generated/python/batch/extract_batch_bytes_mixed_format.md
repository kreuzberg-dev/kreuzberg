---
id: fixture_python_extract_batch_bytes_mixed_format
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(bytes=[80, 68, 70, 32, 112, 108, 97, 99, 101, 104, 111, 108, 100, 101, 114], kind="bytes", mime_type="application/x-unknown")]
    _ = await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
