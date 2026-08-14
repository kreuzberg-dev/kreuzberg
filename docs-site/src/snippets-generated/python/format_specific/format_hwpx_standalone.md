---
id: fixture_python_format_hwpx_standalone
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(filename="simple.hwpx", kind=ExtractInputKind("uri"), mime_type="application/haansofthwpx", uri="https://example.com/hwpx/simple.hwpx")
    await extract(input, None)
    print(result)

asyncio.run(main())

```
