---
id: fixture_c_unregister_validator_after_register
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

unregister_validator

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    xberg_("test-validator");
    return EXIT_SUCCESS;
}

```
