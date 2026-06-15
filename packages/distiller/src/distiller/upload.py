"""Optional upload of the slice into the fleet MinIO archive bucket.

Pushing ``corpus/host=<h>/user=<u>/source=distilled_facts/{data.parquet,
_manifest.json}`` into the ``ix-history`` bucket is all it takes for the
leader's hourly fold + view reconcile to publish the facts to Mixedbread
(the fold has no source allowlist). Credentials come from the environment
(``AWS_ACCESS_KEY_ID`` / ``AWS_SECRET_ACCESS_KEY``) or an ``--env-file``
in systemd EnvironmentFile format (e.g. the indexer unit's secret file).
"""

from __future__ import annotations

import os
from pathlib import Path


def load_env_file(path: Path) -> dict[str, str]:
    env: dict[str, str] = {}
    for line in path.read_text().splitlines():
        line = line.strip()
        if not line or line.startswith("#") or "=" not in line:
            continue
        key, _, value = line.partition("=")
        env[key.strip()] = value.strip().strip('"').strip("'")
    return env


def resolve_credentials(env_file: Path | None) -> tuple[str, str]:
    env = dict(os.environ)
    if env_file is not None:
        env.update(load_env_file(env_file))
    access = env.get("AWS_ACCESS_KEY_ID") or env.get("MINIO_ROOT_USER")
    secret = env.get("AWS_SECRET_ACCESS_KEY") or env.get("MINIO_ROOT_PASSWORD")
    if not access or not secret:
        raise RuntimeError(
            "no S3 credentials: set AWS_ACCESS_KEY_ID/AWS_SECRET_ACCESS_KEY "
            "(or MINIO_ROOT_USER/MINIO_ROOT_PASSWORD), or pass --env-file"
        )
    return access, secret


def upload_slice(
    slice_dir: Path,
    endpoint: str,
    bucket: str,
    key_prefix: str,
    env_file: Path | None = None,
) -> list[str]:
    """Put data.parquet + _manifest.json under ``<key_prefix>/`` in the bucket."""
    import boto3

    access, secret = resolve_credentials(env_file)
    client = boto3.client(
        "s3",
        endpoint_url=endpoint,
        aws_access_key_id=access,
        aws_secret_access_key=secret,
        region_name="auto",
    )
    uploaded = []
    for name in ("data.parquet", "_manifest.json"):
        path = slice_dir / name
        key = f"{key_prefix.rstrip('/')}/{name}"
        client.upload_file(str(path), bucket, key)
        uploaded.append(f"s3://{bucket}/{key}")
    return uploaded
