---
id: fixture_java_api_extract_batch_bytes_with_config
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"bytes\":\"test_documents/pdf/fake_memo.pdf\",\"config\":{\"output_format\":\"markdown\"},\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        System.out.println(result);
    }
}

```
