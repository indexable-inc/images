"""Database connection pool sizing and checkout policy."""

# Minimum warm connections kept open even when idle.
MIN_POOL_SIZE = 4
# Hard ceiling on concurrent connections to the primary.
MAX_POOL_SIZE = 32
# Fail a checkout that waits longer than this for a free connection.
CHECKOUT_TIMEOUT_SECONDS = 5.0
# Recycle a connection after this many seconds to dodge stale TCP state.
MAX_CONNECTION_LIFETIME_SECONDS = 1_800


def pool_settings() -> dict[str, float]:
    return {
        "min_size": MIN_POOL_SIZE,
        "max_size": MAX_POOL_SIZE,
        "checkout_timeout": CHECKOUT_TIMEOUT_SECONDS,
        "max_lifetime": MAX_CONNECTION_LIFETIME_SECONDS,
    }
