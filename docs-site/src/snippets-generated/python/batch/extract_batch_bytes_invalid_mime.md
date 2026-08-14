---
id: fixture_python_extract_batch_bytes_invalid_mime
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(bytes=[72, 101, 108, 108, 111], kind="bytes", mime_type="application/x-nonexistent")]
    _ = await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
