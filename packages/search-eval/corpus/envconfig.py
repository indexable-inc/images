"""Load and validate service configuration from environment variables."""

import os

# Prefix every recognized variable shares, e.g. IXSVC_PORT.
ENV_PREFIX = "IXSVC_"
# Default listen port when IXSVC_PORT is unset.
DEFAULT_PORT = 8787


def load_config() -> dict[str, object]:
    """Read the recognized variables, applying defaults and basic validation."""
    port = int(os.environ.get(f"{ENV_PREFIX}PORT", DEFAULT_PORT))
    if not (1 <= port <= 65_535):
        raise ValueError(f"{ENV_PREFIX}PORT out of range: {port}")
    return {
        "port": port,
        "log_level": os.environ.get(f"{ENV_PREFIX}LOG_LEVEL", "info"),
        "region": os.environ.get(f"{ENV_PREFIX}REGION", "us-east-1"),
    }
