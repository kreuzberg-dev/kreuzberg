---
id: fixture_java_config_chunking_prepend_heading_context
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"document.md\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"chunking\":{\"chunker_type\":\"markdown\",\"max_characters\":500,\"overlap\":50,\"prepend_heading_context\":true}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        for (var chunk : result.results().get(0).chunks()) {
            System.out.println(chunk.content());
            System.out.println(chunk.metadata());
        }
    }
}

```
