---
id: fixture_c_api_extract_batch_bytes_with_config
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    const char *inputs_json_base = "[{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{\"output_format\":\"markdown\"},\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\"}]";
    FILE *inputs_file_0 = fopen("test_documents/pdf/fake_memo.pdf", "rb");
    if (inputs_file_0 == NULL) return EXIT_FAILURE;
    fseek(inputs_file_0, 0, SEEK_END);
    long inputs_size_0 = ftell(inputs_file_0);
    if (inputs_size_0 < 0) { fclose(inputs_file_0); return EXIT_FAILURE; }
    rewind(inputs_file_0);
    uint8_t *inputs_bytes_0 = malloc(inputs_size_0 > 0 ? (size_t)inputs_size_0 : 1);
    if (inputs_bytes_0 == NULL) { fclose(inputs_file_0); return EXIT_FAILURE; }
    if (fread(inputs_bytes_0, 1, (size_t)inputs_size_0, inputs_file_0) != (size_t)inputs_size_0) { free(inputs_bytes_0); fclose(inputs_file_0); return EXIT_FAILURE; }
    fclose(inputs_file_0);
    char *inputs_bytes_json_0 = malloc((size_t)inputs_size_0 * 4 + 3);
    if (inputs_bytes_json_0 == NULL) { free(inputs_bytes_0); return EXIT_FAILURE; }
    size_t inputs_offset_0 = 0;
    inputs_bytes_json_0[inputs_offset_0++] = '[';
    for (long i = 0; i < inputs_size_0; ++i) {
        inputs_offset_0 += (size_t)snprintf(inputs_bytes_json_0 + inputs_offset_0, 5, "%s%u", i == 0 ? "" : ",", inputs_bytes_0[i]);
    }
    inputs_bytes_json_0[inputs_offset_0++] = ']';
    inputs_bytes_json_0[inputs_offset_0] = '\0';
    free(inputs_bytes_0);
    const char *inputs_marker_0 = "\"__ALEF_DOC_FILE_0__\"";
    const char *inputs_position_0 = strstr(inputs_json_base, inputs_marker_0);
    if (inputs_position_0 == NULL) { free(inputs_bytes_json_0); return EXIT_FAILURE; }
    size_t inputs_prefix_0 = (size_t)(inputs_position_0 - inputs_json_base);
    size_t inputs_json_size_0 = strlen(inputs_json_base) - strlen(inputs_marker_0) + strlen(inputs_bytes_json_0) + 1;
    char *inputs_json_0 = malloc(inputs_json_size_0);
    if (inputs_json_0 == NULL) { free(inputs_bytes_json_0); return EXIT_FAILURE; }
    snprintf(inputs_json_0, inputs_json_size_0, "%.*s%s%s", (int)inputs_prefix_0, inputs_json_base, inputs_bytes_json_0, inputs_position_0 + strlen(inputs_marker_0));
    free(inputs_bytes_json_0);
    XBERGExtractInput* inputs_handle = xberg_extract_input_from_json(inputs_json_0);
    free(inputs_json_0);
    XBERGExtractionResult* result = xberg_extract_batch(inputs_handle, NULL);
    xberg_extract_input_free(inputs_handle);
    xberg_extraction_result_free(result);
    return EXIT_SUCCESS;
}

```
