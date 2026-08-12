---
id: fixture_java_url_remote_text_document
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

extract: remote text document URL

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"url\":{\"mode\":\"document\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
