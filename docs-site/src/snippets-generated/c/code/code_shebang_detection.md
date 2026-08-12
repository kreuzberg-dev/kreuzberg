---
id: fixture_c_code_shebang_detection
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"mime_type\":\"text/x-source-code\",\"uri\":\"https://example.com/code/script.sh\"}");
    XBERGExtractionResult* result = xberg_extract(input_handle, NULL);
    xberg_extract_input_free(input_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
