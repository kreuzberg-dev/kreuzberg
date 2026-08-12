---
id: fixture_zig_smoke_pdf_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: PDF with simple text extraction

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"application/pdf\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
