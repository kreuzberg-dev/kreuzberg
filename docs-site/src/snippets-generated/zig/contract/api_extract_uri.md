---
id: fixture_zig_api_extract_uri
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests URI extraction API

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
