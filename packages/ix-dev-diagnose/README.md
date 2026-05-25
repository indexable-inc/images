# ix-dev-diagnose

`ix-dev-diagnose` probes `https://ix.dev/` from the caller's network path and
prints JSON for support/debugging.

```sh
nix run .#ix-dev-diagnose -- --pretty
```

The report includes local DNS answers, one TCP/TLS/HTTP probe per resolved
address, the certificate chain fingerprints and parsed issuer names, native and
Mozilla-root verification results, response headers, and a bounded base64 sample
of the response body. Share the JSON when `https://ix.dev` works for one network
but fails with browser errors such as `SEC_ERROR_UNKNOWN_ISSUER` on another.
Pass an explicit URL when the bad bytes are for a specific artifact, such as a
CLI binary path.
