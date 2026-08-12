---
id: fixture_zig_format_docx_standalone
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Standalone DOCX extraction using extract

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"filename\":\"fake.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/fake.docx\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
