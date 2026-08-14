---
id: fixture_c_list_ocr_backends
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

List OCR backends

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    char* result = xberg_list_ocr_backends();
    xberg_free_string(result);
    return EXIT_SUCCESS;
}

```
