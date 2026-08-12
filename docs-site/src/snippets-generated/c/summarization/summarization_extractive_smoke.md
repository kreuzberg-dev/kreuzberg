---
id: fixture_c_summarization_extractive_smoke
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com/text/book_war_and_peace_1p.txt\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"summarization\":{\"max_tokens\":80,\"strategy\":\"extractive\"}}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
