---
id: fixture_java_extract_batch_uri_all_missing
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"kind\":\"uri\",\"uri\":\"/nonexistent/a.pdf\"}", ExtractInput.class), JsonUtil.fromJson("{\"kind\":\"uri\",\"uri\":\"/nonexistent/b.txt\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
