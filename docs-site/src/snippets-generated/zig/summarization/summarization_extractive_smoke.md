---
id: fixture_zig_summarization_extractive_smoke
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

TextRank extractive summary over a multi-paragraph plain text document. Pure-Rust, deterministic, no external services required.

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"uri\":\"https://example.com/text/book_war_and_peace_1p.txt\"}", "{\"summarization\":{\"max_tokens\":80,\"strategy\":\"extractive\"}}");
    defer std.heap.c_allocator.free(_result_json);
}

```
