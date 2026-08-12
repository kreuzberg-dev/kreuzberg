---
id: fixture_zig_config_pages
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests page extraction and page marker configuration

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{\"pages\":{\"extract_pages\":true,\"insert_page_markers\":true}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
