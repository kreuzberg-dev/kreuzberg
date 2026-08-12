---
id: fixture_c_smoke_image_png
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com/images/sample.png\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"disable_ocr\":true}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
