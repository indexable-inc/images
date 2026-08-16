const builtin = @import("builtin");
const std = @import("std");
const terminal = @import("terminal/main.zig");
const c = terminal.c_api;

const web = @import("terminal/c/web_exports.zig");

comptime {
    if (@import("root") == @This()) {
        @import("lib_vt_exports.zig").exportAll(c, web);
    }
}

pub const std_options: std.Options = options: {
    if (builtin.target.cpu.arch.isWasm()) break :options .{
        .log_level = switch (builtin.mode) {
            .Debug => .debug,
            .ReleaseSmall => .warn,
            else => .info,
        },
        .logFn = @import("os/wasm/log.zig").log,
    };

    if (terminal.options.c_abi) break :options .{
        .logFn = @import("terminal/c/sys.zig").logFn,
    };

    break :options .{};
};

test {
    _ = terminal;
    if (comptime terminal.options.c_abi) {
        _ = c;
        _ = web;
    }
}
