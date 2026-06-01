# Browser

The SDK runs in a browser. Import `@indexable/sdk` from any modern page and you get the same `Sandbox` surface as in Node.

> [!WARNING]
> WebTransport is native in Chromium and Firefox, and shipped in Safari starting 18.2 (late 2024). Older Safari versions will not work. If you need to support them, proxy through your own backend for now. We don't yet provide a fallback transport (WebSocket / WebRTC) in the browser SDK; if you need one, tell us.

## Direct-to-VM

Per-command traffic goes straight to the VM's public endpoint, no hop through our API server. Browser-native IDEs, live REPLs, and in-canvas terminals work as a thin page plus a VM, no backend required. See [network](network.md).

## Transport

All SDK RPC runs over WebTransport.

- **Same SDK everywhere.** Browser-native WebTransport where available; native module via NAPI (Node/Bun) and PyO3 (Python) elsewhere. Same wire protocol.
- **Streams + datagrams on one connection.** Reliable streams for ordered data (RPC, shell, file writes), unreliable datagrams for the rest (audio, video, frame deltas).

## Tokens

Do not ship `IX_TOKEN` to a public page. Mint a short-lived, scope-limited token server-side.

> [!CAUTION]
> Scoped token minting, per-VM ACLs, and device attestation are still landing. Treat any browser-side token as extractable. Short TTLs, tight scopes, server-side revocation.
