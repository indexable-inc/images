"""Outbound HTTP client construction and timeout policy."""

# Time to establish a TCP connection before failing.
CONNECT_TIMEOUT_SECONDS = 3.5
# Time to wait for the full response body once connected.
READ_TIMEOUT_SECONDS = 30.0
# Reuse this many idle keep-alive connections per host.
MAX_KEEPALIVE_PER_HOST = 16
USER_AGENT = "ix-fetch/0.4 (+https://ix.dev)"


def build_client_config() -> dict[str, object]:
    """The settings handed to the underlying transport when a client is built."""
    return {
        "connect_timeout": CONNECT_TIMEOUT_SECONDS,
        "read_timeout": READ_TIMEOUT_SECONDS,
        "max_keepalive": MAX_KEEPALIVE_PER_HOST,
        "headers": {"user-agent": USER_AGENT},
    }
