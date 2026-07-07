import java.nio.file.Path;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.Objects;

import unibind.conformance.Conformance;
import unibind.conformance.ConformanceErrorException;
import unibind.conformance.Point;
import unibind.conformance.Row;
import unibind.conformance.UnibindPanicException;

/**
 * Drives every sync phase-0 conformance case through the generated Java
 * Panama binding. One {@code OK <case>} line per case; exits 1 with
 * {@code FAIL <case>: <detail>} on the first mismatch.
 */
public final class Main {

    private Main() {
    }

    private static int passed = 0;

    public static void main(String[] args) {
        check("echo_bool", () -> {
            expect(true, Conformance.echoBool(true));
            expect(false, Conformance.echoBool(false));
        });
        check("echo_int", () -> {
            expect(0L, Conformance.echoInt(0L));
            expect(-5L, Conformance.echoInt(-5L));
            expect(Long.MAX_VALUE, Conformance.echoInt(Long.MAX_VALUE));
            expect(Long.MIN_VALUE, Conformance.echoInt(Long.MIN_VALUE));
        });
        check("echo_i8", () -> {
            expect((byte) -128, Conformance.echoI8((byte) -128));
            expect((byte) 127, Conformance.echoI8((byte) 127));
        });
        check("echo_i16", () -> expect((short) -12345, Conformance.echoI16((short) -12345)));
        check("echo_i32", () -> expect(-123456789, Conformance.echoI32(-123456789)));
        check("echo_u8", () -> expect((byte) -1, Conformance.echoU8((byte) -1)));
        check("echo_u16", () -> expect((short) -1, Conformance.echoU16((short) -1)));
        check("echo_u32", () -> expect(-1, Conformance.echoU32(-1)));
        check("echo_u64", () -> expect(-1L, Conformance.echoU64(-1L)));
        check("echo_usize", () -> expect(4096L, Conformance.echoUsize(4096L)));
        check("echo_isize", () -> expect(-4096L, Conformance.echoIsize(-4096L)));
        check("echo_f32", () -> expect(1.25f, Conformance.echoF32(1.25f)));
        check("echo_float", () -> expect(-3.5, Conformance.echoFloat(-3.5)));
        check("echo_str", () -> {
            expect("h\u00e9llo \ud83c\udf0d \u65e5\u672c\u8a9e", Conformance.echoStr("h\u00e9llo \ud83c\udf0d \u65e5\u672c\u8a9e"));
            expect("", Conformance.echoStr(""));
        });
        check("str_len", () -> expect(4L, Conformance.strLen("\ud83c\udf0d")));
        check("echo_path", () -> expect(Path.of("/tmp/unibind"), Conformance.echoPath(Path.of("/tmp/unibind"))));
        check("path_join", () -> expect(Path.of("/tmp/base/child"), Conformance.pathJoin(Path.of("/tmp/base"), "child")));
        check("echo_bytes", () -> {
            expectBytes(new byte[] {1, 2, 3}, Conformance.echoBytes(new byte[] {1, 2, 3}));
            expectBytes(new byte[0], Conformance.echoBytes(new byte[0]));
        });
        check("reverse_bytes", () -> expectBytes(new byte[] {3, 2, 1}, Conformance.reverseBytes(new byte[] {1, 2, 3})));
        check("echo_option", () -> {
            expect(null, Conformance.echoOption(null));
            expect(7L, Conformance.echoOption(7L));
        });
        check("echo_option_str", () -> {
            expect(null, Conformance.echoOptionStr(null));
            expect("present", Conformance.echoOptionStr("present"));
        });
        check("echo_vec", () -> {
            expect(List.of(1L, 2L, 3L), Conformance.echoVec(List.of(1L, 2L, 3L)));
            expect(List.of(), Conformance.echoVec(List.of()));
        });
        check("echo_vec_str", () -> expect(List.of("a", "\u03b2"), Conformance.echoVecStr(List.of("a", "\u03b2"))));
        check("echo_map", () -> {
            expect(Map.of("pi", 3.14, "e", 2.71), Conformance.echoMap(Map.of("pi", 3.14, "e", 2.71)));
            expect(Map.of(), Conformance.echoMap(Map.of()));
        });
        check("echo_record", () -> expect(new Point(1.5, -2.5), Conformance.echoRecord(new Point(1.5, -2.5))));
        check("echo_row", () -> {
            expectRow(sampleRow(), Conformance.echoRow(sampleRow()));
            expectRow(bareRow(), Conformance.echoRow(bareRow()));
        });
        check("echo_rows", () -> {
            List<Row> rows = Conformance.echoRows(List.of(sampleRow(), bareRow()));
            expect(2, rows.size());
            expectRow(sampleRow(), rows.get(0));
            expectRow(bareRow(), rows.get(1));
        });
        check("make_row", () -> expectRow(
                sampleRow(),
                Conformance.makeRow(
                        7L,
                        "sample",
                        List.of("alpha", "\u03b2\u03b5\u03c4\u03b1"),
                        Map.of("recall", 0.75, "precision", 1.0),
                        new byte[] {0, -1, 42},
                        Path.of("/var/data/rows"))));
        check("add_with_default", () -> {
            expect(42L, Conformance.addWithDefault(10L));
            expect(15L, Conformance.addWithDefault(10L, 5L));
        });
        check("greet_defaults", () -> {
            expect("friend Ada x1.5!", Conformance.greet("Ada"));
            expect("friend Ada x2.5!", Conformance.greet("Ada", 2.5));
            expect("dr Ada x2.5!", Conformance.greet("Ada", 2.5, "dr"));
            expect("dr Ada x2.5.", Conformance.greet("Ada", 2.5, "dr", false));
            expect("dr Ada x2.5. (hi)", Conformance.greet("Ada", 2.5, "dr", false, "hi"));
        });
        check("throw_value_error", () -> {
            try {
                Conformance.throwValueError();
                throw new AssertionError("no exception raised");
            } catch (ConformanceErrorException.Deliberate error) {
                expect("conformance deliberate failure", error.getMessage());
            }
        });
        check("throw_missing", () -> {
            try {
                Conformance.throwMissing("gate");
                throw new AssertionError("no exception raised");
            } catch (ConformanceErrorException.Missing error) {
                expect("no such name: gate", error.getMessage());
            }
        });
        check("checked_add", () -> {
            expect(5L, Conformance.checkedAdd(2L, 3L));
            try {
                Conformance.checkedAdd(Long.MAX_VALUE, 1L);
                throw new AssertionError("no exception raised");
            } catch (ConformanceErrorException.Deliberate error) {
                expect("i64 overflow", error.getMessage());
            }
        });
        check("panic_sync", () -> {
            try {
                Conformance.panicSync();
                throw new AssertionError("no exception raised");
            } catch (UnibindPanicException error) {
                if (!error.getMessage().contains("deliberate sync panic")) {
                    throw new AssertionError("unexpected panic message: " + error.getMessage());
                }
            }
        });
        System.out.println("ALL " + passed + " CASES PASSED (java)");
    }

