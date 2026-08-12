---
id: fixture_zig_config_chunking_prepend_heading_context
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests markdown chunker records heading hierarchy on chunk metadata

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"document.md\"}", "{\"chunking\":{\"chunker_type\":\"markdown\",\"max_characters\":500,\"overlap\":50,\"prepend_heading_context\":true}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
