---
id: fixture_zig_config_extraction_timeout
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests that extraction_timeout_secs config field is accepted and does not affect fast extractions

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{\"extraction_timeout_secs\":300}");
    defer std.heap.c_allocator.free(_result_json);
}

```
