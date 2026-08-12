```python title="Python"
import asyncio

from xberg import ExtractInput, extract


async def main() -> None:
    output = await extract(ExtractInput(kind="uri", uri="document.pdf"))

    print(output.results[0].content)
    print(f"Results: {output.summary.results}")


asyncio.run(main())
```
