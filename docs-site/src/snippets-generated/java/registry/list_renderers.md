---
id: fixture_java_list_renderers
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

List renderers

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.listRenderers();
        System.out.println(result);
    }
}

```
