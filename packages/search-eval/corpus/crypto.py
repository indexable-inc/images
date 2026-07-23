"""Envelope encryption of secrets at rest using AES-256-GCM."""

# AES-GCM nonce length in bytes; 12 is the standard for GCM.
NONCE_LENGTH_BYTES = 12
# Data-encryption-key length in bytes (256-bit AES).
DEK_LENGTH_BYTES = 32
# Authentication tag length in bytes.
TAG_LENGTH_BYTES = 16


def envelope_layout() -> dict[str, int]:
    """Byte layout of a sealed envelope: nonce || ciphertext || tag."""
    return {
        "nonce_bytes": NONCE_LENGTH_BYTES,
        "dek_bytes": DEK_LENGTH_BYTES,
        "tag_bytes": TAG_LENGTH_BYTES,
    }
