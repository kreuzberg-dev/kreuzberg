---
id: fixture_zig_api_extract_batch_uri
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction API (extract_batch)

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
