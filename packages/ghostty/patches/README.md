# ghostty patches

The surface-teardown series (index#3768), regenerated with
`nix run .#rebase-patches -- ghostty`:

- `0001` macOS: fire undo-close expiry via a main-queue GCD timer, so
  undo-close retention (upstream #7535) cannot keep closed terminals alive
  past `undo-timeout` when the run-loop timer never fires.
- `0002` termio: when the spawn-time `killpg` EPERMs on Darwin (root-owned
  `login(1)` alone in that group), hang up each direct child's current
  process group instead of ignoring the error.
- `0003` terminal: duplicate each cell's resolved OSC 8 hyperlink URI into
  the render state and expose it through the row-cells C API
  (`…_HYPERLINK_URI_LEN`/`_BUF`), so embedders (ix-term via ix-vt,
  indexable-inc/ix#8008) can render real anchors. Unlike 0001/0002 this
  patch is compiled by the vt build lane and exercised end-to-end by
  ix-vt's tests against the patched library.

Verification level: the vt build lane compiles neither `macos/Sources`
(Swift) nor `src/termio` (app-only zig), so these are verified by
`swiftc -parse`, `zig ast-check`, the patched-src canonical-form check, and
the live process-topology evidence in index#3768. Behavioral verification
needs the full app lane that issue tracks.
