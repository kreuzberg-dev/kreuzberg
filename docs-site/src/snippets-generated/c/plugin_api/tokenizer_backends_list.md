---
id: fixture_c_tokenizer_backends_list
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

List all registered tokenizer backends

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGListTokenizerBackends* result = xberg_list_tokenizer_backends(NULL);
    xberg_list_tokenizer_backends_free(result);
    return EXIT_SUCCESS;
}

```
