---
id: fixture_c_unregister_tokenizer_backend_after_register
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

unregister_tokenizer_backend

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    xberg_("test-tokenizer-backend");
    return EXIT_SUCCESS;
}

```
