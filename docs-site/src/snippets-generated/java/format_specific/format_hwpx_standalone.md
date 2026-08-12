---
id: fixture_java_format_hwpx_standalone
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"filename\":\"simple.hwpx\",\"kind\":\"uri\",\"mime_type\":\"application/haansofthwpx\",\"uri\":\"https://example.com/hwpx/simple.hwpx\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
