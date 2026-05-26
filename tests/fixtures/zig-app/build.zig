const std = @import("std");

pub fn build(b: *std.Build) void {
    const target = b.standardTargetOptions(.{});
    const optimize = b.standardOptimizeOption(.{});

    const exe = b.addExecutable(.{
        .name = "zig-app-fixture",
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    b.installArtifact(exe);

    const lib_tests = b.addTest(.{
        .root_source_file = b.path("src/lib.zig"),
        .target = target,
        .optimize = optimize,
    });
    const run_lib_tests = b.addRunArtifact(lib_tests);
    const lib_test_step = b.step("test-lib", "Run library tests");
    lib_test_step.dependOn(&run_lib_tests.step);

    const exe_tests = b.addTest(.{
        .root_source_file = b.path("src/main.zig"),
        .target = target,
        .optimize = optimize,
    });
    const run_exe_tests = b.addRunArtifact(exe_tests);
    const exe_test_step = b.step("test-exe", "Run executable tests");
    exe_test_step.dependOn(&run_exe_tests.step);

    const test_step = b.step("test", "Run all tests");
    test_step.dependOn(lib_test_step);
    test_step.dependOn(exe_test_step);
}
