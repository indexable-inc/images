const std = @import("std");
const formatter = @import("formatter.zig");
const grid_ref = @import("grid_ref.zig");
const lib = @import("../lib.zig");
const point = @import("../point.zig");
const Result = @import("result.zig").Result;
const selection = @import("selection.zig");
const size = @import("../size.zig");
const terminal = @import("terminal.zig");

const web_max_scrollback: usize = 10_000;

fn requireTerminal(
    handle: terminal.Terminal,
    comptime function_name: []const u8,
) std.meta.Child(terminal.Terminal) {
    return handle orelse @panic(function_name ++ " requires a non-null terminal handle");
}

fn clampViewportRow(
    total_rows: usize,
    visible_rows: usize,
    viewport_row: usize,
) usize {
    if (visible_rows == 0 or total_rows <= visible_rows) return 0;
    return @min(viewport_row, total_rows - visible_rows);
}

fn formatterOptions(
    selected: *const selection.CSelection,
) formatter.TerminalOptions {
    return .{
        .emit = .plain,
        .unwrap = false,
        .trim = false,
        .extra = .{
            .palette = false,
            .modes = false,
            .scrolling_region = false,
            .tabstops = false,
            .pwd = false,
            .keyboard = false,
            .screen = .{
                .cursor = false,
                .style = false,
                .hyperlink = false,
                .protection = false,
                .kitty_keyboard = false,
                .charsets = false,
            },
        },
        .selection = selected,
    };
}

fn screenPoint(x: size.CellCountInt, y: usize) point.Point.C {
    return point.Point.cval(.{
        .screen = .{
            .x = x,
            .y = @intCast(y),
        },
    });
}

pub fn new(
    allocator: ?*const lib.alloc.Allocator,
    rows: size.CellCountInt,
    cols: size.CellCountInt,
    result: *terminal.Terminal,
) callconv(lib.calling_conv) Result {
    return terminal.new(allocator, result, .{
        .cols = cols,
        .rows = rows,
        .max_scrollback = web_max_scrollback,
    });
}

pub fn feed(
    handle: terminal.Terminal,
    ptr: [*]const u8,
    len: usize,
) callconv(lib.calling_conv) Result {
    _ = requireTerminal(handle, "ghostty_terminal_feed");
    terminal.vt_write(handle, ptr, len);
    return .success;
}

pub fn resize(
    handle: terminal.Terminal,
    rows: size.CellCountInt,
    cols: size.CellCountInt,
) callconv(lib.calling_conv) Result {
    return terminal.resize(handle, cols, rows, 0, 0);
}

pub fn setPixelSize(
    handle: terminal.Terminal,
    width_px: u32,
    height_px: u32,
) callconv(lib.calling_conv) Result {
    const wrapper = requireTerminal(handle, "ghostty_terminal_set_pixel_size");
    wrapper.terminal.width_px = width_px;
    wrapper.terminal.height_px = height_px;
    return .success;
}

pub fn totalRows(handle: terminal.Terminal) callconv(lib.calling_conv) usize {
    const wrapper = requireTerminal(handle, "ghostty_terminal_total_rows");
    return wrapper.terminal.screens.active.pages.total_rows;
}

pub fn cursor(
    handle: terminal.Terminal,
    row_ptr: *u16,
    col_ptr: *u16,
) callconv(lib.calling_conv) void {
    const wrapper = requireTerminal(handle, "ghostty_terminal_cursor");
    row_ptr.* = wrapper.terminal.screens.active.cursor.y;
    col_ptr.* = wrapper.terminal.screens.active.cursor.x;
}

pub fn screen(
    handle: terminal.Terminal,
    viewport_row: usize,
    out: ?[*]u8,
    out_len: usize,
    out_written: *usize,
) callconv(lib.calling_conv) Result {
    const wrapper = requireTerminal(handle, "ghostty_terminal_screen");

    const total_rows = wrapper.terminal.screens.active.pages.total_rows;
    const visible_rows: usize = @intCast(wrapper.terminal.rows);
    const visible_cols = wrapper.terminal.cols;
    const clamped_viewport_row = clampViewportRow(total_rows, visible_rows, viewport_row);

    if (visible_rows == 0 or visible_cols == 0 or total_rows == 0) {
        out_written.* = 0;
        return .success;
    }

    const last_row = @min(total_rows - 1, clamped_viewport_row + visible_rows - 1);

    var start: grid_ref.CGridRef = .{};
    const start_result = terminal.grid_ref(
        handle,
        screenPoint(0, clamped_viewport_row),
        &start,
    );
    if (start_result != .success) return start_result;

    var end: grid_ref.CGridRef = .{};
    const end_result = terminal.grid_ref(
        handle,
        screenPoint(visible_cols - 1, last_row),
        &end,
    );
    if (end_result != .success) return end_result;

    var selected: selection.CSelection = .{
        .start = start,
        .end = end,
        .rectangle = false,
    };

    var formatter_instance: formatter.Formatter = null;
    const formatter_result = formatter.terminal_new(
        null,
        &formatter_instance,
        handle,
        formatterOptions(&selected),
    );
    if (formatter_result != .success) return formatter_result;
    defer formatter.free(formatter_instance);

    const measure_result = formatter.format_buf(
        formatter_instance,
        null,
        0,
        out_written,
    );
    switch (measure_result) {
        .success => return .success,
        .out_of_space => {},
        else => return measure_result,
    }

    if (out == null or out_len < out_written.*) return .out_of_memory;

    const write_result = formatter.format_buf(
        formatter_instance,
        out,
        out_len,
        out_written,
    );
    return switch (write_result) {
        .out_of_space => .out_of_memory,
        else => write_result,
    };
}
