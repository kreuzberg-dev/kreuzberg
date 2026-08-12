---
id: fixture_java_config_document_structure_with_headings
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/fake.docx\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"include_document_structure\":true}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result.results().get(0).documentStructure());
    }
}

```
