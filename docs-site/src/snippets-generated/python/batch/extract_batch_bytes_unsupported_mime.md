---
id: fixture_python_extract_batch_bytes_unsupported_mime
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```python title="Python"
import asyncio
from xberg import extract_batch, ExtractInput, ExtractionConfig

async def main() -> None:
    inputs = [ExtractInput(bytes=[100, 97, 116, 97], kind="bytes", mime_type="application/x-unknown")]
    _ = await extract_batch(inputs, None)
    print(result)

asyncio.run(main())

```
