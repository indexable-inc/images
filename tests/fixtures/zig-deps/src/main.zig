pub fn main() void {}

test "dependency fixture test runs" {
    const std = @import("std");
    try std.testing.expect(true);
}
