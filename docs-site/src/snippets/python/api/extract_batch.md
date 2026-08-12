```python title="Python"
import asyncio

from xberg import ExtractInput, extract_batch


async def main() -> None:
    inputs = [
        ExtractInput(kind="uri", uri="document.pdf"),
        ExtractInput(
            kind="bytes",
            bytes=b"Hello from memory",
            mime_type="text/plain",
            filename="note.txt",
        ),
    ]

    output = await extract_batch(inputs)

    for result in output.results:
        print(result.results[0].content[:200])


asyncio.run(main())
```
