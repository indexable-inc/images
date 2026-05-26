const std = @import("std");
const lib = @import("lib.zig");

pub fn main() !void {
    const stdout = std.io.getStdOut().writer();
    try stdout.print("{s}\n", .{lib.greeting()});
}

test "main imports the library" {
    try std.testing.expect(lib.greeting().len > 0);
}
