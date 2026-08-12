---
id: fixture_c_format_pptx
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.presentationml.presentation\",\"uri\":\"https://example.com/pptx/simple.pptx\"}");
    XBERGExtractionResult* result = xberg_extract(input_handle, NULL);
    xberg_extract_input_free(input_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
