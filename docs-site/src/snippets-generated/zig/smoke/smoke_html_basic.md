---
id: fixture_zig_smoke_html_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: HTML table extraction

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"text/html\",\"uri\":\"https://example.com/html/simple_table.html\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
