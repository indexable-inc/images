import java.nio.file.Path;
import java.util.List;
import java.util.Map;
import java.util.Objects;

/**
 * The unibind jvm conformance suite, deliberately JUnit-less: a plain
 * {@code main} whose printed checks are the conformance evidence in the CI
 * log, with no test-framework dependency to vendor into the nix sandbox.
 * Every check proves one boundary behavior of the generated
 * {@link UnibindConformanceJvm} class against the real native library
 * (see the crate docs of {@code unibind-conformance-jvm}).
 */
final class ConformanceSuite {
    private static int passed = 0;
    private static final List<String> FAILURES = new java.util.ArrayList<>();

    private ConformanceSuite() {}

    public static void main(String[] args) {
        primitivesRoundTrip();
        unsignedReinterprets();
        containersRoundTrip();
        recordsRoundTrip();
        exceptionsMapToVariantSubclasses();
        defaultsGainOverloads();
        panicsSurfaceAsPanicException();

        System.out.printf("%d passed, %d failed%n", passed, FAILURES.size());
        for (String failure : FAILURES) {
            System.out.println("FAIL " + failure);
        }
        if (!FAILURES.isEmpty()) {
            System.exit(1);
        }
    }

    private static void primitivesRoundTrip() {
        check("bool true", UnibindConformanceJvm.echoBool(true), true);
        check("bool false", UnibindConformanceJvm.echoBool(false), false);
        check("byte min", UnibindConformanceJvm.echoByte(Byte.MIN_VALUE), Byte.MIN_VALUE);
        check("short max", UnibindConformanceJvm.echoShort(Short.MAX_VALUE), Short.MAX_VALUE);
        check("int min", UnibindConformanceJvm.echoInt(Integer.MIN_VALUE), Integer.MIN_VALUE);
        check("long min", UnibindConformanceJvm.echoLong(Long.MIN_VALUE), Long.MIN_VALUE);
        check("float", UnibindConformanceJvm.echoFloat(1.5f), 1.5f);
        check("double", UnibindConformanceJvm.echoDouble(Math.PI), Math.PI);
        check("double nan", Double.isNaN(UnibindConformanceJvm.echoDouble(Double.NaN)), true);
        check("string", UnibindConformanceJvm.echoStr("héllo ☃"), "héllo ☃");
        check("string empty", UnibindConformanceJvm.echoStr(""), "");
        check(
                "path",
                UnibindConformanceJvm.echoPath(Path.of("/var/data/store")),
                Path.of("/var/data/store"));
    }

    private static void unsignedReinterprets() {
        // `u32::MAX` and `u64::MAX` cross the wire bit-for-bit, so Java
        // sees -1 at the same width.
        check("uint max as -1", UnibindConformanceJvm.echoUint(-1), -1);
        check("uint plain", UnibindConformanceJvm.echoUint(123), 123);
        check("ulong max as -1", UnibindConformanceJvm.echoUlong(-1L), -1L);
    }

    private static void containersRoundTrip() {
        check(
                "bytes",
                UnibindConformanceJvm.echoBytes(new byte[] {0, -1, 127}),
                new byte[] {0, -1, 127});
        check("bytes empty", UnibindConformanceJvm.echoBytes(new byte[0]), new byte[0]);
        check("option present", UnibindConformanceJvm.echoOption("x"), "x");
        check("option null", UnibindConformanceJvm.echoOption(null), null);
        check("option defaulted overload", UnibindConformanceJvm.echoOption(), null);
        check(
                "vec",
                UnibindConformanceJvm.echoVec(List.of(1L, -2L, Long.MAX_VALUE)),
                List.of(1L, -2L, Long.MAX_VALUE));
        check("vec empty", UnibindConformanceJvm.echoVec(List.of()), List.of());
        check(
                "map",
                UnibindConformanceJvm.echoMap(Map.of("a", 1L, "b", -2L)),
                Map.of("a", 1L, "b", -2L));
    }

    private static void recordsRoundTrip() {
        var sample =
                new UnibindConformanceJvm.Sample(7L, "seven", 0.25, List.of("t1", "t2"), "/home");
        check("record", UnibindConformanceJvm.echoRecord(sample), sample);
        var homeless = new UnibindConformanceJvm.Sample(8L, "eight", -1.5, List.of(), null);
        check("record with null option field", UnibindConformanceJvm.echoRecord(homeless), homeless);
        check(
                "records nested in a list",
                UnibindConformanceJvm.echoRecords(List.of(sample, homeless)),
                List.of(sample, homeless));
    }

    private static void exceptionsMapToVariantSubclasses() {
        check("ok result", UnibindConformanceJvm.maybeFail(false), 42L);
        checkThrows(
                "first variant subclass",
                UnibindConformanceJvm.ConformanceException.DeliberateException.class,
                "conformance deliberate failure",
                () -> UnibindConformanceJvm.maybeFail(true));
        checkThrows(
                "second variant subclass",
                UnibindConformanceJvm.ConformanceException.GoneException.class,
                "conformance gone failure",
                UnibindConformanceJvm::lost);
        // The variant subclasses share the base, so one catch handles both.
        try {
            UnibindConformanceJvm.lost();
            FAILURES.add("base class catch: no exception");
        } catch (UnibindConformanceJvm.ConformanceException expected) {
            check("base class catch", true, true);
        }
    }

    private static void defaultsGainOverloads() {
        check(
                "no defaults dropped",
                UnibindConformanceJvm.greet("world", "yo", 3),
                "yo, world!!!");
        check("last default dropped", UnibindConformanceJvm.greet("world", "hi"), "hi, world!");
        check("all defaults dropped", UnibindConformanceJvm.greet("world"), "hello, world!");
    }

    private static void panicsSurfaceAsPanicException() {
        checkThrows(
                "panic envelope",
                UnibindConformanceJvm.PanicException.class,
                "conformance deliberate panic",
                UnibindConformanceJvm::explode);
    }

    private static void check(String name, Object actual, Object expected) {
        if (Objects.deepEquals(actual, expected)) {
            System.out.println("ok " + name);
            passed++;
        } else {
            FAILURES.add(name + ": expected " + render(expected) + ", got " + render(actual));
        }
    }

    private static void checkThrows(
            String name, Class<? extends Throwable> type, String needle, Runnable body) {
        try {
            body.run();
            FAILURES.add(name + ": no exception thrown");
        } catch (RuntimeException thrown) {
            if (type.isInstance(thrown) && thrown.getMessage().contains(needle)) {
                System.out.println("ok " + name);
                passed++;
            } else {
                FAILURES.add(
                        name
                                + ": expected "
                                + type.getName()
                                + " carrying \""
                                + needle
                                + "\", got "
                                + thrown.getClass().getName()
                                + ": "
                                + thrown.getMessage());
            }
        }
    }

    private static String render(Object value) {
        if (value instanceof byte[] bytes) {
            return java.util.Arrays.toString(bytes);
        }
        return String.valueOf(value);
    }
}
