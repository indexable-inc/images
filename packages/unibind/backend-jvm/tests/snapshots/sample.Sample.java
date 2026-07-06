package unibind.sample;

import java.lang.foreign.Arena;
import java.lang.foreign.FunctionDescriptor;
import java.lang.foreign.Linker;
import java.lang.foreign.MemorySegment;
import java.lang.foreign.SymbolLookup;
import java.lang.foreign.ValueLayout;
import java.lang.invoke.MethodHandle;
import java.nio.charset.StandardCharsets;

/**
 * A sample boundary exercising the phase 0 surface.
 *
 * Java binding for the Rust module sample; loads the native library named by the
 * {@code unibind.sample.library} system property.
 */
public final class Sample {

    private Sample() {
    }

    private static final SymbolLookup LOOKUP = loadLibrary();
    private static final Linker LINKER = Linker.nativeLinker();
    private static final MethodHandle H_ROWS = handle(
            "unibind_jvm_sample_rows",
            FunctionDescriptor.of(
                    ValueLayout.ADDRESS,
                    ValueLayout.ADDRESS,
                    ValueLayout.JAVA_LONG,
                    ValueLayout.ADDRESS));
    private static final MethodHandle H_ROWS_FREE = handle(
            "unibind_jvm_sample_rows__free", FunctionDescriptor.ofVoid(ValueLayout.ADDRESS));
    private static final MethodHandle H_TOUCH = handle(
            "unibind_jvm_sample_touch",
            FunctionDescriptor.of(
                    ValueLayout.ADDRESS,
                    ValueLayout.ADDRESS,
                    ValueLayout.ADDRESS,
                    ValueLayout.JAVA_DOUBLE,
                    ValueLayout.ADDRESS,
                    ValueLayout.JAVA_BYTE));
    private static final MethodHandle H_TOUCH_FREE = handle(
            "unibind_jvm_sample_touch__free", FunctionDescriptor.ofVoid(ValueLayout.ADDRESS));

    static {
        MethodHandle abi = handle("unibind_jvm_sample_abi_version", FunctionDescriptor.of(ValueLayout.JAVA_INT));
        int version;
        try {
            version = (int) abi.invokeExact();
        } catch (Throwable error) {
            throw new IllegalStateException("unibind ABI probe failed", error);
        }
        if (version != 0) {
            throw new IllegalStateException(
                    "native library speaks unibind ABI " + version + "; this binding expects 0");
        }
    }

