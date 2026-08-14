```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    const output_json = try xberg.extract(
        "{\"kind\":\"uri\",\"uri\":\"document.pdf\"}",
        "{}",
    );
    defer std.heap.c_allocator.free(output_json);

    std.debug.print("{s}\n", .{output_json});
}
```
