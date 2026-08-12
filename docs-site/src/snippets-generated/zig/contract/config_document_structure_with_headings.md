---
id: fixture_zig_config_document_structure_with_headings
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests document structure with DOCX heading-driven nesting

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/docx/fake.docx\"}", "{\"include_document_structure\":true}");
    defer std.heap.c_allocator.free(_result_json);
}

```
