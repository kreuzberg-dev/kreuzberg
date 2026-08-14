```c title="C"
#include "xberg.h"
#include <stdio.h>

int main(void) {
    const char *config_json = "{"
        "\"ocr\": {\"tesseract\": {\"language\": \"eng\"}}"
        "}";

    XBERGAlefHandle config = xberg_extraction_config_from_json(config_json);
    if (config == 0) {
        fprintf(stderr, "config parse failed (code %d): %s\n",
                xberg_last_error_code(),
                xberg_last_error_context());
        return 1;
    }

    XBERGAlefHandle input = xberg_extract_input_from_uri("scanned.png");
    if (input == 0) {
        fprintf(stderr, "Failed to create input (code %d): %s\n",
                xberg_last_error_code(),
                xberg_last_error_context());
        xberg_extraction_config_free(config);
        return 1;
    }

    XBERGAlefHandle result = xberg_extract(input, config);
    if (result != 0) {
        char *results = xberg_extraction_result_results(result);
        if (results) {
            printf("OCR results: %s\n", results);
        }
        xberg_free_string(results);
    } else {
        fprintf(stderr, "OCR error (code %d): %s\n",
                xberg_last_error_code(),
                xberg_last_error_context());
    }

    xberg_extract_input_free(input);
    xberg_extraction_result_free(result);
    xberg_extraction_config_free(config);
    return 0;
}
```
