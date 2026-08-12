---
id: fixture_java_config_extraction_timeout
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"extraction_timeout_secs\":300}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
