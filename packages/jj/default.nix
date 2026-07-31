{
  ix,
  lib,
}: let
  # The indexable-inc/jj jj megamerge (jj-src input): upstream main plus the
  # submodule series. The input is pinned by rev and never floats under the
  # fork-sync cron; see the jj-src comment in flake.nix.
  source = ix.jjSrc;

  workspace = ix.cargoUnit.buildWorkspace {
    pname = "jj";
    src = source;
    workspaceRoot = source;
    cargoLock = source + "/Cargo.lock";
    # jj-cli is the CLI crate; its `jj` bin target is the only one outside the
    # test-fakes feature, and the rest of the workspace is libraries.
    cargoArgs = [
      "-p"
      "jj-cli"
    ];
    cargoTargets = [
      [
        "-p"
        "jj-cli"
      ]
      # Our two crates are not in the `-p jj-cli` graph and so had no units to
      # lint: jj-cli reaches jj-vfs only behind its optional `fs` feature, and
      # never reaches jj-views at all, which is its own binary. A second cargo
      # execution puts them in the merged graph. Widening `cargoTargets` leaves
      # the jj-cli roots byte-identical (see the `cargoTargets` doc), so this
      # costs a unit-graph query and no rebuild of the shipped binary.
      [
        "-p"
        "jj-vfs"
        "-p"
        "jj-views"
      ]
    ];
    # The repo-owned quality gates are for crates we own; upstream jj answers
    # to its own CI.
    policy = {
      denyUnusedCrateDependencies = false;
      cargoAudit.enable = false;
      cargoMachete.enable = false;
      clippy = {
        # Clippy is the one gate that does apply, because two of this
        # workspace's members are ours. `packages` is what keeps it to those
        # two rather than adopting upstream's whole tree; cargo PACKAGE names,
        # so hyphens, not the underscored unit keys.
        packages = [
          "jj-vfs"
          "jj-views"
        ];
        # jj writes its `[workspace.lints.clippy]` at `warn` and its CI runs
        # `cargo clippy --all-features --workspace --all-targets -- -D warnings`
        # (.github/workflows/ci.yml), so upstream's real bar is that every one
        # of those is an error. Without this the manifest levels arrive as `-W`
        # and the gate could only ever fail on clippy's deny-by-default
        # correctness group -- much weaker than "our crates pass jj's lint
        # policy", which is what wiring it here is supposed to mean.
        deniedLints = ["warnings"];
      };
    };
  };
in
  workspace.binaries.jj.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        inherit workspace;
        # `ciChecks` enumerates `passthru.tests`, and the per-crate gates it
        # picks up on its own are the ROOT package's -- jj-cli, which we do not
        # own and do not gate. The jj-vfs and jj-views gates are reachable only
        # from here, so without this the flags above build nothing and the gate
        # passes by never running. Wiring the whole map is safe precisely
        # because `policy.clippy.packages` already narrowed it to our crates,
        # and it means adding a crate there is the only edit needed.
        tests =
          (old.passthru.tests or {})
          // lib.mapAttrs' (name: lib.nameValuePair "clippy-${name}") workspace.clippyByPackage;
      };
    meta =
      (old.meta or {})
      // {
        description = "Jujutsu with index's git submodule series";
        homepage = "https://github.com/jj-vcs/jj";
        license = lib.licenses.asl20;
        mainProgram = "jj";
        platforms = lib.platforms.unix;
      };
  })
