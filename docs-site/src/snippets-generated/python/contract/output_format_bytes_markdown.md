---
id: fixture_python_output_format_bytes_markdown
language: python
target: python
level: typecheck
requires: []
side_effect: safe
---

Tests markdown output format via bytes extraction API

```python title="Python"
import asyncio
from pathlib import Path
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind, OutputFormat

async def main() -> None:
    input = ExtractInput(bytes=Path("test_documents/pdf/fake_memo.pdf").read_bytes(), config={"output_format": "markdown"}, filename="fake_memo.pdf", kind=ExtractInputKind("bytes"), mime_type="application/pdf")
    config = ExtractionConfig(output_format=OutputFormat("markdown"))
    await extract(input, config)
    print(result)

asyncio.run(main())

```
