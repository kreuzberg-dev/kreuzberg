---
id: fixture_zig_config_security_limits
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests archive extraction with custom security limits

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/archives/documents.zip\"}", "{\"security_limits\":{\"max_archive_size\":104857600,\"max_compression_ratio\":50,\"max_files_in_archive\":100}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
