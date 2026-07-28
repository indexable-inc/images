#!/usr/bin/env bash
# Stand up a local Iceberg REST catalog (apache/iceberg-rest-fixture + MinIO)
# and run lake-iceberg's ignored REST round-trip against it: the full
# reconcile/converge/tombstone/cursor cycle plus the stale-base commit leg,
# through the production RestCatalogBuilder + OpenDal S3 wiring. Tears the
# containers down on exit.
#
# Needs docker and cargo on PATH (on a bare NixOS host:
#   nix shell nixpkgs#cargo nixpkgs#rustc nixpkgs#gcc nixpkgs#pkg-config \
#     -c ./rest-fixture.sh
# ). The same LAKE_TEST_* variables point the test at R2 Data Catalog staging
# instead — see README.md.
set -euo pipefail
cd "$(dirname "$0")"

compose() { docker compose -f docker-compose.yml "$@"; }
compose up -d
trap 'compose down -v' EXIT

echo "waiting for the REST catalog on :8181 ..."
for _ in $(seq 60); do
  curl -sf http://localhost:8181/v1/config >/dev/null 2>&1 && break
  sleep 1
done
curl -sf http://localhost:8181/v1/config >/dev/null || {
  echo "the REST fixture never became healthy" >&2
  exit 1
}

echo "creating the warehouse bucket ..."
AWS_ACCESS_KEY_ID=minioadmin AWS_SECRET_ACCESS_KEY=minioadmin \
  nix run nixpkgs#awscli2 -- s3 mb s3://warehouse \
  --endpoint-url http://localhost:9000 --region us-east-1 2>/dev/null || true

echo "=== lake-iceberg: REST round-trip + stale-base commit leg ==="
LAKE_TEST_CATALOG_URI=http://localhost:8181 \
  LAKE_TEST_WAREHOUSE=s3://warehouse/ \
  LAKE_TEST_S3_ENDPOINT=http://localhost:9000 \
  LAKE_TEST_S3_REGION=us-east-1 \
  AWS_ACCESS_KEY_ID=minioadmin \
  AWS_SECRET_ACCESS_KEY=minioadmin \
  cargo test -p lake-iceberg -- --ignored rest_round_trip --nocapture

echo "REST-FIXTURE CHECKS PASS"
