---
id: fixture_python_format_docx_equations
language: python
target: python
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```python title="Python"
import asyncio
from xberg import extract, ExtractInput, ExtractionConfig, ExtractInputKind, OutputFormat

async def main() -> None:
    input = ExtractInput(filename="equations.docx", kind=ExtractInputKind("uri"), mime_type="application/vnd.openxmlformats-officedocument.wordprocessingml.document", uri="https://example.com/docx/equations.docx")
    config = ExtractionConfig(output_format=OutputFormat("markdown"))
    _ = await extract(input, config)

asyncio.run(main())

```
