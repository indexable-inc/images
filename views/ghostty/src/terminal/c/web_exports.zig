const terminal = @import("terminal.zig");
const terminal_graphics = @import("terminal_graphics.zig");
const web_terminal = @import("web_terminal.zig");

pub const terminal_new = web_terminal.new;
pub const terminal_free = terminal.free;
pub const terminal_feed = web_terminal.feed;
pub const terminal_resize = web_terminal.resize;
pub const terminal_set_pixel_size = web_terminal.setPixelSize;
pub const terminal_total_rows = web_terminal.totalRows;
pub const terminal_screen = web_terminal.screen;
pub const terminal_cursor = web_terminal.cursor;
pub const terminal_image_placements = terminal_graphics.imagePlacements;
pub const terminal_image_rgba = terminal_graphics.imageRgba;