    private static Row sampleRow() {
        return new Row(
                7L,
                "sample",
                List.of("alpha", "\u03b2\u03b5\u03c4\u03b1"),
                Map.of("recall", 0.75, "precision", 1.0),
                new byte[] {0, -1, 42},
                Path.of("/var/data/rows"));
    }

    private static Row bareRow() {
        return new Row(0L, "", List.of(), Map.of(), new byte[0], null);
    }

    private interface Body {
        void run() throws Exception;
    }

    private static void check(String name, Body body) {
        try {
            body.run();
        } catch (Throwable error) {
            System.out.println("FAIL " + name + ": " + error);
            System.exit(1);
        }
        passed++;
        System.out.println("OK " + name);
    }

    private static void expect(Object expected, Object actual) {
        if (!Objects.equals(expected, actual)) {
            throw new AssertionError("expected " + expected + ", got " + actual);
        }
    }

    private static void expectBytes(byte[] expected, byte[] actual) {
        if (!Arrays.equals(expected, actual)) {
            throw new AssertionError(
                    "expected " + Arrays.toString(expected) + ", got " + Arrays.toString(actual));
        }
    }

    /** Field-wise comparison: the {@code blob} array makes record equality identity-based. */
    private static void expectRow(Row expected, Row actual) {
        expect(expected.id(), actual.id());
        expect(expected.label(), actual.label());
        expect(expected.tags(), actual.tags());
        expect(expected.scores(), actual.scores());
        expectBytes(expected.blob(), actual.blob());
        expect(expected.origin(), actual.origin());
    }
}
