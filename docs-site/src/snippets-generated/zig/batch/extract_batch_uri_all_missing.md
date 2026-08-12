---
id: fixture_zig_extract_batch_uri_all_missing
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch with missing URI inputs

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"kind\":\"uri\",\"uri\":\"/nonexistent/a.pdf\"},{\"kind\":\"uri\",\"uri\":\"/nonexistent/b.txt\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
