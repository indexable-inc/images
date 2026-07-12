# Per-attempt-patch closure build gates (RFC 0010 verdict A3, #2098).
#
# `upstream-pr` ships an attempt-marked patch upstream as the patch plus its
# transitive dag.json ancestors (its closure), applied standalone against the
# bare base. A closure that no longer BUILDS therefore means we would ship
# upstream a broken PR. The gate for a patch is the fork package rebuilt with
# its series restricted to exactly that closure -- the same package logic, a
# shorter series -- via `patchedSrc`'s `patchNames` argument, so the gate can
# never drift from the real build.
#
# Everything here is pure eval: `dag.json` is a committed repo file read with
# `lib.importJSON` (no IFD, no derivation outputs read at eval), and the gate
# attrset is lazy, so nothing is forced until a consumer asks for a gate.
# Gates are DELIBERATELY not flake checks: they are heavy full-package builds,
# and per-PR flake-check cost must stay flat. They surface on the opted-in
# fork package's `passthru.closureGates` and the non-schema
# `forkClosureGates.<system>.<fork>` flake output, built by the scheduled
# fork-closure-gates workflow and the `upstream-sync --open` preflight.
#
# Enumeration coherence (every intent key, hence every attempt patch, names a
# real patch file, and dag.json's node set equals the patch files) is already
# enforced by the `patch-dag-<name>` check (packages/rebase-patches/
# dag-check.nu); `closureOf` only adds a loud eval error for a missing node so
# a stale dag.json fails with the patch name, not an attribute error.
{lib}: let
  # The patch plus its transitive dag.json ancestors, in NNNN order. NNNN
  # order is a verified topological order of the DAG (the `patch-dag-<name>`
  # check's (c-topo) invariant), so sorting the closure by filename is a valid
  # application order; the recursion terminates for the same reason.
  closureOf = nodes: patch: let
    depsOf = lib.genAttrs' nodes (node: lib.nameValuePair node.patch node.deps);
    go = name:
      [name]
      ++ lib.concatMap go (
        depsOf.${name}
          or (throw "fork-closure-gates: dag.json has no node for patch ${name}; regenerate it with `nix run .#rebase-patches`")
      );
  in
    lib.naturalSort (lib.unique (go patch));
in {
  inherit closureOf;

  # The gate attrset for one fork: `patch file name -> the fork package built
  # with the series restricted to that patch's closure`, one gate per
  # attempt-marked patch. Empty unless the fork opts in (`closureGates = true`
  # in lib/fork-packages.nix), so flipping that one flag is the only switch.
  #
  #   fork     : the fork's lib/fork-packages.nix record (intent + flag).
  #   patchDir : the committed patch dir holding the series and its dag.json.
  #   mkSeries : `patchNames list -> package derivation`, the fork package's
  #              own re-instantiation (see packages/nix/nix/default.nix), so
  #              the gate reuses the real build logic instead of copying it.
  mkGates = {
    fork,
    patchDir,
    mkSeries,
  }: let
    dag = lib.importJSON (patchDir + "/dag.json");
    attempts = lib.attrNames (
      lib.filterAttrs (_: mark: (mark.upstream or "hold") == "attempt") (fork.patches or {})
    );
  in
    lib.optionalAttrs (fork.closureGates or false) (
      lib.genAttrs attempts (patch: mkSeries (closureOf dag.nodes patch))
    );
}
