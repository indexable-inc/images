// The unibind swift-backend conformance runner.
//
// Calls every exported case through the ergonomic overlay, asserts the
// round-trip, and prints one `PASS <case>` line each plus a final summary;
// any failure flips the exit code. Async/stream/cancellation and resource
// close/leak cases are out of scope until the phase 2 IR lands (#1992).

import Foundation

var passed = 0
var failed = 0

func check(_ name: String, _ condition: Bool) {
    if condition {
        passed += 1
        print("PASS \(name)")
    } else {
        failed += 1
        print("FAIL \(name)")
    }
}

check("echo_bool", echoBool(value: true) == true && echoBool(value: false) == false)
check("echo_i8", echoI8(value: -8) == -8)
check("echo_i16", echoI16(value: -1616) == -1616)
check("echo_i32", echoI32(value: -32_000_000) == -32_000_000)
check("echo_i64", echoI64(value: -64_000_000_000) == -64_000_000_000)
check("echo_isize", echoIsize(value: -123) == -123)
check("echo_u8", echoU8(value: 250) == 250)
check("echo_u16", echoU16(value: 65_000) == 65_000)
check("echo_u32", echoU32(value: 4_000_000_000) == 4_000_000_000)
check("echo_u64", echoU64(value: 18_000_000_000_000_000_000) == 18_000_000_000_000_000_000)
check("echo_usize", echoUsize(value: 456) == 456)
check("echo_f32", echoF32(value: 1.5) == 1.5)
check("echo_f64", echoF64(value: -2.25) == -2.25)
check("echo_string", echoString(value: "héllo wörld") == "héllo wörld")
check("greet", greet(name: "swift") == "hello swift")
check("echo_path", echoPath(value: "/tmp/unibind") == "/tmp/unibind")
check("path_components", pathComponents(path: "/tmp/unibind") == 3)
check("echo_bytes", echoBytes(value: [1, 2, 3, 255]) == [1, 2, 3, 255])
check("byte_sum", byteSum(data: [1, 2, 3, 250]) == 256)
check("echo_option_i64_some", echoOptionI64(value: 41) == 41)
check("echo_option_i64_none", echoOptionI64(value: nil) == nil)
check("echo_option_i64_default", echoOptionI64() == nil)
check("echo_option_string_some", echoOptionString(value: "x") == "x")
check("echo_option_string_none", echoOptionString(value: nil) == nil)
check("echo_vec_i64", echoVecI64(value: [1, -2, 3]) == [1, -2, 3])
check("echo_vec_string", echoVecString(value: ["a", "bé", "c"]) == ["a", "bé", "c"])
check("echo_map", echoMap(value: ["a": 1, "b": -2]) == ["a": 1, "b": -2])
check(
    "echo_map_of_vec",
    echoMapOfVec(value: ["k": [1.5, 2.5], "empty": []]) == ["k": [1.5, 2.5], "empty": []]
)

let row = Row(
    id: 7,
    label: "seven",
    tags: ["odd", "prime"],
    weights: ["w": 0.5],
    blob: [7, 7],
    home: "/home/seven"
)
let echoed = echoRow(row: row)
check(
    "echo_row",
    echoed.id == 7 && echoed.label == "seven" && echoed.tags == ["odd", "prime"]
        && echoed.weights == ["w": 0.5] && echoed.blob == [7, 7] && echoed.home == "/home/seven"
)
let anonymous = Row(id: 0, label: "", tags: [], weights: [:], blob: [], home: nil)
check("echo_row_empty", echoRow(row: anonymous).home == nil)

let rows = echoRows(rows: [row, anonymous])
check("echo_rows", rows.count == 2 && rows[0].id == 7 && rows[1].label == "")
check("first_row_some", firstRow(rows: [row])?.id == 7)
check("first_row_none", firstRow(rows: []) == nil)

do {
    let value = try failIf(trigger: false, store: "s3")
    check("fail_if_ok", value == 41)
} catch {
    check("fail_if_ok", false)
}
do {
    _ = try failIf(trigger: true, store: "s3")
    check("fail_if_throws", false)
} catch let error as ConformanceError {
    if case .storeGone(let message) = error {
        check("fail_if_throws_store_gone", message == "store `s3` is gone")
    } else {
        check("fail_if_throws_store_gone", false)
    }
} catch {
    check("fail_if_throws_store_gone", false)
}
do {
    let value = try checkedDiv(dividend: 10, divisor: 2)
    check("checked_div_ok", value == 5)
} catch {
    check("checked_div_ok", false)
}
do {
    _ = try checkedDiv(dividend: 1, divisor: 0)
    check("checked_div_throws", false)
} catch let error as ConformanceError {
    if case .invalid(let message) = error {
        check("checked_div_throws_invalid", message == "invalid input: division by zero")
    } else {
        check("checked_div_throws_invalid", false)
    }
} catch {
    check("checked_div_throws_invalid", false)
}

check("repeat_defaults", `repeat`(word: "ha") == "ha ha ha")
check("repeat_explicit", `repeat`(word: "ha", count: 2, separator: "-") == "ha-ha")

print("conformance: \(passed) passed, \(failed) failed")
exit(failed == 0 ? 0 : 1)
