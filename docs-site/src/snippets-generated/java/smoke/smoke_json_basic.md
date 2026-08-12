---
id: fixture_java_smoke_json_basic
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"application/json\",\"uri\":\"https://example.com/json/simple.json\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
