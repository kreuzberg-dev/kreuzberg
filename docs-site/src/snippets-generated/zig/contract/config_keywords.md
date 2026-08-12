---
id: fixture_zig_config_keywords
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests keyword extraction via YAKE algorithm

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{\"keywords\":{\"algorithm\":\"yake\",\"max_keywords\":10}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
