# The compiled `builtins.wasm` plugin for `.ix` imports: the ix2nix converter
# built for `wasm32-unknown-unknown` through a target-scoped unit graph over
# the same workspace source and lock the native graph uses (one source of
# truth; only the target, toolchain, and profile differ, which is exactly the
# unit-identity change that warrants a second `buildWorkspace`).
{
  ix,
  lib,
  repoPackages,
  ...
}: let
  inherit (ix) pkgs;

  target = "wasm32-unknown-unknown";
  workspace = ix.cargoUnit.buildWorkspace {
    pname = "ix2nix-wasm";
    inherit (ix.rustWorkspace) src;
    cargoLock.lockFile = ix.rustWorkspace.cargoLock;
    workspaceRoot = ix.rustWorkspace.root;
    cargoArgs = [
      "-p"
      "ix2nix-wasm"
    ];
    inherit target;
    # Same shape as the darwin cross graphs (lib/rust/workspace.nix): a
    # rust-overlay toolchain carrying the target's `rust-std`, pure-build
    # policy (the native graph already runs clippy/audit over these crates),
    # and input-addressed drvs so consumers substitute via plain narinfo.
    rustToolchain = ix.languages.rust.toolchain pkgs {
      channel = "stable";
      version = "latest";
      targets = [target];
    };
    # wasm32-unknown-unknown ships no unwinder; the root manifest's
    # `wasm-plugin` profile is release plus panic=abort.
    profile = "wasm-plugin";
    policy = ix.cargoUnit.policyPresets.pureBuild;
    contentAddressed = false;
  };
  unit = workspace.libraries.ix2nix_wasm;

  package =
    pkgs.runCommand "ix2nix-wasm"
    {
      strictDeps = true;
      meta = {
        description = "ix2nix compiled to Wasm for in-eval .ix imports via builtins.wasm";
        license = lib.licenses.mit;
      };
    }
    ''
      shopt -s nullglob
      artifacts=(${unit}/lib/*.wasm)
      if [ ''${#artifacts[@]} -ne 1 ]; then
        echo "expected exactly one .wasm artifact under ${unit}/lib, got ''${#artifacts[@]}" >&2
        ls -la ${unit}/lib >&2 || true
        exit 1
      fi
      install -m444 -D "''${artifacts[0]}" "$out/lib/ix2nix.wasm"
    '';

  # End-to-end over every boundary this package exists for: the patched
  # nix-ix evaluator loads the plugin (`wasm-builtin`), the shim's calling
  # convention matches the renderer's `{ __dir, __importIx, __ixTy }:`
  # wrapper, a relative `.ix` import recurses through the shim, a conversion
  # error surfaces its positioned diagnostic as a Nix eval error, and type
  # annotations check in `assert` mode and cost nothing in `erase` mode.
  # Client-side eval against a scratch store; no daemon. The crate's sibling
  # files are reached through the repo root (`../` literals are banned:
  # no-parent-path).
  crateDir = ix.paths.root + "/packages/ix2nix";

  e2e =
    pkgs.runCommand "ix2nix-wasm-e2e"
    {
      strictDeps = true;
      nativeBuildInputs = [repoPackages.nix-ix];
    }
    ''
      export HOME="$TMPDIR/home"
      export NIX_STORE_DIR="$TMPDIR/store" NIX_STATE_DIR="$TMPDIR/state" NIX_CONF_DIR="$TMPDIR/conf"
      mkdir -p "$HOME" "$NIX_CONF_DIR"

      evalIx() {
        nix eval \
          --extra-experimental-features 'nix-command wasm-builtin' \
          --impure \
          --expr "let importIx = import ${crateDir}/import-ix.nix { converter = ${package}/lib/ix2nix.wasm; typeMode = \"$2\"; }; in importIx $1"
      }

      value=$(evalIx ${crateDir + "/examples"}/main.ix assert)
      expected='"doubled: 42"'
      if [ "$value" != "$expected" ]; then
        printf 'expected: %s\nactual:   %s\n' "$expected" "$value" >&2
        exit 1
      fi

      # Type annotations: assert mode passes a well-typed module ...
      value=$(evalIx ${crateDir + "/examples"}/typed.ix assert)
      if [ "$value" != "42" ]; then
        printf 'typed.ix: expected 42, got %s\n' "$value" >&2
        exit 1
      fi
      # ... fails an ill-typed one with a positioned error naming the module ...
      if evalIx ${crateDir + "/examples"}/typed-error.ix assert 2> typed.log; then
        echo "typed-error.ix unexpectedly passed its checks" >&2
        exit 1
      fi
      grep -F 'expected int, got string' typed.log
      grep -F '3:24 argument `b`' typed.log
      # ... and erase mode evaluates the same module with the checks free.
      value=$(evalIx ${crateDir + "/examples"}/typed-error.ix erase)
      if [ "$value" != "1" ]; then
        printf 'typed-error.ix under erase: expected 1, got %s\n' "$value" >&2
        exit 1
      fi

      if evalIx ${crateDir + "/examples"}/strict-equality.ix 2> diagnostic.log; then
        echo "conversion of strict-equality.ix unexpectedly succeeded" >&2
        exit 1
      fi
      # The rendered ix2nix diagnostic (message and caret position) must reach
      # the Nix eval error verbatim.
      grep -F '`===` has no Nix equivalent; use `==`' diagnostic.log
      grep -F -- '--> 2:16' diagnostic.log

      mkdir -p "$out"
    '';
in
  package.overrideAttrs (old: {
    passthru =
      (old.passthru or {})
      // {
        tests = {inherit e2e;};
        # This wasm32 graph is its own `buildWorkspace`, invisible to
        # per-system.nix's shared-workspace `crossIfdRoots`. Every scaffolded
        # `ix init` eval forces these at eval time through
        # `lib.importIxWasm` (the `import unitsNix` behind the converter
        # drv), so cache-push publishes them as explicit roots via the
        # `workspacePackageIfdRoots` harvest -- otherwise each fresh project
        # re-vendors and re-renders the graph before its first substitution
        # (#4127; same #1890 class as codex's second workspace).
        workspaceIfdRoots = {
          inherit (workspace) unitsNix unitGraphJson vendorDir;
        };
      };
  })
