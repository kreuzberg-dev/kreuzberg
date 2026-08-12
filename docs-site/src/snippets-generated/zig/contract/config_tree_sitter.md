---
id: fixture_zig_config_tree_sitter
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests tree-sitter configuration round-trip

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/code/hello.py\"}", "{\"tree_sitter\":{\"groups\":[\"web\"],\"languages\":[\"python\",\"rust\"],\"process\":{\"comments\":false,\"diagnostics\":false,\"docstrings\":false,\"exports\":true,\"imports\":true,\"structure\":true,\"symbols\":false}}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
