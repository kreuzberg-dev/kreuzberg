---
id: fixture_c_extract_batch_bytes_invalid_mime
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* inputs_handle = xberg_extract_input_from_json("[{\"bytes\":[72,101,108,108,111],\"kind\":\"bytes\",\"mime_type\":\"application/x-nonexistent\"}]");
    XBERGExtractionResult* result = xberg_extract_batch(inputs_handle, NULL);
    xberg_extract_input_free(inputs_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
