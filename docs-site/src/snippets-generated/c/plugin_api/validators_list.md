---
id: fixture_c_validators_list
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

List all registered validators

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    char* result = xberg_list_validators();
    xberg_free_string(result);
    return EXIT_SUCCESS;
}

```
