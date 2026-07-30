{
  ix,
  lib,
  bash,
  coreutils,
  git,
  jq,
  makeWrapper,
  symlinkJoin,
  ...
}: let
  bin = ix.cargoUnit.selectBinaryWithTests ix.rustWorkspace.units {
    binary = "submodule-sync-test";
    meta = {
      description = "Integration test for update-flake-lock.yml's direct submodule sync: gitlink and lock advance together, a current pin is a no-op, and a lost push race retries onto the new tip";
      license = lib.licenses.mit;
      mainProgram = "submodule-sync-test";
    };
  };
in
  symlinkJoin {
    name = "submodule-sync-test";
    paths = [bin];
    nativeBuildInputs = [makeWrapper];
    # `jq` and `git` are for the WORKFLOW STEP UNDER TEST, not for this harness.
    # The harness parses YAML and JSON in-process, which is why the caller no
    # longer provisions tools -- but the extracted `Bump submodules` body is
    # bash that shells out to jq 5 times and git 27 times. Dropping the
    # caller's `ensure-tools` step without replacing them here reddened
    # flake-check with `worker.sh: line 45: jq: command not found`.
    #
    # Baked rather than required from the ambient PATH so `nix run
    # .#submodule-sync-test` is hermetic: the previous arrangement worked
    # locally only because a developer machine happens to have jq.
    #
    # `bash` and `coreutils` are here for the same reason and were found the
    # same way: with a PATH holding only the harness's own needs, the step died
    # on `git-sh-setup: uname: command not found` and `worker.sh: line 79: mv:
    # command not found`. A real runner has both, so CI would not have caught
    # it -- the point of baking them is that `nix run .#submodule-sync-test`
    # now depends on nothing outside its own closure.
    #
    # Not baked, deliberately: `awk` is named in the step only in a comment
    # explaining that it avoids awk (the runner lacks it). If the step ever
    # starts using it, it fails loudly with "command not found" rather than
    # silently skipping a check. `nix` and `gh` are shadowed by the harness's
    # own stubs, so the real ones would never be reached anyway: the `nix` stub
    # is a no-op, and the `gh` stub models `pr list` / `pr create` / `pr edit`
    # and exits 1 on anything else, which is how the rolling-PR path is
    # exercised without a network or a token.
    postBuild = ''
      # shell
      wrapProgram $out/bin/submodule-sync-test \
        --prefix PATH : ${lib.makeBinPath [bash coreutils git jq]}
    '';
    inherit (bin) meta;
    inherit (bin) passthru;
  }
