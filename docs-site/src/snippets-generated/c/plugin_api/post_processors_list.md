---
id: fixture_c_post_processors_list
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

List all registered post-processors

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    char* result = xberg_list_post_processors();
    xberg_free_string(result);
    return EXIT_SUCCESS;
}

```
