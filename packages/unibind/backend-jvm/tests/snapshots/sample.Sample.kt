// Kotlin sugar over the Java Panama binding [Sample]; no second FFI path.
// suspend/Flow sugar lands with async IR (#2083 follow-up)
package unibind.sample

/**
 * Fetch rows.
 *
 * Docs become docstrings.
 *
 * @param limit Unsigned in Rust; a negative value is the raw two's-complement bit pattern.
 * @param root May be null.
 */
fun rows(
    store: String,
    limit: Long = 10,
    root: String? = null,
): List<Row> =
    Sample.rows(store, limit, root)

fun touch(
    path: java.nio.file.Path,
    data: ByteArray,
    ratio: Double = 0.5,
    note: String = "note",
    flush: Boolean = false,
): Boolean =
    Sample.touch(path, data, ratio, note, flush)

