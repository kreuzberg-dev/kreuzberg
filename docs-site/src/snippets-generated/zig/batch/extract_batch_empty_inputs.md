---
id: fixture_zig_extract_batch_empty_inputs
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch: empty batch

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
