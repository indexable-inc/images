pub fn greeting() []const u8 {
    return "hello from zig app fixture";
}

test "greeting text is stable" {
    const std = @import("std");
    try std.testing.expectEqualStrings("hello from zig app fixture", greeting());
}
