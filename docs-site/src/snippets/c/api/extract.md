```c title="C"
#include "xberg.h"
#include <stdio.h>

int main(void) {
    XBERGAlefHandle config = xberg_extraction_config_from_json("{}");
    XBERGAlefHandle input =
        xberg_extract_input_from_json("{\"kind\":\"uri\",\"uri\":\"document.pdf\"}");

    XBERGAlefHandle output = xberg_extract(input, config);
    if (output == 0) {
        fprintf(stderr, "extraction failed: %s\n", xberg_last_error_context());
        xberg_extract_input_free(input);
        xberg_extraction_config_free(config);
        return 1;
    }

    char *json = xberg_extraction_result_to_json(output);
    puts(json);

    xberg_free_string(json);
    xberg_extraction_result_free(output);
    xberg_extract_input_free(input);
    xberg_extraction_config_free(config);
    return 0;
}
```
