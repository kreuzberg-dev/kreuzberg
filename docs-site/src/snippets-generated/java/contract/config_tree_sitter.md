---
id: fixture_java_config_tree_sitter
language: java
target: java
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var inputJson = "{\"kind\":\"uri\",\"uri\":\"https://example.com/code/hello.py\"}";
var input = JsonUtil.fromJson(inputJson, ExtractInput.class);
        var configJson = "{\"tree_sitter\":{\"groups\":[\"web\"],\"languages\":[\"python\",\"rust\"],\"process\":{\"comments\":false,\"diagnostics\":false,\"docstrings\":false,\"exports\":true,\"imports\":true,\"structure\":true,\"symbols\":false}}}";
var config = JsonUtil.fromJson(configJson, ExtractionConfig.class);
        var result = Xberg.extract(input, config);
        System.out.println(result);
    }
}

```
