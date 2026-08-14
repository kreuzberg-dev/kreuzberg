---
id: fixture_python_code_shebang_detection
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), mime_type="text/x-source-code", uri="https://example.com/code/script.sh")
    _ = await extract(input, None)
    print(result)

asyncio.run(main())

```
