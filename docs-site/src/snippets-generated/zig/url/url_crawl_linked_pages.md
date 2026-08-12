---
id: fixture_zig_url_crawl_linked_pages
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

extract: crawl mode follows linked pages

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com\"}", "{\"url\":{\"crawl\":{\"max_depth\":1,\"max_pages\":4,\"respect_robots_txt\":false},\"mode\":\"crawl\"}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
