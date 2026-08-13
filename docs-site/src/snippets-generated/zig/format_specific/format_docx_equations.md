---
id: fixture_zig_format_docx_equations
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

DOCX equations extract to LaTeX math in markdown output

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"filename\":\"equations.docx\",\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.wordprocessingml.document\",\"uri\":\"https://example.com/docx/equations.docx\"}", "{\"output_format\":\"markdown\"}");
    defer std.heap.c_allocator.free(_result_json);
}

```
