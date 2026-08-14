---
id: fixture_zig_extract_batch_bytes_happy
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

Extract multiple in-memory documents in one batch.

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    var inputs_file_0_threaded = std.Io.Threaded.init(allocator, .{});
defer inputs_file_0_threaded.deinit();
const inputs_file_0_io = inputs_file_0_threaded.io();
const inputs_file_0 = try std.Io.Dir.cwd().readFileAlloc(inputs_file_0_io, "test_documents/html/html.html", allocator, .unlimited);
defer allocator.free(inputs_file_0);
    const inputs_file_0_json = try std.json.Stringify.valueAlloc(allocator, inputs_file_0, .{ .emit_strings_as_arrays = true });
defer allocator.free(inputs_file_0_json);
    const inputs_json_0 = try std.mem.replaceOwned(u8, allocator, "[{\"bytes\":[72,101,108,108,111,44,32,119,111,114,108,100,33],\"kind\":\"bytes\",\"mime_type\":\"text/plain\"},{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"kind\":\"bytes\",\"mime_type\":\"text/html\"}]", "\"__ALEF_DOC_FILE_0__\"", inputs_file_0_json);
defer allocator.free(inputs_json_0);
    const _result_json = try xberg.extract_batch(inputs_json_0, "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
