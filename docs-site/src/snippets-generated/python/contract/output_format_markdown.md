---
id: fixture_python_output_format_markdown
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

Tests Markdown output format

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind, OutputFormat

async def main() -> None:
    input = ExtractInput(kind=ExtractInputKind("uri"), uri="https://example.com/pdf/fake_memo.pdf")
    config = ExtractionConfig(output_format=OutputFormat("markdown"))
    _ = await extract(input, config)
    print(result)

asyncio.run(main())

```
