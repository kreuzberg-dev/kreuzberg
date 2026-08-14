```csharp title="C#"
using Xberg;

var config = new ExtractionConfig
{
    ForceOcr = true,
    Ocr = new OcrConfig
    {
        Backend = "tesseract",
        Language = new List<string> { "eng" },
    },
};

var result = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("scanned.pdf"), config)).Results[0];
Console.WriteLine(result.Content);
Console.WriteLine(result.DetectedLanguages);
```
