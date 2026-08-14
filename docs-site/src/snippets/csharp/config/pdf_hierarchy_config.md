```csharp title="C#"
using Xberg;

// Basic hierarchy configuration with properties
var config = new ExtractionConfig
{
    PdfOptions = new PdfConfig
    {
        ExtractImages = true,
        Hierarchy = new HierarchyConfig
        {
            Enabled = true,
            KClusters = 6,
            IncludeBbox = true
        }
    }
};

var basicResult = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), config)).Results[0];
Console.WriteLine($"Content length: {basicResult.Content.Length}");

// Advanced hierarchy detection with custom parameters
var advancedConfig = new ExtractionConfig
{
    PdfOptions = new PdfConfig
    {
        ExtractImages = true,
        Hierarchy = new HierarchyConfig
        {
            Enabled = true,
            KClusters = 12,           // More clusters for detailed hierarchy
            IncludeBbox = true        // Include bounding box coordinates
        }
    }
};

var advancedResult = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("complex_document.pdf"), advancedConfig)).Results[0];
Console.WriteLine($"Advanced hierarchy detection completed: {advancedResult.Content.Length} chars");

// Minimal configuration with only enabled flag
var minimalConfig = new ExtractionConfig
{
    PdfOptions = new PdfConfig
    {
        Hierarchy = new HierarchyConfig
        {
            Enabled = true,
            // Other properties use defaults:
            // KClusters = 6
            // IncludeBbox = true
        }
    }
};

var minimalResult = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), minimalConfig)).Results[0];
Console.WriteLine("Extraction with default hierarchy settings complete");

// Disabling hierarchy detection
var noHierarchyConfig = new ExtractionConfig
{
    PdfOptions = new PdfConfig
    {
        Hierarchy = new HierarchyConfig
        {
            Enabled = false
        }
    }
};

var noHierarchyResult = (await XbergConverter.ExtractAsync(ExtractInput.FromUri("document.pdf"), noHierarchyConfig)).Results[0];
Console.WriteLine("Extraction without hierarchy detection complete");
```
