---
id: fixture_zig_api_extract_batch_uri_with_config
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests batch URI extraction with per-input config (extract_batch)

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"config\":{\"output_format\":\"markdown\"},\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
