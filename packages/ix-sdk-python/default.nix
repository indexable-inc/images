{
  ix,
  lib,
  pkgs,
  # Match the interpreter of any consumer (ix-fleet builds on pkgs.python3).
  # The wheel is abi3 (cp313+), so a 3.13+ interpreter is required.
  python3 ? pkgs.python3,
}: let
  # Prebuilt `ix_sdk` wheels hosted on the public R2 bucket `ix-sdk-artifacts`.
  # This is the index <-> ix artifact boundary (ENG-2151): index fetches the
  # published wheel and never builds private ix source. The native `_ix_sdk`
  # cdylib is built, stripped, and scanned store-clean by ix's
  # `nix/packages/workspace-sdks.nix`, then uploaded to R2 with `wrangler`.
  #
  # The per-system URL + SRI catalog lives in the sibling pins.json (repo
  # policy: no inline hash literals), next to the consumer rather than in
  # flake.lock, so a routine SDK bump is: re-publish the wheel to R2 and edit
  # pins.json. Each URL path embeds the wheel's nix-store hash so distinct
  # builds never collide.
  #
  # Published for x86_64-linux (health-checks runner) and aarch64-darwin
  # (operators run ix-fleet on their Macs). The darwin wheel repoints its one
  # nix-store dylib (libiconv) at /usr/lib so it loads off-nix; see ix's
  # workspace-sdks.nix. Other systems fall through to the loud placeholder below.
  catalog = ix.pins.loadPins ./pins.json;

  inherit (pkgs.stdenv.hostPlatform) system;
  rawEntry = catalog.${system} or null;
  # Catch a careless bump (an http:// URL) at eval time, the same guard
  # lib/util/artifacts.nix applies to its loader catalogs; loadPins already
  # rejects a non-SRI hash.
  entry =
    if rawEntry == null
    then null
    else
      assert lib.assertMsg (lib.hasPrefix "https://" rawEntry.url)
      "ix-sdk-python: catalog entry for ${system} needs an https:// url"; rawEntry;
in
  if entry == null
  then
    # Eval-safe placeholder: `packages.<unsupported>.ix-sdk-python` still
    # evaluates (so flake eval and x86_64-linux CI are unaffected), but realizing
    # it fails loudly instead of silently guessing a wheel. Reject the fallback.
    pkgs.runCommand "ix-sdk-python-unsupported-${system}"
    {
      meta.description = "ix_sdk Python bindings (no prebuilt wheel for ${system})";
    }
    ''
      echo "ix-sdk-python: no prebuilt ix_sdk wheel published for ${system} (have x86_64-linux + aarch64-darwin)." >&2
      echo "Build + publish the wheel for this platform to the R2 bucket ix-sdk-artifacts and add it to packages/ix-sdk-python/default.nix." >&2
      exit 1
    ''
  else let
    wheel = pkgs.fetchurl {inherit (entry) url hash;};

    # `toPythonModule` stamps `pythonModule = python3` so the package composes
    # the normal way (`python3.withPackages (ps: [ ix-sdk-python ])`); without
    # it nixpkgs' `hasPythonModule` filter silently drops the package from any
    # environment. This is the repo convention (see packages/mcp).
    package = python3.pkgs.toPythonModule (
      pkgs.runCommand "ix-sdk-python-0.1.0"
      {
        inherit wheel;
        nativeBuildInputs = [python3];
        passthru = {
          inherit python3 wheel;
          inherit (python3) sitePackages;
        };
        meta = {
          description = "Prebuilt Python bindings for the ix Rust SDK (fetched from R2)";
          homepage = "https://github.com/indexable-inc/ix";
          platforms = builtins.attrNames catalog;
        };
      }
      ''
        mkdir -p "$out/${python3.sitePackages}"
        # A wheel is a zip: extract `ix_sdk/` + `ix_sdk-*.dist-info/` straight
        # into site-packages so consumers `import ix_sdk` with no shim.
        python3 -m zipfile -e "$wheel" "$out/${python3.sitePackages}/"
      ''
    );

    # The surface ix-fleet depends on, asserted once so a bad wheel fails the
    # check rather than ix-fleet at runtime.
    assertSurface = ''
      import inspect
      import ix_sdk
      assert ix_sdk.__version__, "missing __version__"
      for name in ("Client", "Group", "GroupMember"):
          assert hasattr(ix_sdk, name), f"missing ix_sdk.{name}"
      for method in ("create_group", "add_group_member", "create", "branches", "list_secrets"):
          assert hasattr(ix_sdk.Client, method), f"missing Client.{method}"
      # ix-fleet declares per-VM secrets through these create kwargs; assert the
      # packaged wheel accepts them so a stale wheel fails here, not at deploy.
      for kwarg in ("secrets", "no_default_secrets"):
          for method in ("create", "create_with_progress"):
              params = inspect.signature(getattr(ix_sdk.Client, method)).parameters
              assert kwarg in params, f"Client.{method} missing {kwarg} kwarg"
      print("ix_sdk", ix_sdk.__version__, "imported; group + lifecycle + secret surface present")
    '';

    # Import through a real `withPackages` environment, the way consumers use it,
    # so the toPythonModule wiring can't silently regress.
    importTest =
      pkgs.runCommand "ix-sdk-python-import"
      {
        pythonEnv = python3.withPackages (_: [package]);
      }
      ''
        "$pythonEnv/bin/python" - <<'PY'
        ${assertSurface}
        PY
        touch "$out"
      '';
  in
    package.overrideAttrs (old: {
      passthru =
        (old.passthru or {})
        // {
          tests =
            (old.passthru.tests or {})
            // {
              import = importTest;
            };
        };
    })
