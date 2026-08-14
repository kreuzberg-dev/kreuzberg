```csharp title="C#"
using Xberg;

var config = new ExtractionConfig
{
    TokenReduction = new TokenReductionOptions
    {
        Mode = "moderate",              // "off", "light", "moderate", "aggressive", or "maximum"
        PreserveImportantWords = true
    }
};
```
