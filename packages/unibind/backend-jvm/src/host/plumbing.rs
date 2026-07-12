//! The private FFM plumbing: the linker constants and method handles, the
//! call helper marshalling one request/reply round trip, the reply-status
//! check, and the `UnibindWire`/`UnibindReader` codec classes.

use std::fmt::Write as _;

use unibind_core::ir;

use super::Uses;
use crate::names;

/// Render the plumbing members for one interface.
pub fn render(interface: &ir::Interface, uses: &Uses) -> String {
    let key = names::library_key(interface);
    let free_symbol = names::free_symbol(interface);

    let mut out = String::from(
        "private static final Linker LINKER = Linker.nativeLinker();\n\
         private static final FunctionDescriptor CALL_DESC =\n    \
         FunctionDescriptor.ofVoid(ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.ADDRESS);\n\
         private static final FunctionDescriptor FREE_DESC =\n    \
         FunctionDescriptor.ofVoid(ValueLayout.ADDRESS, ValueLayout.JAVA_LONG, ValueLayout.JAVA_LONG);\n\
         private static final SymbolLookup LIBRARY = library();\n",
    );
    let _ = writeln!(
        out,
        "private static final MethodHandle H_FREE =\n    handle(\"{free_symbol}\", FREE_DESC);"
    );
    for function in &interface.functions {
        let constant = names::handle_constant(function);
        let symbol = names::symbol(interface, function);
        let _ = writeln!(
            out,
            "private static final MethodHandle {constant} =\n    handle(\"{symbol}\", CALL_DESC);"
        );
    }
    out.push('\n');

    let _ = writeln!(
        out,
        "private static SymbolLookup library() {{\n    \
         String explicit = System.getProperty(\"unibind.library.{key}\");\n    \
         String name = explicit != null ? explicit : System.mapLibraryName(\"{key}\");\n    \
         return SymbolLookup.libraryLookup(name, Arena.global());\n\
         }}"
    );
    out.push('\n');
    out.push_str(HANDLE_AND_CALL);
    if uses.vec {
        out.push_str("\n\n");
        out.push_str(LIST_HELPERS);
    }
    if uses.map {
        out.push_str("\n\n");
        out.push_str(MAP_HELPERS);
    }
    out.push_str("\n\n");
    out.push_str(WIRE_CLASS);
    out.push_str("\n\n");
    out.push_str(READER_CLASS);
    out
}

/// The symbol-binding helper, the call round trip, and the reply-status
/// check. Fixed text; the per-interface parts are the constants above.
const HANDLE_AND_CALL: &str = r#"private static MethodHandle handle(String symbol, FunctionDescriptor descriptor) {
    MemorySegment address = LIBRARY.find(symbol)
        .orElseThrow(() -> new IllegalStateException("unibind: missing symbol " + symbol));
    return LINKER.downcallHandle(address, descriptor);
}

private static UnibindReader call(MethodHandle handle, UnibindWire args) {
    try (Arena arena = Arena.ofConfined()) {
        MemorySegment argsSegment;
        if (args.length() == 0) {
            argsSegment = MemorySegment.NULL;
        } else {
            argsSegment = arena.allocate(args.length());
            MemorySegment.copy(args.bytes(), 0, argsSegment, ValueLayout.JAVA_BYTE, 0, args.length());
        }
        MemorySegment out = arena.allocate(24, 8);
        handle.invokeExact(argsSegment, (long) args.length(), out);
        long ptr = out.get(ValueLayout.JAVA_LONG, 0);
        long len = out.get(ValueLayout.JAVA_LONG, 8);
        long cap = out.get(ValueLayout.JAVA_LONG, 16);
        byte[] reply;
        try {
            reply = len == 0
                ? new byte[0]
                : MemorySegment.ofAddress(ptr).reinterpret(len).toArray(ValueLayout.JAVA_BYTE);
        } finally {
            H_FREE.invokeExact(MemorySegment.ofAddress(ptr), len, cap);
        }
        return new UnibindReader(reply);
    } catch (RuntimeException | Error e) {
        throw e;
    } catch (Throwable t) {
        throw new IllegalStateException("unibind: native call failed", t);
    }
}

private static void expectOk(int status, UnibindReader reply) {
    if (status == 0) {
        return;
    }
    if (status == 255) {
        String message = reply.readString();
        reply.finish();
        throw new PanicException(message);
    }
    throw new IllegalStateException("unibind: unexpected reply status " + status);
}"#;

/// The `List` codec helpers; emitted only when a `Vec` crosses.
const LIST_HELPERS: &str = r"private static <T> List<T> readList(UnibindReader reader, Function<UnibindReader, T> element) {
    int count = reader.readCount();
    List<T> items = new ArrayList<>(count);
    for (int i = 0; i < count; i++) {
        items.add(element.apply(reader));
    }
    return items;
}

private static <T> void writeList(UnibindWire wire, List<T> items, BiConsumer<UnibindWire, T> element) {
    wire.writeInt(items.size());
    for (T item : items) {
        element.accept(wire, item);
    }
}";

