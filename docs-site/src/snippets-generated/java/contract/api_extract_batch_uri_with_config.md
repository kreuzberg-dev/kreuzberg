---
id: fixture_java_api_extract_batch_uri_with_config
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"config\":{\"output_format\":\"markdown\"},\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        for (var result : result.results()) {
            System.out.println(result.content());
        }
    }
}

```