    /**
     * Fetch rows.
     *
     * Docs become docstrings.
     *
     * @param limit Unsigned in Rust; a negative value is the raw two's-complement bit pattern.
     * @param root May be null.
     */
    public static java.util.List<Row> rows(String store, long limit, String root) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment storeArg = arena.allocate(16, 8);
            encodeStr(arena, storeArg, 0, store);
            MemorySegment rootArg = arena.allocate(24, 8);
            encodeOptStr(arena, rootArg, 0, root);
            MemorySegment envelope;
            try {
                envelope = (MemorySegment) H_ROWS.invokeExact(storeArg, limit, rootArg);
            } catch (Throwable error) {
                throw new IllegalStateException("unibind downcall rows failed", error);
            }
            envelope = envelope.reinterpret(40);
            try {
                int code = envelope.get(ValueLayout.JAVA_INT, 0);
                if (code != 0) {
                    throw sampleErrorException(code, decodeStr(envelope, 8));
                }
                return decodeListRow(envelope, 24);
            } finally {
                free(H_ROWS_FREE, envelope);
            }
        }
    }

    /** Calls {@link #rows(String, long, String)} with default trailing arguments. */
    public static java.util.List<Row> rows(String store, long limit) {
        return rows(store, limit, null);
    }

    /** Calls {@link #rows(String, long, String)} with default trailing arguments. */
    public static java.util.List<Row> rows(String store) {
        return rows(store, 10L, null);
    }

    public static boolean touch(java.nio.file.Path path, byte[] data, double ratio, String note, boolean flush) {
        try (Arena arena = Arena.ofConfined()) {
            MemorySegment pathArg = arena.allocate(16, 8);
            encodePath(arena, pathArg, 0, path);
            MemorySegment dataArg = arena.allocate(16, 8);
            encodeBytes(arena, dataArg, 0, data);
            MemorySegment noteArg = arena.allocate(16, 8);
            encodeStr(arena, noteArg, 0, note);
            MemorySegment envelope;
            try {
                envelope = (MemorySegment) H_TOUCH.invokeExact(pathArg, dataArg, ratio, noteArg, (byte) (flush ? 1 : 0));
            } catch (Throwable error) {
                throw new IllegalStateException("unibind downcall touch failed", error);
            }
            envelope = envelope.reinterpret(32);
            try {
                int code = envelope.get(ValueLayout.JAVA_INT, 0);
                if (code != 0) {
                    throw unexpectedStatus(code, decodeStr(envelope, 8));
                }
                return envelope.get(ValueLayout.JAVA_BYTE, 24) != 0;
            } finally {
                free(H_TOUCH_FREE, envelope);
            }
        }
    }

    /** Calls {@link #touch(java.nio.file.Path, byte[], double, String, boolean)} with default trailing arguments. */
    public static boolean touch(java.nio.file.Path path, byte[] data, double ratio, String note) {
        return touch(path, data, ratio, note, false);
    }

    /** Calls {@link #touch(java.nio.file.Path, byte[], double, String, boolean)} with default trailing arguments. */
    public static boolean touch(java.nio.file.Path path, byte[] data, double ratio) {
        return touch(path, data, ratio, "note", false);
    }

    /** Calls {@link #touch(java.nio.file.Path, byte[], double, String, boolean)} with default trailing arguments. */
    public static boolean touch(java.nio.file.Path path, byte[] data) {
        return touch(path, data, 0.5, "note", false);
    }

    private static RuntimeException sampleErrorException(int code, String message) {
        return switch (code) {
            case 1 -> new SampleErrorException.StoreGone(message);
            case 2 -> new SampleErrorException.Invalid(message);
            default -> unexpectedStatus(code, message);
        };
    }

    private static RuntimeException unexpectedStatus(int code, String message) {
        if (code == -1) {
            return new UnibindPanicException(message);
        }
        return new IllegalStateException("unexpected unibind status " + code + ": " + message);
    }

    private static void encodeBytes(Arena arena, MemorySegment seg, long offset, byte[] value) {
        MemorySegment data = arena.allocateFrom(ValueLayout.JAVA_BYTE, value);
        seg.set(ValueLayout.ADDRESS, offset, data);
        seg.set(ValueLayout.JAVA_LONG, offset + 8, value.length);
    }

    private static void encodeOptStr(Arena arena, MemorySegment seg, long offset, String value) {
        if (value == null) {
            return;
        }
        seg.set(ValueLayout.JAVA_BYTE, offset, (byte) 1);
        encodeStr(arena, seg, offset + 8, value);
    }

    private static void encodePath(Arena arena, MemorySegment seg, long offset, java.nio.file.Path value) {
        encodeStr(arena, seg, offset, value.toString());
    }

    private static void encodeStr(Arena arena, MemorySegment seg, long offset, String value) {
        byte[] bytes = value.getBytes(StandardCharsets.UTF_8);
        MemorySegment data = arena.allocateFrom(ValueLayout.JAVA_BYTE, bytes);
        seg.set(ValueLayout.ADDRESS, offset, data);
        seg.set(ValueLayout.JAVA_LONG, offset + 8, bytes.length);
    }

    private static byte[] decodeBytes(MemorySegment seg, long offset) {
        long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);
        if (len == 0) {
            return new byte[0];
        }
        return seg.get(ValueLayout.ADDRESS, offset).reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
    }

    private static java.util.List<Row> decodeListRow(MemorySegment seg, long offset) {
        long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);
        java.util.List<Row> list = new java.util.ArrayList<>();
        if (len == 0) {
            return list;
        }
        MemorySegment data = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len * 96);
        for (long index = 0; index < len; index++) {
            list.add(decodeRow(data, index * 96));
        }
        return list;
    }

    private static java.util.List<String> decodeListStr(MemorySegment seg, long offset) {
        long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);
        java.util.List<String> list = new java.util.ArrayList<>();
        if (len == 0) {
            return list;
        }
        MemorySegment data = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len * 16);
        for (long index = 0; index < len; index++) {
            list.add(decodeStr(data, index * 16));
        }
        return list;
    }

    private static java.util.Map<String, Double> decodeMapStrF64(MemorySegment seg, long offset) {
        long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);
        java.util.Map<String, Double> map = new java.util.LinkedHashMap<>();
        if (len == 0) {
            return map;
        }
        MemorySegment data = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len * 24);
        for (long index = 0; index < len; index++) {
            map.put(decodeStr(data, index * 24), data.get(ValueLayout.JAVA_DOUBLE, index * 24 + 16));
        }
        return map;
    }

    private static java.nio.file.Path decodeOptPath(MemorySegment seg, long offset) {
        if (seg.get(ValueLayout.JAVA_BYTE, offset) == 0) {
            return null;
        }
        return decodePath(seg, offset + 8);
    }

    private static java.nio.file.Path decodePath(MemorySegment seg, long offset) {
        return java.nio.file.Path.of(decodeStr(seg, offset));
    }

    private static Row decodeRow(MemorySegment seg, long offset) {
        return new Row(
                seg.get(ValueLayout.JAVA_LONG, offset),
                decodeStr(seg, offset + 8),
                decodeListStr(seg, offset + 24),
                decodeMapStrF64(seg, offset + 40),
                decodeBytes(seg, offset + 56),
                decodeOptPath(seg, offset + 72));
    }

    private static String decodeStr(MemorySegment seg, long offset) {
        long len = seg.get(ValueLayout.JAVA_LONG, offset + 8);
        if (len == 0) {
            return "";
        }
        byte[] bytes = seg.get(ValueLayout.ADDRESS, offset).reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
        return new String(bytes, StandardCharsets.UTF_8);
    }

    private static SymbolLookup loadLibrary() {
        String library = System.getProperty("unibind.sample.library");
        if (library == null) {
            throw new IllegalStateException(
                    "set -Dunibind.sample.library=/path/to/the/native/library before using this binding");
        }
        return SymbolLookup.libraryLookup(java.nio.file.Path.of(library), Arena.global());
    }

    private static MethodHandle handle(String symbol, FunctionDescriptor descriptor) {
        MemorySegment address = LOOKUP.find(symbol)
                .orElseThrow(() -> new IllegalStateException("native library exports no " + symbol));
        return LINKER.downcallHandle(address, descriptor);
    }

    private static void free(MethodHandle handle, MemorySegment envelope) {
        try {
            handle.invokeExact(envelope);
        } catch (Throwable error) {
            throw new IllegalStateException("unibind envelope free failed", error);
        }
    }
}

