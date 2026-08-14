---
id: fixture_c_list_embedding_backends
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

List embedding backends

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGListEmbeddingBackends* result = xberg_list_embedding_backends("{}");
    xberg_list_embedding_backends_free(result);
    return EXIT_SUCCESS;
}

```
