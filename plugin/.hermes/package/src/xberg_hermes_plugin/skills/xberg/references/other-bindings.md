<!--
AI-RULEZ :: GENERATED FILE — DO NOT EDIT
Content-Hash: blake3:dacb4d7def21681b8b237603d2c7b89beaf18b8ee4110175b6ea5593022f3229
Source-Hash: blake3:e627b65881d01f835095c09b8ff3f8e15d61895139679b904925a8aa19500a12
Schema-Version: v1
-->

# Language Bindings Reference

Xberg provides bindings for Python, TypeScript/Node.js, Ruby, PHP, Go, Java, C#, Elixir, WebAssembly, Dart, Kotlin Android, Swift, Zig, and C. Platform and packaging support varies by binding.

Every binding shares the same shape: build an **`ExtractInput`** (from a URI or bytes), pass it with an **`ExtractionConfig`** to `extract` (or `extract_batch`), and read per-document data from the result **envelope's `results` array** (index `[0]` for a single input). WASM follows the same shape — its `extract` returns a `WasmExtractionResult` envelope with a `results` array, not the document directly.

## Go

**Installation:**

```bash
go get github.com/xberg-io/xberg/packages/go
```

**Basic Extraction:**

```go
package main

import (
    "fmt"
    "log"

    "github.com/xberg-io/xberg/packages/go"
)

func main() {
    input := xberg.ExtractInputFromURI("document.pdf")
    result, err := xberg.Extract(*input, xberg.ExtractionConfig{})
    if err != nil {
        log.Fatalf("extract failed: %v", err)
    }
    fmt.Println(result.Results[0].Content)
}
```

**With OCR** (config fields are pointers):

```go
ocrConfig := &xberg.OcrConfig{Backend: "tesseract", Language: []string{"eng"}}
config := xberg.ExtractionConfig{Ocr: ocrConfig}

input := xberg.ExtractInputFromURI("scanned.pdf")
result, err := xberg.Extract(*input, config)
```

See the [Go binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/go) for the complete API reference.

## Ruby

**Installation:**

```bash
gem install xberg
```

Or in your Gemfile:

```ruby
gem 'xberg'
```

**Basic Extraction:**

```ruby
require 'xberg'

input = Xberg::ExtractInput.new(uri: 'document.pdf')
config = Xberg::ExtractionConfig.new
result = Xberg.extract(input, config)
puts result.results.first.content
```

**With OCR:**

```ruby
config = Xberg::ExtractionConfig.new(
  ocr: Xberg::OcrConfig.new(
    backend: 'tesseract',
    language: ['eng', 'fra'],
    tesseract_config: Xberg::TesseractConfig.new(psm: 3)
  )
)
result = Xberg.extract(Xberg::ExtractInput.new(uri: 'scanned.pdf'), config)
puts result.results.first.content
```

See the [Ruby binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/ruby) for the complete API reference.

## Java

**Installation:** add to your Maven `pom.xml` (the binding version tracks the core release, currently `1.1.0`):

```xml
<dependency>
    <groupId>io.xberg</groupId>
    <artifactId>xberg</artifactId>
    <version>1.1.0</version>
</dependency>
```

**Basic Extraction:**

```java
import io.xberg.Xberg;
import io.xberg.ExtractInput;
import io.xberg.ExtractInputKind;
import io.xberg.ExtractionConfig;
import io.xberg.ExtractionResult;
import io.xberg.ExtractedDocument;

public class Example {
    public static void main(String[] args) throws Exception {
        ExtractionResult output = Xberg.extract(
            ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("document.pdf").build(),
            ExtractionConfig.builder().build()
        );
        ExtractedDocument result = output.results().get(0);
        System.out.println(result.content());
    }
}
```

**With OCR:**

```java
import io.xberg.OcrConfig;
import java.util.List;

ExtractionConfig config = ExtractionConfig.builder()
    .withOcr(OcrConfig.builder().withBackend("tesseract").withLanguage(List.of("eng")).build())
    .build();
ExtractionResult output = Xberg.extract(
    ExtractInput.builder().withKind(ExtractInputKind.Uri).withUri("scanned.pdf").build(),
    config
);
```

See the [Java binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/java) for the complete API reference.

## C#

**Installation:**

```bash
dotnet add package XbergIo.Xberg
```

**Basic Extraction** (async):

```csharp
using Xberg;

var result = (await XbergConverter.ExtractAsync(
    ExtractInput.FromUri("document.pdf"),
    new ExtractionConfig())).Results[0];
Console.WriteLine(result.Content);
```

**With OCR:**

