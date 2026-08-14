---
id: fixture_zig_ocr_image_png
language: zig
target: zig
level: typecheck
requires: []
side_effect: safe
---

OCR: PNG image extraction with OCR enabled. In WASM this exercises the Uint8Array bridge parameter and Promise await in the generated OcrBackend bridge.

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
const input_file_0 = try std.Io.Dir.cwd().readFileAlloc(input_file_0_io, "test_documents/images/test_hello_world.png", allocator, .unlimited);
defer allocator.free(input_file_0);
    const input_file_0_json = try std.json.Stringify.valueAlloc(allocator, input_file_0, .{ .emit_strings_as_arrays = true });
defer allocator.free(input_file_0_json);
    const input_json_0 = try std.mem.replaceOwned(u8, allocator, "{\"bytes\":\"__ALEF_DOC_FILE_0__\",\"config\":{},\"filename\":\"test_hello_world.png\",\"kind\":\"bytes\",\"mime_type\":\"image/png\"}", "\"__ALEF_DOC_FILE_0__\"", input_file_0_json);
defer allocator.free(input_json_0);
    const _result_json = try xberg.extract(input_json_0, "{}");
    defer std.heap.c_allocator.free(_result_json);
}

```
