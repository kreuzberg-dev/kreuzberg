---
id: fixture_zig_extract_batch_uri_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

extract_batch over URI inputs

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"kind\":\"uri\",\"uri\":\"pdf/fake_memo.pdf\"},{\"kind\":\"uri\",\"uri\":\"text/fake_text.txt\"}]", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
