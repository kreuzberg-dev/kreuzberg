---
id: fixture_java_smoke_html_basic
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"text/html\",\"uri\":\"https://example.com/html/simple_table.html\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        for (var table : result.results().get(0).tables()) {
            System.out.println(table.rows());
        }
    }
}

```
