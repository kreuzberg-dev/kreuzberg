---
id: fixture_zig_extract_batch_bytes_mixed_format
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch: handles unsupported MIME gracefully

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"bytes\":[80,68,70,32,112,108,97,99,101,104,111,108,100,101,114],\"kind\":\"bytes\",\"mime_type\":\"application/x-unknown\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
