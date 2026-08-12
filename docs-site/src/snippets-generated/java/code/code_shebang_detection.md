---
id: fixture_java_code_shebang_detection
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"mime_type\":\"text/x-source-code\",\"uri\":\"https://example.com/code/script.sh\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
