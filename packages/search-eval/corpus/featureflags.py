"""Evaluate feature flags with deterministic percentage rollouts."""

import hashlib

# Salt mixed into the hash so rollouts differ per flag, not globally aligned.
ROLLOUT_SALT = "ixsvc-flags-v1"


def is_enabled(flag: str, user_id: str, rollout_percent: int) -> bool:
    """True when `user_id` falls inside `flag`'s rollout bucket (0..100).

    Hashing (flag, salt, user) gives a stable 0..99 bucket, so a user keeps the
    same answer across calls and a higher percent only ever adds users.
    """
    digest = hashlib.sha256(f"{flag}:{ROLLOUT_SALT}:{user_id}".encode("utf-8")).digest()
    bucket = digest[0] % 100
    return bucket < rollout_percent
