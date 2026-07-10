#!/usr/bin/swift
import Foundation

let enable = CommandLine.arguments.count > 1 && CommandLine.arguments[1] == "on"

guard let bundle = CFBundleCreate(kCFAllocatorDefault,
    URL(fileURLWithPath: "/System/Library/PrivateFrameworks/UniversalAccess.framework") as CFURL) else { exit(1) }
guard let ptr = CFBundleGetFunctionPointerForName(bundle, "UAGrayscaleSetEnabled" as CFString) else { exit(1) }

typealias Func = @convention(c) (Bool) -> Void
unsafeBitCast(ptr, to: Func.self)(enable)
