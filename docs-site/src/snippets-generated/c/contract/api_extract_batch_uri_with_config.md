---
id: fixture_c_api_extract_batch_uri_with_config
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* inputs_handle = xberg_extract_input_from_json("[{\"config\":{\"output_format\":\"markdown\"},\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}]");
    XBERGExtractionResult* result = xberg_extract_batch(inputs_handle, NULL);
    xberg_extract_input_free(inputs_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
