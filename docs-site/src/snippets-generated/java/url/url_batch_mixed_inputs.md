---
id: fixture_java_url_batch_mixed_inputs
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var configJson = "{\"url\":{\"mode\":\"document\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", ExtractInput.class), JsonUtil.fromJson("{\"bytes\":[66,97,116,99,104,32,98,121,116,101,115,32,99,111,110,116,101,110,116],\"filename\":\"inline.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ExtractInput.class)), config);
        System.out.println(result);
    }
}

```
