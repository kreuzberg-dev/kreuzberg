---
id: fixture_python_extract_batch_bytes_happy
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(bytes=[72, 101, 108, 108, 111, 44, 32, 119, 111, 114, 108, 100, 33], kind="bytes", mime_type="text/plain"), ExtractInput(bytes=Path("test_documents/html/html.html").read_bytes(), kind="bytes", mime_type="text/html")]
    await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
