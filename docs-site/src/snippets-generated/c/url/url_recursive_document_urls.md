---
id: fixture_c_url_recursive_document_urls
language: c
target: c
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    XBERGExtractInput* input_handle = xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}");
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{\"url\":{\"crawl\":{\"document_url_depth\":1,\"follow_document_urls\":true,\"respect_robots_txt\":false},\"mode\":\"document\"}}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
