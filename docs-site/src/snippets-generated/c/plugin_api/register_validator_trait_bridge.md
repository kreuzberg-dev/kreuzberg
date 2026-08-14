---
id: fixture_c_register_validator_trait_bridge
language: c
target: c
level: typecheck
requires: []
side_effect: safe
---

register_validator: trait bridge

```c title="C"
#include <stdint.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "xberg.h"

typedef struct SampleContext {
    int released;
} SampleContext;

static char *sample_copy_string(const char *value) {
    size_t length = strlen(value) + 1;
    char *copy = malloc(length);
    if (copy != NULL) {
        memcpy(copy, value, length);
    }
    return copy;
}

static void sample_release_callback_string(char *value) {
    free(value);
}

static void sample_free_context(void *user_data) {
    SampleContext *context = user_data;
    if (context != NULL) {
        context->released = 1;
        free(context);
    }
}

static int32_t sample_name(const void *user_data, char **out_result, char **out_error) {
    (void)user_data;
    *out_error = NULL;
    *out_result = sample_copy_string("test-backend");
        return *out_result == NULL ? -1 : 0;
}

static int32_t sample_version(const void *user_data, char **out_result, char **out_error) {
    (void)user_data;
    *out_error = NULL;
    *out_result = sample_copy_string("1.0.0");
        return *out_result == NULL ? -1 : 0;
}

static int32_t sample_initialize(const void *user_data, char **out_error) {
    (void)user_data;
    *out_error = NULL;
    return 0;
}

static int32_t sample_shutdown(const void *user_data, char **out_error) {
    (void)user_data;
    *out_error = NULL;
    return 0;
}

static int32_t sample_validate(const void *user_data, const char * result, const char * config, char ** out_error) {
    (void)user_data;
    (void)result;
    (void)config;
    *out_error = NULL;
    return 0;
}

int main(void) {
    SampleContext *context = calloc(1, sizeof(*context));
    if (context == NULL) {
        return EXIT_FAILURE;
    }
    XBERGXbergValidatorVTable vtable = {
        .name_fn = sample_name,
        .version_fn = sample_version,
        .initialize_fn = sample_initialize,
        .shutdown_fn = sample_shutdown,
        .validate = sample_validate,
        .free_string = sample_release_callback_string,
        .free_user_data = sample_free_context,
    };
    char *error = NULL;
    int32_t status = xberg_register_validator("test-backend", &vtable, context, &error);
    if (status != 0) {
        xberg_free_string(error);
        return EXIT_FAILURE;
    }
    status = xberg_unregister_validator("test-backend", &error);
    if (status != 0) {
        xberg_free_string(error);
        return EXIT_FAILURE;
    }
    return EXIT_SUCCESS;
}

```
