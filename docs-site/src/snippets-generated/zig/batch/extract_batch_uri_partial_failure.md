---
id: fixture_zig_extract_batch_uri_partial_failure
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch with mixed valid and missing URI inputs

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"kind\":\"uri\",\"uri\":\"text/plain.txt\"},{\"kind\":\"uri\",\"uri\":\"/nonexistent/missing.pdf\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
