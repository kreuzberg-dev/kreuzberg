---
id: fixture_java_url_crawl_linked_pages
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"url\":{\"crawl\":{\"max_depth\":1,\"max_pages\":4,\"respect_robots_txt\":false},\"mode\":\"crawl\"}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
