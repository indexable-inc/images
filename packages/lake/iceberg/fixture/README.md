# lake-iceberg REST fixture

The lake's unit suite runs against iceberg's in-memory catalog; this fixture
runs the same code against a **real REST catalog** locally — the production
`RestCatalogBuilder` + OpenDal S3 wiring, server-side metadata writes, and the
stale-base commit behavior a real server exhibits (merge, or HTTP 409 mapped
to the retryable `CatalogCommitConflicts`).

```sh
./rest-fixture.sh   # needs docker + cargo on PATH
```

Stands up `apache/iceberg-rest-fixture` + MinIO with host networking (one
`localhost` namespace, so the fixture's own metadata writes and our client
reach the same S3 endpoint — no split-horizon hostnames), creates the
warehouse bucket, and runs the ignored `rest_round_trip_via_env` test.

## The same test against R2 Data Catalog staging

```sh
LAKE_TEST_CATALOG_URI=https://catalog.cloudflarestorage.com/<account>/<bucket> \
LAKE_TEST_WAREHOUSE=<account>_<bucket> \
LAKE_TEST_CATALOG_TOKEN=<cloudflare api token> \
LAKE_TEST_S3_ENDPOINT=https://<account>.r2.cloudflarestorage.com \
LAKE_TEST_S3_REGION=auto \
AWS_ACCESS_KEY_ID=... AWS_SECRET_ACCESS_KEY=... \
cargo test -p lake-iceberg -- --ignored rest_round_trip --nocapture
```

The stale-base leg prints which behavior the backend exhibits (`MERGES` or
`CASes (409, retryable)`) — for R2 that print is a spike deliverable
(issue #752, phase 0.4). What it cannot cover locally: R2's managed
compaction interacting with the snapshot cursor, and snapshot expiration —
those need the R2 staging run.

`ensure_table` creates the table but never migrates it. A staging
`corpus.documents` created before the `version` column (field 13) was added
will fail every append with an arrow column-count error; drop the table and
let the next run recreate it.
