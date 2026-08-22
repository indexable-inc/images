# ix platform default runner template

The NixOS configuration behind `runs-on: ix` for pools that pin no BYO
template. `module.nix` is the single-job runner mechanism, vendored verbatim
from github.com/indexable-inc/ix-runners @ 768d545; `platform.nix` is ix's
policy layer (cache substitution, hosted-image parity packages, parallelism
pins). The runner control plane (`crates/ci/runners`) builds
`github:indexable-inc/index/<rev>?dir=ci/runner-template#ci-runner` on the
customer's machine; the JIT credential contract is
`/var/lib/ix-runner/jitconfig` (see module.nix header).

## The `baml` attr

`#baml` is the toolchain-baked variant for baml-class pools: the same
mechanism and platform policy plus `baml.nix`, which bakes the toolchains
those repos' lanes expect preinstalled (rustup, sccache, go, node, ruby,
the musl cross gcc) and the openssl/dotnet/playwright env, ported from
ix-runners pool-mode-v2 `pools/baml/ci-runner.nix`. Use it for pools whose
jobs assume a hosted-image-like PATH instead of installing per-job; the
cost is a heavier image closure, which is why it is a separate attr and
not a default-template change. Pin procedure is unchanged - the pool's
`template_rev` selects the commit, and the attr rides the same subflake
(`...?dir=ci/runner-template#baml`).

## The `flox` attr

`#flox` is the family variant for flox-class pools (`ci/runner-template/
flox.nix`). Deliberately NOT toolchain-baked the way `#baml` is: flox runs
every CI step inside `nix develop`, so its devshell closure carries rust,
just, pre-commit and the whole bats dependency set. What the image adds is
the three things a job cannot give itself on a machine that pins
`trusted-users = ["root"]`:

1. `cache.flox.dev` as a trusted substituter. flox's root flake declares it
   through `nixConfig.extra-substituters`, but an unprivileged job cannot
   add a substituter and `--accept-flake-config` is ignored for it, so
   without this every lane source-builds flox's patched Nix and its Rust
   workspace.
2. `max-jobs = 4` (with `cores = 8`). This is the entry that cannot be moved
   into a workflow: `max-jobs` is a daemon setting, so on a
   trusted-users-root machine a job cannot lower it, and the daemon
   otherwise schedules one derivation per visible core against a guest that
   advertises ~100 vCPUs. `cores` a client can forward per build; `max-jobs`
   it cannot.
3. `util-linux` on the job PATH, which GitHub's ubuntu images ship and
   neither `module.nix` nor `platform.nix` does.

Every entry carries the lane observation that produced it; see the file.
The consuming pool is `indexable-inc/flox` (preview), whose repository also
carries a `.github/actions/ix-setup` PATH shim that exists ONLY because this
attr was unavailable to it, and which should be deleted once the pool's
`template_attr` is `flox`.

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
