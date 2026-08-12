---
id: fixture_zig_format_pdf_text
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Standalone PDF text extraction using extract

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"filename\":\"fake_memo.pdf\",\"kind\":\"uri\",\"mime_type\":\"application/pdf\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
