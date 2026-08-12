---
id: fixture_java_extract_batch_empty_inputs
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.List.of(), ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
