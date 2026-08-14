```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const inputs_json =
        "["
        ++ "{\"kind\":\"uri\",\"uri\":\"document.pdf\"},"
        ++ "{\"kind\":\"bytes\",\"bytes\":[72,101,108,108,111],"
        ++ "\"mime_type\":\"text/plain\",\"filename\":\"note.txt\"}"
        ++ "]";

    const output_json = try xberg.extract_batch(inputs_json, "{}");
    defer std.heap.c_allocator.free(output_json);

    std.debug.print("{s}\n", .{output_json});
}
```
