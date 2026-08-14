```csharp title="C#"
using Xberg;
using System;
using System.Collections.Concurrent;
using System.Text.Json;

var processor = new StatefulPostProcessor();
PostProcessorRegistry.RegisterPostProcessor(processor);
Console.WriteLine("Post-processor registered");

public class StatefulPostProcessor : IPostProcessor
{
    private readonly object _lock = new();
    private int _callCount = 0;
    private readonly ConcurrentDictionary<string, string> _cache = new();

    public string Name => "stateful-plugin";
    public string Version => "1.0.0";
    public int Priority => 50;
    public ProcessingStage ProcessingStage => ProcessingStage.Middle;

    public void Initialize() { }
    public void Shutdown() { }

    public bool ShouldProcess(ExtractedDocument result, ExtractionConfig config) => true;
    public ulong EstimatedDurationMs(ExtractedDocument result) => 5;

    public void Process(ExtractedDocument result, ExtractionConfig config)
    {
        lock (_lock)
        {
            _callCount++;
            _cache["last_mime"] = result.MimeType;
        }
        result.Metadata.Additional["call_count"] = JsonSerializer.SerializeToElement(_callCount);
    }
}
```