```csharp
using Xberg;

var config = new ExtractionConfig
{
    Ocr = new OcrConfig { Backend = "tesseract", Language = ["eng"] }
};

var result = (await XbergConverter.ExtractAsync(
    ExtractInput.FromUri("scanned.pdf"), config)).Results[0];
Console.WriteLine(result.Content);
```

See the [C# binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/csharp) for the complete API reference.

## PHP

**Installation:**

```bash
composer require xberg-io/xberg
```

**Basic Extraction:**

```php
<?php
declare(strict_types=1);

require 'vendor/autoload.php';

use Xberg\XbergApi;
use Xberg\ExtractionConfig;
use Xberg\ExtractInput;

$output = XbergApi::extract(ExtractInput::fromUri('document.pdf'), ExtractionConfig::default());
$result = $output->results[0];
echo $result->content;
```

**From bytes:**

```php
$output = XbergApi::extract(ExtractInput::fromBytes($fileData, $mimeType), ExtractionConfig::default());
$result = $output->results[0];
```

**With OCR:**

```php
use Xberg\OcrConfig;

$ocrConfig = new OcrConfig(
    enabled: true,
    backend: 'tesseract',
    language: ['eng'],
    autoRotate: true,
    vlmFallback: false,
);

$config = new ExtractionConfig(forceOcr: true, ocr: $ocrConfig);

$output = XbergApi::extract(ExtractInput::fromUri('scanned.pdf'), $config);
$result = $output->results[0];
echo $result->content;
```

See the [PHP binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/php) for the complete API reference.

## Elixir

**Installation:** add to your `mix.exs` dependencies (the binding version tracks the core release, currently `1.1.0`):

```elixir
def deps do
  [
    {:xberg, "~> 1.1.0"}
  ]
end
```

**Basic Extraction** (returns `{:ok, output}` / `{:error, reason}`):

```elixir
case Xberg.extract(input: %Xberg.ExtractInput{kind: :uri, uri: "document.pdf"}, config: nil) do
  {:ok, output} ->
    result = List.first(output.results)
    IO.puts(result.content)

  {:error, reason} ->
    IO.puts("Error: #{reason}")
end
```

**With OCR** (config may be a struct or a JSON string):

```elixir
config = ~s({"ocr": {"backend": "tesseract", "language": "eng"}})

{:ok, output} =
  Xberg.extract(input: %Xberg.ExtractInput{kind: :uri, uri: "scanned.pdf"}, config: config)

result = List.first(output.results)
IO.puts(result.content)
```

See the [Elixir binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/elixir) for the complete API reference.

## WebAssembly (WASM)

**Installation:**

```bash
npm install @xberg-io/xberg-wasm
```

**Basic Extraction:** the WASM build is bytes-based and must be initialized once with `initWasm()`. Like the native bindings, it returns a `WasmExtractionResult` envelope with a `results` array.

```typescript
import { initWasm, extract } from "@xberg-io/xberg-wasm";

await initWasm();

const response = await fetch("document.pdf");
const data = new Uint8Array(await response.arrayBuffer());

const result = await extract({ kind: "bytes", bytes: data, mimeType: "application/pdf" }, undefined);
console.log(result.results[0].content);
```

**With OCR:**

```typescript
const config = { force_ocr: true, ocr: { backend: "tesseract", language: ["eng"] } };
const result = await extract({ kind: "bytes", bytes: data, mimeType: "application/pdf" }, config);
console.log(result.results[0].content);
```

Supports browsers, Deno, and Cloudflare Workers.

See the [WASM binding documentation](https://github.com/xberg-io/xberg/tree/main/packages/typescript) for the complete API reference.

## Docker

**Installation:** pull the official image from GitHub Container Registry:

```bash
docker pull ghcr.io/xberg-io/xberg
```

**API Server Mode:**

```bash
docker run -p 8000:8000 ghcr.io/xberg-io/xberg serve --host 0.0.0.0
```

**CLI Mode:**

```bash
docker run -v $(pwd):/data ghcr.io/xberg-io/xberg extract /data/document.pdf
```

**MCP Server Mode:**

```bash
docker run -i ghcr.io/xberg-io/xberg mcp
```

See the [Docker guide](https://docs.xberg.io/guides/docker/) for deployment details.

## Other Bindings

Bindings also ship for Dart/Flutter, Swift, Kotlin Android, Zig, and C FFI. Python and TypeScript/Node.js have dedicated references in this skill. See the per-language directories under [`packages/`](https://github.com/xberg-io/xberg/tree/main/packages) for exact installation and API details.

## Platform Support

Precompiled target support varies by binding. Refer to the main [README](https://github.com/xberg-io/xberg) for the current platform matrix.
