---
id: fixture_c_post_processors_clear
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

Clear all post-processors and verify list is empty

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    xberg_();
    return EXIT_SUCCESS;
}

```
