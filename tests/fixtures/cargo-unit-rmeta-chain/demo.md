# Reproducible rebuild-count demo for the rmeta-stability cutoff

Run from the repo's `index/` directory (the flake that exposes `ciChecks`).
A runbook rather than a committed script: new shell is fenced (#3823,
`shell-allowlist.txt`), and these three commands are for a human following
along, not for automation.

What it shows, with real derivation counts:

1. Build the base chain (`a <- b <- c <- d` bin) on the fork toolchain with
   `policy.compiler.rmetaStability` = the full trio.
2. Build the comment-edit variant (two comment lines inserted into
   `chain-a/src/lib.rs`, shifting every line below). Count how many unit
   derivations actually build: with the flags and content addressing this
   is 1 (the edited leaf recompiles once, its output converges to the same
   store path, and every dependent's resolved derivation is already
   realized), plus the variant's source-staging derivations. Without the
   wiring it is the whole chain (leaf + 3 dependents).
3. Negative control: the signature-edit variant (a new `pub fn` in
   `chain-a`) must rebuild the full chain; interface changes are metadata
   changes.

Counting method: `nix build -v` prints one `building '/nix/store/....drv'`
line per derivation actually executed. Crate unit derivations are named
after their target: `chain_a-0.1.0` / `chain_b-0.1.0` / `chain_c-0.1.0`
(libs) and `chain-d-0.1.0` (the bin, which is its own link step). Anchoring
the pattern to the 32-character store hash prefix excludes the variant
staging (`cargo-unit-rmeta-chain-*`) and per-crate source staging
(`cargo-unit-source-chain-*-0.1.0-*`) derivations, which also mention the
crate names.

```bash
check='.#ciChecks.x86_64-linux.cargo-unit-rmeta-cutoff'

echo "== 1. base chain (cold or cache-warm; establishes the baseline paths)"
nix build --no-link "$check.baseBin"

echo "== 2. comment-edit variant: count unit builds"
nix build --no-link -v "$check.commentBin" 2>&1 \
  | grep -E "^building '/nix/store/.*\.drv'" \
  | tee /dev/stderr \
  | grep -cE "/nix/store/[a-z0-9]{32}-chain[-_][abcd]-0\.1\.0\.drv" || true

echo "== 3. negative control, signature edit: the chain must rebuild"
nix build --no-link -v "$check.signatureBin" 2>&1 \
  | grep -E "^building '/nix/store/.*\.drv'" \
  | tee /dev/stderr \
  | grep -cE "/nix/store/[a-z0-9]{32}-chain[-_][abcd]-0\.1\.0\.drv" || true
```

Interpretation: step 2 must report the leaf compile only (its output
converges, so `chain-b`, `chain-c` and the `chain-d` link never run); step
3 must report all four crates.
