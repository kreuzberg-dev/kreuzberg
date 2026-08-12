---
id: fixture_zig_api_extract_batch_bytes_with_config
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

Tests batch bytes extraction with per-input config (extract_batch)

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const inputs_file_0 = try std.Io.Dir.cwd().readFileAlloc(std.testing.io, "test_documents/pdf/fake_memo.pdf", allocator, .unlimited);
defer allocator.free(inputs_file_0);
    const inputs_file_0_json = try std.json.Stringify.valueAlloc(allocator, inputs_file_0, .{ .emit_strings_as_arrays = true });
defer allocator.free(inputs_file_0_json);
    const inputs_json_0 = try std.mem.replaceOwned(u8, allocator, "[{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{\"output_format\":\"markdown\"},\"filename\":\"fake_memo.pdf\",\"kind\":\"bytes\"}]", "\"__ALEF_DOC_FILE_0__\"", inputs_file_0_json);
defer allocator.free(inputs_json_0);
    const _result_json = try xberg.extract_batch(inputs_json_0, "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
