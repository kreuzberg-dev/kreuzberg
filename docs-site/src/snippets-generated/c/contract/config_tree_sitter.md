---
id: fixture_c_config_tree_sitter
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com/code/hello.py\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"tree_sitter\":{\"groups\":[\"web\"],\"languages\":[\"python\",\"rust\"],\"process\":{\"comments\":false,\"diagnostics\":false,\"docstrings\":false,\"exports\":true,\"imports\":true,\"structure\":true,\"symbols\":false}}}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
