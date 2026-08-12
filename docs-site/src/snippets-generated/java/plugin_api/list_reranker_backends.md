---
id: fixture_java_list_reranker_backends
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

List all registered reranker backends

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.listRerankerBackends();
        System.out.println(result);
    }
}

```
