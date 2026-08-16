const std = @import("std");
const terminal_api = @import("terminal.zig");
const terminalpkg = @import("../main.zig");
const image_rgba = @import("terminal_image_rgba.zig");
const Result = @import("result.zig").Result;

const Image = terminalpkg.kitty.graphics.Image;
const Placement = terminalpkg.kitty.graphics.ImageStorage.Placement;
const TerminalHandle = terminal_api.TerminalHandle;

pub const ImagePlacement = extern struct {
    image_id: u32,
    row: i32,
    col: i32,
    z_index: i32,
    offset_x: u32,
    offset_y: u32,
    source_x: u32,
    source_y: u32,
    source_width: u32,
    source_height: u32,
    dest_width: u32,
    dest_height: u32,
};

pub const ImageInfo = extern struct {
    width: u32,
    height: u32,
    generation: u64,
};

comptime {
    if (@sizeOf(ImagePlacement) != 48) @compileError("unexpected image placement size");
    if (@sizeOf(ImageInfo) != 16) @compileError("unexpected image info size");
}

pub fn imagePlacements(
    handle: TerminalHandle,
    viewport_row: usize,
    out: ?[*]ImagePlacement,
    out_len: usize,
    written: ?*usize,
) callconv(.c) Result {
    const wrapper = handle orelse return .invalid_value;
    const out_written = written orelse return .invalid_value;
    if (comptime !terminalpkg.options.kitty_graphics) {
        out_written.* = 0;
        return .success;
    }

    const screen = wrapper.terminal.screens.active;
    const viewport_top = clampViewportRow(screen, viewport_row);
    const viewport_bottom = viewport_top + screen.pages.rows - 1;

    const count = countVisiblePlacements(&wrapper.terminal, viewport_top, viewport_bottom);
    out_written.* = count;
    if (count == 0) return .success;

    const buffer = out orelse return .out_of_memory;
    if (out_len < count) return .out_of_memory;
    writeVisiblePlacements(buffer[0..count], &wrapper.terminal, viewport_top, viewport_bottom);
    return .success;
}

pub fn imageRgba(
    handle: TerminalHandle,
    image_id: u32,
    info: ?*ImageInfo,
    out: ?[*]u8,
    out_len: usize,
    written: ?*usize,
) callconv(.c) Result {
    const wrapper = handle orelse return .invalid_value;
    const out_info = info orelse return .invalid_value;
    const out_written = written orelse return .invalid_value;
    if (comptime !terminalpkg.options.kitty_graphics) return .invalid_value;
    const image = wrapper.terminal.screens.active.kitty_images.imageById(image_id) orelse
        return .invalid_value;

    const required = image_rgba.byteLen(image) catch return .invalid_value;
    out_info.* = .{
        .width = image.width,
        .height = image.height,
        .generation = image.generation,
    };
    out_written.* = required;

    const buffer = out orelse return .out_of_memory;
    if (out_len < required) return .out_of_memory;
    image_rgba.write(buffer[0..required], image);
    return .success;
}

fn clampViewportRow(screen: *const terminalpkg.Screen, viewport_row: usize) usize {
    const total_rows = screen.pages.total_rows;
    const viewport_rows: usize = @intCast(screen.pages.rows);
    const max_viewport_row = total_rows - viewport_rows;
    return @min(viewport_row, max_viewport_row);
}

fn resolvePlacement(
    terminal: *const terminalpkg.Terminal,
    viewport_top: usize,
    viewport_bottom: usize,
    placement: Placement,
    image: Image,
) ?ImagePlacement {
    const rect = placement.rect(image, terminal) orelse return null;
    const top = terminal.screens.active.pages.pointFromPin(.screen, rect.top_left).?.screen.y;
    const bottom = terminal.screens.active.pages.pointFromPin(.screen, rect.bottom_right).?.screen.y;

    if (top > viewport_bottom or bottom < viewport_top) return null;

    const dest_size = placement.calculatedSize(image, terminal);
    if (dest_size.width == 0 or dest_size.height == 0) return null;

    const source_x = @min(image.width, placement.source_x);
    const source_y = @min(image.height, placement.source_y);
    const source_width = if (placement.source_width > 0)
        @min(image.width - source_x, placement.source_width)
    else
        image.width;
    const source_height = if (placement.source_height > 0)
        @min(image.height - source_y, placement.source_height)
    else
        image.height;

    return .{
        .image_id = image.id,
        .row = @as(i32, @intCast(top)) - @as(i32, @intCast(viewport_top)),
        .col = @intCast(rect.top_left.x),
        .z_index = placement.z,
        .offset_x = placement.x_offset,
        .offset_y = placement.y_offset,
        .source_x = source_x,
        .source_y = source_y,
        .source_width = source_width,
        .source_height = source_height,
        .dest_width = dest_size.width,
        .dest_height = dest_size.height,
    };
}

fn countVisiblePlacements(
    terminal: *const terminalpkg.Terminal,
    viewport_top: usize,
    viewport_bottom: usize,
) usize {
    var count: usize = 0;
    var it = terminal.screens.active.kitty_images.placements.iterator();
    while (it.next()) |entry| {
        const image = terminal.screens.active.kitty_images.imageById(entry.key_ptr.image_id) orelse continue;
        if (resolvePlacement(terminal, viewport_top, viewport_bottom, entry.value_ptr.*, image) == null) {
            continue;
        }
        count += 1;
    }

    return count;
}

fn writeVisiblePlacements(
    out: []ImagePlacement,
    terminal: *const terminalpkg.Terminal,
    viewport_top: usize,
    viewport_bottom: usize,
) void {
    var index: usize = 0;
    var it = terminal.screens.active.kitty_images.placements.iterator();
    while (it.next()) |entry| {
        const image = terminal.screens.active.kitty_images.imageById(entry.key_ptr.image_id) orelse continue;
        const placement = resolvePlacement(terminal, viewport_top, viewport_bottom, entry.value_ptr.*, image) orelse
            continue;
        out[index] = placement;
        index += 1;
    }
}
