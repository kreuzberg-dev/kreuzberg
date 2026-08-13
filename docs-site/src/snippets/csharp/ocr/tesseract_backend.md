```csharp title="C#"
using Xberg;

var config = new ExtractionConfig
{
    Ocr = new OcrConfig
    {
        Backend = "tesseract",
        Language = ["eng", "deu", "fra"],
        TesseractConfig = new TesseractConfig
        {
            Psm = 3
        }
    }
};

var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), config)).Results[0];
Console.WriteLine(result.Content);
```
