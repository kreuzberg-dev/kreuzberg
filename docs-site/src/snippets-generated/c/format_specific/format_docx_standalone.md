---
id: fixture_c_format_docx_standalone
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"filename\":\"fake.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/fake.docx\"}");
    XBERGExtractionResult* result = xberg_extract(input_handle, NULL);
    xberg_extract_input_free(input_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