/// The `Map` codec helpers; emitted only when a map crosses. Decode order
/// matches the wire: each entry's key strictly before its value.
const MAP_HELPERS: &str = r"private static <K, V> Map<K, V> readMap(
        UnibindReader reader, Function<UnibindReader, K> key, Function<UnibindReader, V> value) {
    int count = reader.readCount();
    Map<K, V> entries = new HashMap<>(count);
    for (int i = 0; i < count; i++) {
        K decodedKey = key.apply(reader);
        entries.put(decodedKey, value.apply(reader));
    }
    return entries;
}

private static <K, V> void writeMap(
        UnibindWire wire, Map<K, V> entries, BiConsumer<UnibindWire, K> key, BiConsumer<UnibindWire, V> value) {
    wire.writeInt(entries.size());
    for (Map.Entry<K, V> entry : entries.entrySet()) {
        key.accept(wire, entry.getKey());
        value.accept(wire, entry.getValue());
    }
}";

/// The growable little-endian request encoder.
const WIRE_CLASS: &str = r"private static final class UnibindWire {
    private byte[] buffer = new byte[32];
    private int length;

    int length() {
        return length;
    }

    byte[] bytes() {
        return buffer;
    }

    private void ensure(int extra) {
        int needed = length + extra;
        if (needed > buffer.length) {
            buffer = Arrays.copyOf(buffer, Math.max(needed, buffer.length * 2));
        }
    }

    void writeBool(boolean value) {
        writeByte(value ? (byte) 1 : (byte) 0);
    }

    void writeByte(byte value) {
        ensure(1);
        buffer[length] = value;
        length += 1;
    }

    void writeShort(short value) {
        ensure(2);
        buffer[length] = (byte) value;
        buffer[length + 1] = (byte) (value >>> 8);
        length += 2;
    }

    void writeInt(int value) {
        ensure(4);
        buffer[length] = (byte) value;
        buffer[length + 1] = (byte) (value >>> 8);
        buffer[length + 2] = (byte) (value >>> 16);
        buffer[length + 3] = (byte) (value >>> 24);
        length += 4;
    }

    void writeLong(long value) {
        writeInt((int) value);
        writeInt((int) (value >>> 32));
    }

    void writeFloat(float value) {
        writeInt(Float.floatToRawIntBits(value));
    }

    void writeDouble(double value) {
        writeLong(Double.doubleToRawLongBits(value));
    }

    void writeString(String value) {
        writeBytes(value.getBytes(StandardCharsets.UTF_8));
    }

    void writeBytes(byte[] value) {
        writeInt(value.length);
        ensure(value.length);
        System.arraycopy(value, 0, buffer, length, value.length);
        length += value.length;
    }
}";

/// The little-endian reply decoder; every read is bounds-checked, and
/// `finish()` rejects trailing bytes so a codec mismatch fails loudly.
const READER_CLASS: &str = r#"private static final class UnibindReader {
    private final byte[] buffer;
    private int position;

    UnibindReader(byte[] buffer) {
        this.buffer = buffer;
    }

    private void need(int count) {
        if (count > buffer.length - position) {
            throw new IllegalStateException("unibind: truncated reply");
        }
    }

    boolean readBool() {
        byte value = readByte();
        if (value == 0) {
            return false;
        }
        if (value == 1) {
            return true;
        }
        throw new IllegalStateException("unibind: malformed bool " + value);
    }

    byte readByte() {
        need(1);
        byte value = buffer[position];
        position += 1;
        return value;
    }

    short readShort() {
        need(2);
        short value = (short) ((buffer[position] & 0xFF) | ((buffer[position + 1] & 0xFF) << 8));
        position += 2;
        return value;
    }

    int readInt() {
        need(4);
        int value = (buffer[position] & 0xFF)
            | ((buffer[position + 1] & 0xFF) << 8)
            | ((buffer[position + 2] & 0xFF) << 16)
            | ((buffer[position + 3] & 0xFF) << 24);
        position += 4;
        return value;
    }

    long readLong() {
        long low = readInt() & 0xFFFF_FFFFL;
        long high = readInt();
        return low | (high << 32);
    }

    float readFloat() {
        return Float.intBitsToFloat(readInt());
    }

    double readDouble() {
        return Double.longBitsToDouble(readLong());
    }

    int readCount() {
        int count = readInt();
        if (count < 0) {
            throw new IllegalStateException(
                "unibind: count overflows int: " + Integer.toUnsignedString(count));
        }
        return count;
    }

    String readString() {
        return new String(readBytes(), StandardCharsets.UTF_8);
    }

    byte[] readBytes() {
        int count = readCount();
        need(count);
        byte[] value = Arrays.copyOfRange(buffer, position, position + count);
        position += count;
        return value;
    }

    void finish() {
        if (position != buffer.length) {
            throw new IllegalStateException(
                "unibind: " + (buffer.length - position) + " unread reply bytes");
        }
    }
}"#;
