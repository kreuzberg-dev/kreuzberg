---
id: fixture_java_format_pdf_text
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"filename\":\"fake_memo.pdf\",\"kind\":\"uri\",\"mime_type\":\"application/pdf\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result.results().get(0).metadata());
    }
}

```
