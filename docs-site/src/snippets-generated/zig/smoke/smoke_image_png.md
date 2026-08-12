---
id: fixture_zig_smoke_image_png
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: PNG image (without OCR, metadata only)

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/images/sample.png\"}", "{\"disable_ocr\":true}");
    defer std.heap.c_allocator.free(_result_json);
}

```
