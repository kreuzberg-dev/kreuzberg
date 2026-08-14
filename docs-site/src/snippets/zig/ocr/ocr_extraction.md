```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const config_json =
        \\{
        \\  "ocr": {
        \\    "backend": "tesseract",
        \\    "language": "eng"
        \\  }
        \\}
    ;

    const input_json = "{\"kind\":\"uri\",\"uri\":\"scanned.pdf\"}";
    const output_json = try xberg.extract(input_json, config_json);
    defer std.heap.c_allocator.free(output_json);

    const owned = try allocator.dupe(u8, output_json);
    defer allocator.free(owned);

    std.debug.print("{s}\n", .{owned});
}
```
