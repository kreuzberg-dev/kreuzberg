---
id: fixture_java_list_ocr_backends
language: java
target: java
level: typecheck
requires: []
side_effect: safe
---

List OCR backends

```java title="Java"
import io.xberg.*;

public final class Example {
    public static void main(String[] args) throws Exception {
        var result = Xberg.listOcrBackends();
        System.out.println(result);
    }
}

```
