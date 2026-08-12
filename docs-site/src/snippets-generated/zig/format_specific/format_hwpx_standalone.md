---
id: fixture_zig_format_hwpx_standalone
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Standalone HWPX extraction using extract

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"filename\":\"simple.hwpx\",\"kind\":\"uri\",\"mime_type\":\"application/haansofthwpx\",\"uri\":\"https://example.com/hwpx/simple.hwpx\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
