---
id: fixture_zig_extract_batch_bytes_unsupported_mime
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch with unsupported bytes MIME type

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"bytes\":[100,97,116,97],\"kind\":\"bytes\",\"mime_type\":\"application/x-unknown\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
