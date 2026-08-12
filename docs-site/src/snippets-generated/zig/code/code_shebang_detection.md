---
id: fixture_zig_code_shebang_detection
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Test language detection from shebang line via bytes input

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"text/x-source-code\",\"uri\":\"https://example.com/code/script.sh\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
