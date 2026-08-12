---
id: fixture_java_summarization_extractive_smoke
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/text/book_war_and_peace_1p.txt\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"summarization\":{\"max_tokens\":80,\"strategy\":\"extractive\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result.results().get(0).summary());
    }
}

```
