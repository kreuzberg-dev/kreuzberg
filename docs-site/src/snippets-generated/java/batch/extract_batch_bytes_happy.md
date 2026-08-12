---
id: fixture_java_extract_batch_bytes_happy
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"bytes\":[72,101,108,108,111,44,32,119,111,114,108,100,33],\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", ExtractInput.class), JsonUtil.fromJson("{\"bytes\":\"test_documents/html/html.html\",\"kind\":\"bytes\",\"mime_type\":\"text/html\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
