---
id: fixture_zig_config_embedding_plugin
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests EmbeddingModelType::Plugin variant deserialization in ChunkingConfig — config accepts the plugin variant shape; actual dispatch requires a host-language backend registered via register_embedding_backend at runtime

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{\"chunking\":{\"embedding\":{\"max_embed_duration_secs\":30,\"model\":{\"name\":\"test-plugin-backend\",\"type\":\"plugin\"},\"normalize\":true},\"max_chars\":500,\"max_overlap\":50}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
