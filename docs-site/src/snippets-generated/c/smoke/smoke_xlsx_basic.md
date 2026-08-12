---
id: fixture_c_smoke_xlsx_basic
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
