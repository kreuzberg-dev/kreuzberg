---
id: fixture_c_unregister_post_processor_after_register
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

unregister_post_processor

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    xberg_("test-processor");
    return EXIT_SUCCESS;
}

```
