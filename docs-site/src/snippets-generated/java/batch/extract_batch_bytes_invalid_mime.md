---
id: fixture_java_extract_batch_bytes_invalid_mime
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"bytes\":[72,101,108,108,111],\"kind\":\"bytes\",\"mime_type\":\"application/x-nonexistent\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
