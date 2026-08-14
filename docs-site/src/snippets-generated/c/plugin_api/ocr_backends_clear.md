---
id: fixture_c_ocr_backends_clear
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

Clear all OCR backends and verify list is empty

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    xberg_clear_ocr_backends();
    return EXIT_SUCCESS;
}

```
