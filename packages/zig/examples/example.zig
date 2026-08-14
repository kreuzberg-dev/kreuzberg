const std = @import("std");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();
    _ = allocator;

    std.debug.print("Example: module loaded successfully\n", .{});
}
