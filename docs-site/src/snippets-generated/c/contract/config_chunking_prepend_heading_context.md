---
id: fixture_c_config_chunking_prepend_heading_context
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"document.md\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"chunking\":{\"chunker_type\":\"markdown\",\"max_characters\":500,\"overlap\":50,\"prepend_heading_context\":true}}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
