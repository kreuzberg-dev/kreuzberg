```c title="C"
#include <xberg.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main(void) {
    XBERGAlefHandle config = xberg_extraction_config_from_json("{}");

    XBERGAlefHandle input = xberg_extract_input_from_uri("document.pdf");
    if (input == 0) {
        fprintf(stderr, "Failed to create input (code %d): %s\n",
                xberg_last_error_code(),
                xberg_last_error_context());
        xberg_extraction_config_free(config);
        return 1;
    }

    XBERGAlefHandle result = xberg_extract(input, config);
    if (result == 0) {
        fprintf(stderr, "extraction failed (code %d): %s\n",
                xberg_last_error_code(),
                xberg_last_error_context());
        xberg_extract_input_free(input);
        xberg_extraction_config_free(config);
        return 1;
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
