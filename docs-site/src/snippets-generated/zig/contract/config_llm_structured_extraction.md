---
id: fixture_zig_config_llm_structured_extraction
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Tests structured extraction via liter-llm with JSON schema

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/pdf/fake_memo.pdf\"}", "{\"structured_extraction\":{\"llm\":{\"model\":\"openai/gpt-4o\"},\"schema\":{\"properties\":{\"date\":{\"type\":\"string\"},\"summary\":{\"type\":\"string\"},\"title\":{\"type\":\"string\"}},\"required\":[\"title\"],\"type\":\"object\"},\"schema_name\":\"memo_data\"}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
