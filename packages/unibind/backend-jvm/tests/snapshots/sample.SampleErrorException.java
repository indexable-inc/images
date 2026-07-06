package unibind.sample;

/**
 * Boundary failures.
 */
public class SampleErrorException extends RuntimeException {

    protected SampleErrorException(String message) {
        super(message);
    }

    /**
     * The store is gone.
     */
    public static final class StoreGone extends SampleErrorException {

        public StoreGone(String message) {
            super(message);
        }
    }

    /**
     * Bad input.
     */
    public static final class Invalid extends SampleErrorException {

        public Invalid(String message) {
            super(message);
        }
    }
}

