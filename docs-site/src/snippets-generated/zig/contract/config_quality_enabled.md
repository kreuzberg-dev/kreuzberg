---
id: fixture_zig_config_quality_enabled
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests quality scoring produces a score value in [0.0, 1.0]

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{\"enable_quality_processing\":true}");
    defer std.heap.c_allocator.free(_result_json);
}

```
