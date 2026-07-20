# ghostty patches

The surface-teardown series (index#3768), regenerated with
`nix run .#rebase-patches -- ghostty`:

- `0001` macOS: fire undo-close expiry via a main-queue GCD timer, so
  undo-close retention (upstream #7535) cannot keep closed terminals alive
  past `undo-timeout` when the run-loop timer never fires.
- `0002` termio: when the spawn-time `killpg` EPERMs on Darwin (root-owned
  `login(1)` alone in that group), hang up each direct child's current
  process group instead of ignoring the error.

Verification level: the vt build lane compiles neither `macos/Sources`
(Swift) nor `src/termio` (app-only zig), so these are verified by
`swiftc -parse`, `zig ast-check`, the patched-src canonical-form check, and
the live process-topology evidence in index#3768. Behavioral verification
needs the full app lane that issue tracks.
