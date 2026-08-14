```csharp title="C#"
using System;
using System.Diagnostics;
using System.IO;
using System.Threading.Tasks;

var processInfo = new ProcessStartInfo
{
    FileName = "xberg",
    Arguments = "mcp",
    UseShellExecute = false,
    RedirectStandardInput = true,
    RedirectStandardOutput = true,
    RedirectStandardError = true
};

var process = Process.Start(processInfo)
    ?? throw new InvalidOperationException("Failed to start the xberg MCP server process");

var clientInput = process.StandardInput;
var clientOutput = process.StandardOutput;

// Initialize session by sending initialize request
var initRequest = new
{
    jsonrpc = "2.0",
    id = 1,
    method = "initialize",
    parameters = new { }
};

await clientInput.WriteLineAsync(System.Text.Json.JsonSerializer.Serialize(initRequest));
await clientInput.FlushAsync();

var initResponse = await clientOutput.ReadLineAsync();
Console.WriteLine($"Init response: {initResponse}");

// List available tools
var listRequest = new
{
    jsonrpc = "2.0",
    id = 2,
    method = "tools/list"
};

await clientInput.WriteLineAsync(System.Text.Json.JsonSerializer.Serialize(listRequest));
await clientInput.FlushAsync();

var listResponse = await clientOutput.ReadLineAsync();
Console.WriteLine($"Available tools: {listResponse}");

process?.WaitForExit();
```
