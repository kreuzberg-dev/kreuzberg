```python title="Python"
import asyncio
from xberg import ExtractInput, extract, ExtractionConfig, OcrConfig, TesseractConfig

async def main() -> None:
    config = ExtractionConfig(
        force_ocr=True,
        ocr=OcrConfig(
            backend="tesseract",
            language="eng",
            tesseract_config=TesseractConfig(psm=3)
        )
    )
    result = await extract(ExtractInput(uri="scanned.pdf"), config)
    print(result.results[0].content)
    print(f"Detected Languages: {result.results[0].detected_languages}")

asyncio.run(main())
```
