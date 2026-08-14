---
id: fixture_zig_error_empty_mime
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

Show how an empty MIME type is rejected consistently.

```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    var input_file_0_threaded = std.Io.Threaded.init(allocator, .{});
defer input_file_0_threaded.deinit();
const input_file_0_io = input_file_0_threaded.io();
const input_file_0 = try std.Io.Dir.cwd().readFileAlloc(input_file_0_io, "test_documents/text/plain.txt", allocator, .unlimited);
defer allocator.free(input_file_0);
    const input_file_0_json = try std.json.Stringify.valueAlloc(allocator, input_file_0, .{ .emit_strings_as_arrays = true });
defer allocator.free(input_file_0_json);
    const input_json_0 = try std.mem.replaceOwned(u8, allocator, "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{},\"filename\":\"plain.txt\",\"kind\":\"bytes\",\"mime_type\":\"\"}", "\"__ALEF_DOC_FILE_0__\"", input_file_0_json);
defer allocator.free(input_json_0);
    if (xberg.extract(input_json_0, "{}")) |_| {
        return error.TestUnexpectedResult;
    } else |err| { std.debug.print("call failed as expected: {s}\n", .{@errorName(err)}); }
}

```
