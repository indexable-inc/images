const std = @import("std");
const terminal_api = @import("terminal.zig");
const terminal_graphics = @import("terminal_graphics.zig");
const Result = @import("result.zig").Result;

test "kitty image placement and rgba roundtrip" {
    const testing = std.testing;
    const lib_alloc = @import("../../lib/allocator.zig");

    var handle: terminal_api.TerminalHandle = undefined;
    try testing.expectEqual(Result.success, terminal_api.new(
        &lib_alloc.test_allocator,
        4,
        8,
        &handle,
    ));
    defer terminal_api.free(handle);

    try testing.expectEqual(Result.success, terminal_api.setPixelSize(handle, 80, 40));
    try testing.expectEqual(Result.success, terminal_api.feed(
        handle,
        "\x1b_Gf=24,s=1,v=1,a=T,t=d,c=1,r=1;////\x1b\\",
        "\x1b_Gf=24,s=1,v=1,a=T,t=d,c=1,r=1;////\x1b\\".len,
    ));

    var placement_count: usize = 0;
    try testing.expectEqual(Result.out_of_memory, terminal_graphics.imagePlacements(
        handle,
        0,
        null,
        0,
        &placement_count,
    ));
    try testing.expectEqual(@as(usize, 1), placement_count);

    var placement: [1]terminal_graphics.ImagePlacement = undefined;
    try testing.expectEqual(Result.success, terminal_graphics.imagePlacements(
        handle,
        0,
        &placement,
        placement.len,
        &placement_count,
    ));
    try testing.expectEqual(@as(usize, 1), placement_count);
    try testing.expectEqual(@as(i32, 0), placement[0].row);
    try testing.expectEqual(@as(i32, 0), placement[0].col);
    try testing.expectEqual(@as(u32, 10), placement[0].dest_width);
    try testing.expectEqual(@as(u32, 10), placement[0].dest_height);

    var info: terminal_graphics.ImageInfo = undefined;
    var rgba_len: usize = 0;
    try testing.expectEqual(Result.out_of_memory, terminal_graphics.imageRgba(
        handle,
        placement[0].image_id,
        &info,
        null,
        0,
        &rgba_len,
    ));
    try testing.expectEqual(@as(u32, 1), info.width);
    try testing.expectEqual(@as(u32, 1), info.height);
    try testing.expect(info.generation > 0);
    try testing.expectEqual(@as(usize, 4), rgba_len);

    var rgba: [4]u8 = undefined;
    try testing.expectEqual(Result.success, terminal_graphics.imageRgba(
        handle,
        placement[0].image_id,
        &info,
        &rgba,
        rgba.len,
        &rgba_len,
    ));
    try testing.expectEqualSlices(u8, &.{ 255, 255, 255, 255 }, rgba[0..rgba_len]);
}
