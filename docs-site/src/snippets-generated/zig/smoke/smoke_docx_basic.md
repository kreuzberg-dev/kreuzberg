---
id: fixture_zig_smoke_docx_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: DOCX with formatted text

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/fake.docx\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
