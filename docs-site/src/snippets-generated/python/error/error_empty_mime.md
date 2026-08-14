---
id: fixture_python_error_empty_mime
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind
from xberg import XbergError

async def main() -> None:
    try:
        input = ExtractInput(bytes=Path("test_documents/text/plain.txt").read_bytes(), config={}, filename="plain.txt", kind=ExtractInputKind("bytes"), mime_type="")
        config = ExtractionConfig()
        await extract(input, config)
    except XbergError as error:
        print(f"{type(error).__name__}: {error}")

asyncio.run(main())

```
