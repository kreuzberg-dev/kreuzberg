---
id: fixture_java_extract_batch_uri_basic
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.extractBatch(java.util.Arrays.asList(JsonUtil.fromJson("{\"kind\":\"uri\",\"uri\":\"pdf/fake_memo.pdf\"}", ExtractInput.class), JsonUtil.fromJson("{\"kind\":\"uri\",\"uri\":\"text/fake_text.txt\"}", ExtractInput.class)), ExtractionConfig.builder().build());
        for (var result : result.results()) {
            System.out.println(result.content());
        }
    }
}

```
