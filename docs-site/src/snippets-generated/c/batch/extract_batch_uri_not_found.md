---
id: fixture_c_extract_batch_uri_not_found
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI input

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* inputs_handle = xberg_extract_input_from_json("[{\"kind\":\"uri\",\"uri\":\"/nonexistent/a.pdf\"}]");
    XBERGExtractionResult* result = xberg_extract_batch(inputs_handle, NULL);
    xberg_extract_input_free(inputs_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
