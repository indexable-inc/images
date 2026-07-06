package unibind.sample;

/**
 * A Rust panic crossed the unibind boundary (envelope code -1).
 */
public final class UnibindPanicException extends RuntimeException {

    public UnibindPanicException(String message) {
        super(message);
    }
}

