---
id: fixture_c_config_extraction_timeout
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"extraction_timeout_secs\":300}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
