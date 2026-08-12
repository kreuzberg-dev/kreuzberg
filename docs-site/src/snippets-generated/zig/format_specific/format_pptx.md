---
id: fixture_zig_format_pptx
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

PPTX presentation extraction using extract

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.presentationml.presentation\",\"uri\":\"https://example.com/pptx/simple.pptx\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
