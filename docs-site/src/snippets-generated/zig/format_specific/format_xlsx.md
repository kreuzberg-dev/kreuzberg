---
id: fixture_zig_format_xlsx
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

XLSX spreadsheet extraction using extract

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
