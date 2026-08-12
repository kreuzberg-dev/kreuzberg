---
id: fixture_zig_config_element_types
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests element-based result format with element type assertions on DOCX

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/unit_test_headers.docx\"}", "{\"result_format\":\"element_based\"}");
    defer std.heap.c_allocator.free(_result_json);
}

```
