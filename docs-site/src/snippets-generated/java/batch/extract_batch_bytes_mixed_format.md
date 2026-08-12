---
id: fixture_java_extract_batch_bytes_mixed_format
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"bytes\":[80,68,70,32,112,108,97,99,101,104,111,108,100,101,114],\"kind\":\"bytes\",\"mime_type\":\"application/x-unknown\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
