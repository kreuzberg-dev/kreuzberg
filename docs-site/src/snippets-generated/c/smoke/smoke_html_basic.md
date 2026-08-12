---
id: fixture_c_smoke_html_basic
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"mime_type\":\"text/html\",\"uri\":\"https://example.com/html/simple_table.html\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
