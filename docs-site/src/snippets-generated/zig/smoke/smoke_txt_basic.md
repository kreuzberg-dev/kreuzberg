---
id: fixture_zig_smoke_txt_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: Plain text file

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"text/plain\",\"uri\":\"https://example.com/text/report.txt\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
