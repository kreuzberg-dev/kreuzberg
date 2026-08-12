---
id: fixture_zig_error_empty_bytes
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

Graceful handling of empty bytes (should not error)

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"bytes\":[],\"config\":{},\"filename\":\"empty.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
