---
id: fixture_python_error_empty_bytes
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind

async def main() -> None:
    input = ExtractInput(bytes=[], config={}, filename="empty.txt", kind=ExtractInputKind("bytes"), mime_type="text/plain")
    config = ExtractionConfig()
    await extract(input, config)
    print(result)

asyncio.run(main())

```
