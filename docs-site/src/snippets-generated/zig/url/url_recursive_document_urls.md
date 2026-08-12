---
id: fixture_zig_url_recursive_document_urls
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

extract: recursive URL extraction follows document links discovered in results

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", "{\"url\":{\"crawl\":{\"document_url_depth\":1,\"follow_document_urls\":true,\"respect_robots_txt\":false},\"mode\":\"document\"}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
