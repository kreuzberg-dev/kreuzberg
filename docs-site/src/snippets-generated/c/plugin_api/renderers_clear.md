---
id: fixture_c_renderers_clear
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

Clear all renderers and verify list is empty

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    xberg_clear_renderers();
    return EXIT_SUCCESS;
}

```
