// Drives the sync phase-0 conformance cases through the generated Kotlin
// sugar (real default parameter values, nullable types), which delegates to
// the same Java Panama binding. One `OK <case>` line per case; exits 1 with
// `FAIL <case>: <detail>` on the first mismatch.
import java.nio.file.Path
import kotlin.system.exitProcess
import unibind.conformance.ConformanceErrorException
import unibind.conformance.Point
import unibind.conformance.Row
import unibind.conformance.UnibindPanicException
import unibind.conformance.addWithDefault
import unibind.conformance.checkedAdd
import unibind.conformance.echoBool
import unibind.conformance.echoBytes
import unibind.conformance.echoF32
import unibind.conformance.echoFloat
import unibind.conformance.echoI16
import unibind.conformance.echoI32
import unibind.conformance.echoI8
import unibind.conformance.echoInt
import unibind.conformance.echoIsize
import unibind.conformance.echoMap
import unibind.conformance.echoOption
import unibind.conformance.echoOptionStr
import unibind.conformance.echoPath
import unibind.conformance.echoRecord
import unibind.conformance.echoRow
import unibind.conformance.echoRows
import unibind.conformance.echoStr
import unibind.conformance.echoU16
import unibind.conformance.echoU32
import unibind.conformance.echoU64
import unibind.conformance.echoU8
import unibind.conformance.echoUsize
import unibind.conformance.echoVec
import unibind.conformance.echoVecStr
import unibind.conformance.greet
import unibind.conformance.makeRow
import unibind.conformance.panicSync
import unibind.conformance.pathJoin
import unibind.conformance.reverseBytes
import unibind.conformance.strLen
import unibind.conformance.throwMissing
import unibind.conformance.throwValueError

private var passed = 0

private fun check(name: String, body: () -> Unit) {
    try {
        body()
    } catch (error: Throwable) {
        println("FAIL $name: $error")
        exitProcess(1)
    }
    passed++
    println("OK $name")
}

private fun expect(expected: Any?, actual: Any?) {
    if (expected != actual) {
        throw AssertionError("expected $expected, got $actual")
    }
}

private fun expectBytes(expected: ByteArray, actual: ByteArray) {
    if (!expected.contentEquals(actual)) {
        throw AssertionError(
            "expected ${expected.contentToString()}, got ${actual.contentToString()}",
        )
    }
}

// Field-wise comparison: the `blob` array makes record equality identity-based.
private fun expectRow(expected: Row, actual: Row) {
    expect(expected.id(), actual.id())
    expect(expected.label(), actual.label())
    expect(expected.tags(), actual.tags())
    expect(expected.scores(), actual.scores())
    expectBytes(expected.blob(), actual.blob())
    expect(expected.origin(), actual.origin())
}

private fun sampleRow(): Row =
    Row(
        7L,
        "sample",
        listOf("alpha", "\u03b2\u03b5\u03c4\u03b1"),
        mapOf("recall" to 0.75, "precision" to 1.0),
        byteArrayOf(0, -1, 42),
        Path.of("/var/data/rows"),
    )

private fun bareRow(): Row = Row(0L, "", listOf(), mapOf(), ByteArray(0), null)

