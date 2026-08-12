```c title="C"
#include "xberg.h"
#include <stdio.h>
#include <stdlib.h>

int main(void) {
    XBERGExtractionConfig *config = xberg_extraction_config_default();

    /* Pass an unsupported MIME type to trigger an error. */
    XBERGExtractInput *input =
        xberg_extract_input_from_bytes(NULL, 0, "application/x-unknown", NULL);
    if (!input) {
        int32_t code = xberg_last_error_code();
        const char *message = xberg_last_error_context();
        /* message is valid until the next FFI call on this thread — copy if needed. */
        fprintf(stderr, "error %d: %s\n", code, message ? message : "(no message)");
        xberg_extraction_config_free(config);
        return code != 0 ? code : 1;
    }

    XBERGExtractionResult *result = xberg_extract(input, config);
    if (!result) {
        int32_t code = xberg_last_error_code();
        const char *message = xberg_last_error_context();
        fprintf(stderr, "error %d: %s\n", code, message ? message : "(no message)");
        xberg_extract_input_free(input);
        xberg_extraction_config_free(config);
        return code != 0 ? code : 1;
    }

    char *content = xberg_extraction_result_results(result);
    printf("%s\n", content ? content : "(empty)");
    xberg_free_string(content);

    xberg_extract_input_free(input);
    xberg_extraction_result_free(result);
    xberg_extraction_config_free(config);
    return 0;
}
```
