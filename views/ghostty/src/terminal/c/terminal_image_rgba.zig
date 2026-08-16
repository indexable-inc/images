const std = @import("std");
const Image = @import("../main.zig").kitty.graphics.Image;

pub fn byteLen(image: Image) error{InvalidValue}!usize {
    const pixels = std.math.mul(usize, image.width, image.height) catch return error.InvalidValue;
    return std.math.mul(usize, pixels, 4) catch return error.InvalidValue;
}

pub fn write(out: []u8, image: Image) void {
    switch (image.format) {
        .gray => writeGray(out, image.data),
        .gray_alpha => writeGrayAlpha(out, image.data),
        .rgb => writeRgb(out, image.data),
        .rgba, .png => @memcpy(out, image.data),
    }
}

fn writeGray(out: []u8, data: []const u8) void {
    for (data, 0..) |gray, index| {
        const base = index * 4;
        out[base] = gray;
        out[base + 1] = gray;
        out[base + 2] = gray;
        out[base + 3] = 255;
    }
}

fn writeGrayAlpha(out: []u8, data: []const u8) void {
    var src_index: usize = 0;
    var dst_index: usize = 0;
    while (src_index < data.len) : ({
        src_index += 2;
        dst_index += 4;
    }) {
        const gray = data[src_index];
        out[dst_index] = gray;
        out[dst_index + 1] = gray;
        out[dst_index + 2] = gray;
        out[dst_index + 3] = data[src_index + 1];
    }
}

fn writeRgb(out: []u8, data: []const u8) void {
    var src_index: usize = 0;
    var dst_index: usize = 0;
    while (src_index < data.len) : ({
        src_index += 3;
        dst_index += 4;
    }) {
        out[dst_index] = data[src_index];
        out[dst_index + 1] = data[src_index + 1];
        out[dst_index + 2] = data[src_index + 2];
        out[dst_index + 3] = 255;
    }
}
