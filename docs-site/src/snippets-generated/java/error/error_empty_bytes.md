---
id: fixture_java_error_empty_bytes
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"bytes\":[],\"config\":{},\"filename\":\"empty.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
