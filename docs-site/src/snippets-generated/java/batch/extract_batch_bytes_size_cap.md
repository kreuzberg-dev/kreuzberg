---
id: fixture_java_extract_batch_bytes_size_cap
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract_batch: archive size cap triggers error

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        try {
        var configJson = "{\"security_limits\":{\"max_content_size\":1}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"bytes\":\"test_documents/text/fake_text.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ExtractInput.class)), config);
        System.out.println(result);
        } catch (XbergException error) {
            System.err.println(error.getClass().getSimpleName() + ": " + error.getMessage());
        }
    }
}

```
