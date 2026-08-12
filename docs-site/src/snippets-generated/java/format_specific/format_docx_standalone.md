---
id: fixture_java_format_docx_standalone
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"filename\":\"fake.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/fake.docx\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
