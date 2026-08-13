---
id: fixture_c_format_docx_equations
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"filename\":\"equations.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/equations.docx\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"output_format\":\"markdown\"}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
