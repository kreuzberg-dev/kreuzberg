---
id: fixture_c_error_invalid_mime_format
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

Error when extracting with invalid MIME type format

```c title="C"
#include <assert.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

int main(void) {
    const char *input_json_base = "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"not-a-mime\"}";
    FILE *input_file_0 = fopen("test_documents/text/plain.txt", "rb");
    if (input_file_0 == NULL) return EXIT_FAILURE;
    fseek(input_file_0, 0, SEEK_END);
    long input_size_0 = ftell(input_file_0);
    if (input_size_0 < 0) { fclose(input_file_0); return EXIT_FAILURE; }
    rewind(input_file_0);
    uint8_t *input_bytes_0 = malloc(input_size_0 > 0 ? (size_t)input_size_0 : 1);
    if (input_bytes_0 == NULL) { fclose(input_file_0); return EXIT_FAILURE; }
    if (fread(input_bytes_0, 1, (size_t)input_size_0, input_file_0) != (size_t)input_size_0) { free(input_bytes_0); fclose(input_file_0); return EXIT_FAILURE; }
    fclose(input_file_0);
    char *input_bytes_json_0 = malloc((size_t)input_size_0 * 4 + 3);
    if (input_bytes_json_0 == NULL) { free(input_bytes_0); return EXIT_FAILURE; }
    size_t input_offset_0 = 0;
    input_bytes_json_0[input_offset_0++] = '[';
    for (long i = 0; i < input_size_0; ++i) {
        input_offset_0 += (size_t)snprintf(input_bytes_json_0 + input_offset_0, 5, "%s%u", i == 0 ? "" : ",", input_bytes_0[i]);
    }
    input_bytes_json_0[input_offset_0++] = ']';
    input_bytes_json_0[input_offset_0] = '\0';
    free(input_bytes_0);
    const char *input_marker_0 = "\"__ALEF_DOC_FILE_0__\"";
    const char *input_position_0 = strstr(input_json_base, input_marker_0);
    if (input_position_0 == NULL) { free(input_bytes_json_0); return EXIT_FAILURE; }
    size_t input_prefix_0 = (size_t)(input_position_0 - input_json_base);
    size_t input_json_size_0 = strlen(input_json_base) - strlen(input_marker_0) + strlen(input_bytes_json_0) + 1;
    char *input_json_0 = malloc(input_json_size_0);
    if (input_json_0 == NULL) { free(input_bytes_json_0); return EXIT_FAILURE; }
    snprintf(input_json_0, input_json_size_0, "%.*s%s%s", (int)input_prefix_0, input_json_base, input_bytes_json_0, input_position_0 + strlen(input_marker_0));
    free(input_bytes_json_0);
    XBERGExtractInput* input_handle = xberg_extract_input_from_json(input_json_0);
    free(input_json_0);
    XBERGExtractionConfig* config_handle = xberg_extraction_config_from_json("{}");
    XBERGExtractionResult* result = xberg_extract(input_handle, config_handle);
    xberg_extract_input_free(input_handle);
    xberg_extraction_config_free(config_handle);
    if (result != NULL) { return EXIT_FAILURE; }
    return EXIT_SUCCESS;
}

```
