---
id: fixture_c_config_document_structure_with_headings
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/fake.docx\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"include_document_structure\":true}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
