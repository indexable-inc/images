package unibind.sample;

/**
 * A row.
 *
 * @param id Identifier. Unsigned in Rust; a negative value is the raw two's-complement bit pattern.
 * @param name
 * @param tags
 * @param weights
 * @param blob
 * @param home May be null.
 */
public record Row(
        long id,
        String name,
        java.util.List<String> tags,
        java.util.Map<String, Double> weights,
        byte[] blob,
        java.nio.file.Path home) {
}

