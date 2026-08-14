---
id: fixture_python_error_unsupported_mime
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with unsupported MIME type

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind
from xberg import XbergError

async def main() -> None:
    try:
        input = ExtractInput(bytes=Path("test_documents/text/plain.txt").read_bytes(), config={}, filename="plain.txt", kind=ExtractInputKind("bytes"), mime_type="application/x-nonexistent")
        config = ExtractionConfig()
        await extract(input, config)
    except XbergError as error:
        print(f"{type(error).__name__}: {error}")

asyncio.run(main())

```