fun main() {
    check("echo_bool") {
        expect(true, echoBool(true))
        expect(false, echoBool(false))
    }
    check("echo_int") {
        expect(0L, echoInt(0L))
        expect(Long.MAX_VALUE, echoInt(Long.MAX_VALUE))
        expect(Long.MIN_VALUE, echoInt(Long.MIN_VALUE))
    }
    check("echo_i8") { expect((-128).toByte(), echoI8((-128).toByte())) }
    check("echo_i16") { expect((-12345).toShort(), echoI16((-12345).toShort())) }
    check("echo_i32") { expect(-123456789, echoI32(-123456789)) }
    check("echo_u8") { expect((-1).toByte(), echoU8((-1).toByte())) }
    check("echo_u16") { expect((-1).toShort(), echoU16((-1).toShort())) }
    check("echo_u32") { expect(-1, echoU32(-1)) }
    check("echo_u64") { expect(-1L, echoU64(-1L)) }
    check("echo_usize") { expect(4096L, echoUsize(4096L)) }
    check("echo_isize") { expect(-4096L, echoIsize(-4096L)) }
    check("echo_f32") { expect(1.25f, echoF32(1.25f)) }
    check("echo_float") { expect(-3.5, echoFloat(-3.5)) }
    check("echo_str") {
        expect("h\u00e9llo \uD83C\uDF0D \u65E5\u672C\u8A9E", echoStr("h\u00e9llo \uD83C\uDF0D \u65E5\u672C\u8A9E"))
        expect("", echoStr(""))
    }
    check("str_len") { expect(4L, strLen("\uD83C\uDF0D")) }
    check("echo_path") { expect(Path.of("/tmp/unibind"), echoPath(Path.of("/tmp/unibind"))) }
    check("path_join") { expect(Path.of("/tmp/base/child"), pathJoin(Path.of("/tmp/base"), "child")) }
    check("echo_bytes") {
        expectBytes(byteArrayOf(1, 2, 3), echoBytes(byteArrayOf(1, 2, 3)))
        expectBytes(ByteArray(0), echoBytes(ByteArray(0)))
    }
    check("reverse_bytes") { expectBytes(byteArrayOf(3, 2, 1), reverseBytes(byteArrayOf(1, 2, 3))) }
    check("echo_option") {
        expect(null, echoOption(null))
        expect(7L, echoOption(7L))
        expect(null, echoOption())
    }
    check("echo_option_str") {
        expect(null, echoOptionStr(null))
        expect("present", echoOptionStr("present"))
    }
    check("echo_vec") {
        expect(listOf(1L, 2L, 3L), echoVec(listOf(1L, 2L, 3L)))
        expect(listOf<Long>(), echoVec(listOf()))
    }
    check("echo_vec_str") { expect(listOf("a", "\u03b2"), echoVecStr(listOf("a", "\u03b2"))) }
    check("echo_map") {
        expect(mapOf("pi" to 3.14, "e" to 2.71), echoMap(mapOf("pi" to 3.14, "e" to 2.71)))
        expect(mapOf<String, Double>(), echoMap(mapOf()))
    }
    check("echo_record") { expect(Point(1.5, -2.5), echoRecord(Point(1.5, -2.5))) }
    check("echo_row") {
        expectRow(sampleRow(), echoRow(sampleRow()))
        expectRow(bareRow(), echoRow(bareRow()))
    }
    check("echo_rows") {
        val rows = echoRows(listOf(sampleRow(), bareRow()))
        expect(2, rows.size)
        expectRow(sampleRow(), rows[0])
        expectRow(bareRow(), rows[1])
    }
    check("make_row") {
        expectRow(
            sampleRow(),
            makeRow(
                7L,
                "sample",
                listOf("alpha", "\u03b2\u03b5\u03c4\u03b1"),
                mapOf("recall" to 0.75, "precision" to 1.0),
                byteArrayOf(0, -1, 42),
                Path.of("/var/data/rows"),
            ),
        )
        expectRow(bareRow(), makeRow(0L, "", listOf(), mapOf(), ByteArray(0), null))
    }
    check("add_with_default") {
        expect(42L, addWithDefault(10L))
        expect(15L, addWithDefault(10L, 5L))
    }
    check("greet_defaults") {
        expect("friend Ada x1.5!", greet("Ada"))
        expect("friend Ada x2.5!", greet("Ada", ratio = 2.5))
        expect("dr Ada x1.5!", greet("Ada", title = "dr"))
        expect("friend Ada x1.5.", greet("Ada", excited = false))
        expect("friend Ada x1.5! (hi)", greet("Ada", note = "hi"))
        expect("dr Ada x2.5. (hi)", greet("Ada", 2.5, "dr", false, "hi"))
    }
    check("throw_value_error") {
        try {
            throwValueError()
            throw AssertionError("no exception raised")
        } catch (error: ConformanceErrorException.Deliberate) {
            expect("conformance deliberate failure", error.message)
        }
    }
    check("throw_missing") {
        try {
            throwMissing("gate")
            throw AssertionError("no exception raised")
        } catch (error: ConformanceErrorException.Missing) {
            expect("no such name: gate", error.message)
        }
    }
    check("checked_add") {
        expect(5L, checkedAdd(2L, 3L))
        try {
            checkedAdd(Long.MAX_VALUE, 1L)
            throw AssertionError("no exception raised")
        } catch (error: ConformanceErrorException.Deliberate) {
            expect("i64 overflow", error.message)
        }
    }
    check("panic_sync") {
        try {
            panicSync()
            throw AssertionError("no exception raised")
        } catch (error: UnibindPanicException) {
            if (error.message?.contains("deliberate sync panic") != true) {
                throw AssertionError("unexpected panic message: ${error.message}")
            }
        }
    }
    println("ALL $passed CASES PASSED (kotlin)")
}
