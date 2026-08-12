---
id: fixture_java_config_element_types
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/unit_test_headers.docx\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"result_format\":\"element_based\"}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        for (var element : result.results().get(0).elements()) {
            System.out.println(element.elementType());
            System.out.println(element.content());
        }
    }
}

```
