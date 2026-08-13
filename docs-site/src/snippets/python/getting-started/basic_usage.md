```python title="Python"
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig

async def main() -> None:
    config = ExtractionConfig(
        use_cache=True,
        enable_quality_processing=True
    )
    result = await extract(ExtractInput(uri="document.pdf"), config)
    print(result.results[0].content)

asyncio.run(main())
```
