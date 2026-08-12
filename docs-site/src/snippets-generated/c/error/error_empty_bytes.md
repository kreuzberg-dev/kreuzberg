---
id: fixture_c_error_empty_bytes
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"bytes\":[],\"config\":{},\"filename\":\"empty.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
