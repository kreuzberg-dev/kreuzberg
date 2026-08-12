---
id: fixture_c_format_hwpx_standalone
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"filename\":\"simple.hwpx\",\"kind\":\"uri\",\"mime_type\":\"application/haansofthwpx\",\"uri\":\"https://example.com/hwpx/simple.hwpx\"}");
    XBERGExtractionResult* result = xberg_extract(input_handle, NULL);
    xberg_extract_input_free(input_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
