# ix platform default runner template

The NixOS configuration behind `runs-on: ix` for pools that pin no BYO
template. `module.nix` is the single-job runner mechanism, vendored verbatim
from github.com/indexable-inc/ix-runners @ 768d545; `platform.nix` is ix's
policy layer (cache substitution, hosted-image parity packages, parallelism
pins). The runner control plane (`crates/ci/runners`) builds
`github:indexable-inc/index/<rev>?dir=ci/runner-template#ci-runner` on the
customer's machine; the JIT credential contract is
`/var/lib/ix-runner/jitconfig` (see module.nix header).

## Pinning a new template rev

The control plane consumes an exact commit, never a branch:

1. Land the change in ix `index/` (this tree is a read-only projection).
2. Wait for the projection tick (<= 15 min).
3. `gh api repos/indexable-inc/index/commits/main --jq .sha` - that sha is
   the `template_rev` to deploy (`defaultTemplateRev` in the runner-control
   nix module).

Rev-roll cost: every bump invalidates every platform-default pool's seed
lineages, so all their next jobs cold-boot and re-seed. Bump deliberately
and in quiet windows, never as a side effect of unrelated index changes.

## Lock bumps

`flake.lock` here is generated with `nix flake lock` and committed. Bump it
when GitHub deprecates the pinned runner version (the runner refuses to
register once too old) or when a platform workaround lands upstream; a lock
bump is a rev roll and pays the cost above. The root flake's
`checks.x86_64-linux.runner-template` evals these modules against the ROOT
lock's nixpkgs, so it gates module errors, not this lock's rot.
