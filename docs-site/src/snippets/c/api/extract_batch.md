```c title="C"
#include "xberg.h"
#include <stdio.h>

int main(void) {
    const char *inputs_json =
        "["
        "{\"kind\":\"uri\",\"uri\":\"document.pdf\"},"
        "{\"kind\":\"bytes\",\"bytes\":[72,101,108,108,111],"
        "\"mime_type\":\"text/plain\",\"filename\":\"note.txt\"}"
        "]";

    XBERGAlefHandle config = xberg_extraction_config_from_json("{}");
    XBERGAlefHandle output = xberg_extract_batch(inputs_json, config);
    if (output == 0) {
        fprintf(stderr, "batch extraction failed: %s\n", xberg_last_error_context());
        xberg_extraction_config_free(config);
        return 1;
    }

    char *json = xberg_extraction_result_to_json(output);
    puts(json);

    xberg_free_string(json);
    xberg_extraction_result_free(output);
    xberg_extraction_config_free(config);
    return 0;
}
```
