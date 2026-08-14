```zig title="Zig"
const std = @import("std");
const xberg = @import("xberg");

pub fn main() !void {
    var gpa: std.heap.DebugAllocator(.{}) = .init;
    defer _ = gpa.deinit();
    const allocator = gpa.allocator();

    const config_json = "{}";
    const input_json = "{\"kind\":\"uri\",\"uri\":\"document.pdf\"}";
    const output_json = try xberg.extract(input_json, config_json);
    defer std.heap.c_allocator.free(output_json);

    var parsed = try std.json.parseFromSlice(std.json.Value, allocator, output_json, .{});
    defer parsed.deinit();

    const output = parsed.value;
    if (output != .object) return;

    const results_val = output.object.get("results") orelse return;
    if (results_val != .array or results_val.array.items.len == 0) return;
    const root = results_val.array.items[0];
    if (root != .object) return;

    if (root.object.get("metadata")) |metadata_val| {
        if (metadata_val != .object) return;
        const metadata = metadata_val.object;

        if (metadata.get("title")) |title_val| {
            if (title_val == .string) {
                std.debug.print("Title: {s}\n", .{title_val.string});
            }
        }

        if (metadata.get("authors")) |authors_val| {
            if (authors_val == .array) {
                for (authors_val.array.items) |author| {
                    if (author == .string) {
                        std.debug.print("Author: {s}\n", .{author.string});
                    }
                }
            }
        }

        if (metadata.get("language")) |language_val| {
            if (language_val == .string) {
                std.debug.print("Language: {s}\n", .{language_val.string});
            }
        }

        if (metadata.get("created_at")) |created_val| {
            if (created_val == .string) {
                std.debug.print("Created: {s}\n", .{created_val.string});
            }
        }

        if (metadata.get("pages")) |pages_val| {
            if (pages_val == .object) {
                if (pages_val.object.get("total_count")) |total_val| {
                    if (total_val == .integer) {
                        std.debug.print("Pages: {d}\n", .{total_val.integer});
                    }
                }
            }
        }
    }
}
```
