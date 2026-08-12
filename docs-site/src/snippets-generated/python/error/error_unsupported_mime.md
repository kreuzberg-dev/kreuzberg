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

async def main() -> None:
    try:
        input = ExtractInput(bytes=Path("test_documents/text/plain.txt").read_bytes(), config={}, filename="plain.txt", kind=ExtractInputKind("bytes"), mime_type="application/x-nonexistent")
        config = ExtractionConfig()
        _ = await extract(input, config)
    except Exception as error:
        print(f"Call failed as expected: {error}")
    else:
        raise AssertionError("expected call to fail")

asyncio.run(main())

```
