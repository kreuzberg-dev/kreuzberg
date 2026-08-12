---
id: fixture_java_config_keywords
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"keywords\":{\"algorithm\":\"yake\",\"max_keywords\":10}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        for (var keyword : result.results().get(0).keywords()) {
            System.out.println(keyword.text());
            System.out.println(keyword.score());
        }
    }
}

```
