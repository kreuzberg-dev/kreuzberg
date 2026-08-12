---
id: fixture_c_renderers_list
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

List all registered renderers

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGListRenderers* result = xberg_list_renderers(NULL);
    xberg_list_renderers_free(result);
    return EXIT_SUCCESS;
}

```
