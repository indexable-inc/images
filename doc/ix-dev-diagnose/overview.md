# ix-dev-diagnose

`packages/ix-dev-diagnose` probes `https://ix.dev/` (or any HTTPS URL) from the
caller's network path and writes a single JSON diagnostic capturing every layer
of the request: system DNS answers, one TCP/TLS/HTTP probe per resolved address,
the certificate chain with fingerprints and parsed issuers, verification results
against both the OS trust store and the Mozilla root set, response headers, and a
bounded base64 body sample (`README.md:1-21`). It exists to triage the case where
`ix.dev` works on one network but fails with a browser error like
`SEC_ERROR_UNKNOWN_ISSUER` on another: the JSON is shareable evidence.

- Crate: `ix-dev-diagnose` (workspace member, MIT, `Cargo.toml:1-7`).
- Flake output: `nix run .#ix-dev-diagnose`. Built by
  `ix.cargoUnit.selectBinaryWithTests`, `mainProgram = "ix-dev-diagnose"`
  (`default.nix:3-5`); `package.nix` sets `flake`, `inRustWorkspace`,
  `passthruTests`.

## CLI surface (`src/main.rs:35-72`)

| arg | default | effect |
| --- | --- | --- |
| `URL` (positional) | `https://ix.dev/` | target; must be `https`, else it bails (`src/main.rs:359-362`). |
| `--family <any\|ipv4\|ipv6>` | `any` | restrict probes to one address family. |
| `--connect-timeout-ms <N>` | `5000` | per-address TCP connect timeout. |
| `--read-timeout-ms <N>` | `5000` | per-address TLS/HTTP read+write timeout. |
| `--max-body-bytes <N>` | `65536` (64 KiB) | response body bytes retained in the report. |
| `--output <PATH>` | none | write the JSON report here. |
| `--pretty` | off | pretty-print the JSON. |
| `--json` | off | print the JSON report to stdout instead of the status summary. |

Default output: `success` or `failure` (plus a `diagnoses:` line and the report
path) on stdout, with the report written to `--output` or a default
`ix-dev-diagnose-<host>-<unix-ms>.json` in the cwd (`src/main.rs:282-351`,
`:316-322`). With `--json`, the JSON goes to stdout and a file is written only if
`--output` was given.

Exit code is not the signal: `main` returns `Ok` whether or not the host is
reachable, so callers must read the printed `success`/`failure` or the JSON
`summary.ok`, not `$?` (`src/main.rs:271-293`). A file write error is the only
failure that changes the exit status.

## Probe flow (`run`, `src/main.rs:353-407`)

1. Install the rustls `ring` crypto provider; parse and validate the URL
   (HTTPS-only), extract host, port (`port_or_known_default`, else 443), and
   path+query.
2. Build two trust stores: native via `rustls_native_certs::load_native_certs`
   and Mozilla via `webpki_roots::TLS_SERVER_ROOTS`, each reported as a
   `RootStoreReport { loaded, accepted, ignored, errors }`
   (`src/main.rs:409-455`). A native store with zero parsable certs becomes
   `None` (verification then reports `not_run`).
3. Resolve `host:port` through the system resolver, filter by `--family`, dedup
   and sort via a `BTreeSet` (`src/main.rs:463-481`).
4. Run one probe per address: `TcpStream::connect_timeout`, then the TLS
   handshake, then a single HTTP GET (`src/main.rs:483-537`).
5. Summarize into diagnoses (below).

## The recording verifier (`src/main.rs:1183-1238`)

TLS uses a custom `ServerCertVerifier` installed through rustls'
`dangerous().with_custom_certificate_verifier`. On `verify_server_cert` it runs
the presented chain through both the native and Mozilla `WebPkiServerVerifier`s,
records the peer certificate DERs and both `VerificationStatus`es
(`passed`/`failed`/`not_run`), and then always returns
`ServerCertVerified::assertion()` so the handshake completes regardless of trust
outcome. That is the whole point: completing the handshake lets the tool capture
the full chain and the per-store verdicts even when the certificate would
normally be rejected. Signature verification (`verify_tls12/13_signature`,
`supported_verify_schemes`) is delegated to the Mozilla verifier. ALPN is pinned
to `http/1.1` (`src/main.rs:570-574`).

## Report schema (`src/main.rs:81-244`)

`Report { command, target, trust_roots, dns, probes[], summary }`:

- `command`: tool name, `CARGO_PKG_VERSION`, start time in unix ms.
- `target`: url, host, port, path_and_query.
- `trust_roots`: the two `RootStoreReport`s.
- `dns`: `resolver: "system"`, the resolved addresses, any error.
- `probes[]`: per address, a tagged `tcp` (`connected`/`failed`), `tls`
  (`completed`/`failed`, each with `certificate_verification` against both stores
  and parsed `peer_certificates`), and `http`. Each `CertificateReport` carries a
  SHA-256 fingerprint, subject, issuer, serial, validity window, and SANs
  (DNS/IP/URI), parsed with `x509-parser` (`src/main.rs:992-1051`).
- `http`: status code/reason, headers, header byte count, completeness flags, and
  a body sample (length, SHA-256, base64) bounded by `--max-body-bytes`, plus a
  96-byte hex prefix of the raw response (`src/main.rs:209-232`, `:33`).

HTTP is hand-rolled: a `GET ... HTTP/1.1` with `Connection: close`, then framing
detection that understands `Content-Length`, `Transfer-Encoding: chunked`, the
bodyless `204`/`205`/`304` statuses, and close-delimited responses, plus `1xx`
interim-header skipping (`src/main.rs:640-943`).

## Diagnoses and the verdict (`summarize`, `src/main.rs:1053-1091`)

The summary collects a sorted set of diagnosis strings:
`dns_failed`, `dns_returned_no_addresses`,
`tcp_connect_failed_for_all_addresses`, `tls_handshake_failed`,
`certificate_verification_failed`, `certificate_unknown_issuer` (matched on the
verifier error text), `unexpected_http_status` (a code outside `200..400`),
`http_response_bytes_unavailable`, `http_response_incomplete`, and `reachable`.
`summary.ok` is true only when the set is exactly `{ reachable }`, i.e. a probe
connected, completed TLS, did not fail verification, and returned a `2xx`/`3xx`
with a complete (or read-limit-truncated) body (`src/main.rs:1144-1162`).

## Dependencies

`anyhow`, `base64`, `clap` (derive), `rustls`, `rustls-native-certs`,
`webpki-roots`, `x509-parser`, `serde`/`serde_json`, `sha2`, `url`
(`Cargo.toml:12-23`). No async runtime: probes are synchronous blocking sockets.
