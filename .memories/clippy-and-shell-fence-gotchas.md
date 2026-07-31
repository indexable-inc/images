---
tldr: Clippy fails locally on darwin with E0602 on a build script; and shell-allowlist.txt only shrinks, so deleting a generated script needs its count updated
genre: memory
topic: [tooling]
handle: [cargo clippy, shell-allowlist.txt, writeBashApplication]
prior: 0.5
based_on:
  - path: shell-allowlist.txt
    blake3: d923b4533362c8d8
validated:
  - at: 2026-07-30T03:04:25Z
    by: claude-opus-4-6
    how: cargo clippy -p memories --all-targets failed E0602 on file-search build script; git push refused on shell-fence until workstation.nix count went 6->5
    ok: true
---
Two gates that fail in ways that do not name the real cause.

**Clippy is not runnable here.** `cargo clippy -p <crate> --all-targets` dies with
`E0602` (unknown lint tool) while compiling a *build script* of a dependency, so
the error points at `file-search`'s build script rather than at your code. It is
the forked-toolchain gap, not a defect in the crate under test. Do not chase it on
a Mac: let CI be the gate, and say in the PR that clippy was not run rather than
implying it passed.

**`shell-allowlist.txt` only shrinks.** It records a count per call site, e.g.
`users/andrewgazelka/profiles/workstation.nix:writeBashApplication:6`. Deleting one
generated script drops the real count to 5, and `shell-fence` then fails `nix run
.#lint` with "stale shell-allowlist.txt entries (script gone or call-site
count changed)". That is the gate working: it flags a stale entry, not a new
violation. Edit the number down in the same commit. Never add an entry.
