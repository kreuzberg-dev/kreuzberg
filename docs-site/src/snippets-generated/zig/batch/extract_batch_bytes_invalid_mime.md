---
id: fixture_zig_extract_batch_bytes_invalid_mime
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch with invalid bytes MIME type

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"bytes\":[72,101,108,108,111],\"kind\":\"bytes\",\"mime_type\":\"application/x-nonexistent\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
