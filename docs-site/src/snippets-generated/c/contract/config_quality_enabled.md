---
id: fixture_c_config_quality_enabled
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"enable_quality_processing\":true}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
