---
id: fixture_zig_url_html_page_extract
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

extract: website URL returns page content

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", "{\"url\":{\"mode\":\"document\"}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
