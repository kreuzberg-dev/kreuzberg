---
id: fixture_zig_url_batch_mixed_inputs
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

extract_batch: mixed bytes and URL inputs share one output envelope

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract_batch("[{\"kind\":\"uri\",\"uri\":\"https://example.com\"},{\"bytes\":[66,97,116,99,104,32,98,121,116,101,115,32,99,111,110,116,101,110,116],\"filename\":\"inline.txt\",\"kind\":\"bytes\",\"mime_type\":\"text/plain\"}]", "{\"url\":{\"mode\":\"document\"}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
