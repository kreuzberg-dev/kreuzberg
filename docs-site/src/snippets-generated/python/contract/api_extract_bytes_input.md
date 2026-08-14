---
id: fixture_python_api_extract_bytes_input
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Tests bytes input extraction API (extract)

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract, ExtractInput, ExtractInputKind, ExtractionConfig

async def main() -> None:
    input = ExtractInput(bytes=Path("test_documents/pdf/fake_memo.pdf").read_bytes(), filename="fake_memo.pdf", kind=ExtractInputKind("bytes"))
    await extract(input, None)
    print(result)

asyncio.run(main())

```
