---
id: fixture_zig_smoke_xlsx_basic
language: zig
target: zig
level: typecheck
requires: []
side_effect: server
---

Smoke test: XLSX with basic spreadsheet data including tables

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const _result_json = try xberg.extract("{\"kind\":\"uri\",\"mime_type\":\"application/vnd.openxmlformats-officedocument.spreadsheetml.sheet\",\"uri\":\"https://example.com/xlsx/stanley_cups.xlsx\"}", "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
