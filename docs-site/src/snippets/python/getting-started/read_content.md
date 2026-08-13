```python title="Python"
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig

async def main() -> None:
    result = await extract(ExtractInput(uri="document.pdf"), ExtractionConfig())
    content: str = result.results[0].content
    tables: int = len(result.results[0].tables)
    format_type: str | None = result.results[0].metadata.format.format_type if result.results[0].metadata.format else None

    print(f"Content length: {len(content)} characters")
    print(f"Tables found: {tables}")
    print(f"Format: {format_type}")

asyncio.run(main())
```
