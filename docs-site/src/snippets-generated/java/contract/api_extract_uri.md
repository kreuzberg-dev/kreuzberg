---
id: fixture_java_api_extract_uri
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var result = Xberg.extract(input, ExtractionConfig.builder().build());
        System.out.println(result.results().get(0).content());
    }
}

```
