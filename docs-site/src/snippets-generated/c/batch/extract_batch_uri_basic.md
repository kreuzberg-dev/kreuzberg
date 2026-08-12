---
id: fixture_c_extract_batch_uri_basic
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* inputs_handle = xberg_extract_input_from_json("[{\"kind\":\"uri\",\"uri\":\"pdf/fake_memo.pdf\"},{\"kind\":\"uri\",\"uri\":\"text/fake_text.txt\"}]");
    XBERGExtractionResult* result = xberg_extract_batch(inputs_handle, NULL);
    xberg_extract_input_free(inputs_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
