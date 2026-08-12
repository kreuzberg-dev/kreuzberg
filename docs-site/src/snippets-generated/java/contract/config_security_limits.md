---
id: fixture_java_config_security_limits
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/archives/documents.zip\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"security_limits\":{\"max_archive_size\":104857600,\"max_compression_ratio\":50,\"max_files_in_archive\":100}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
