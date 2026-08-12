---
id: fixture_zig_smoke_json_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: JSON file extraction

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"application/json\",\"uri\":\"https://example.com/json/simple.json\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
