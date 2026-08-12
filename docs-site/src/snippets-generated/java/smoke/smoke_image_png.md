---
id: fixture_java_smoke_image_png
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/images/sample.png\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"disable_ocr\":true}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
