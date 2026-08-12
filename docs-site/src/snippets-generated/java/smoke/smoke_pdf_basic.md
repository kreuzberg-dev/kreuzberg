---
id: fixture_java_smoke_pdf_basic
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"application/pdf\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
