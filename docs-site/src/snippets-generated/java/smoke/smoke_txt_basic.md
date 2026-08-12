---
id: fixture_java_smoke_txt_basic
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"text/plain\",\"uri\":\"https://example.com/text/report.txt\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
