{
  lib,
  pkgs,
  # Match the interpreter of any consumer (ix-fleet builds on pkgs.python3).
  # The wheel is abi3 (cp313+), so a 3.13+ interpreter is required.
  python3 ? pkgs.python3,
}:

let
  # Prebuilt `ix_sdk` wheels hosted on the public R2 bucket `ix-sdk-artifacts`.
  # This is the index <-> ix artifact boundary (ENG-2151): index fetches the
  # published wheel and never builds private ix source. The native `_ix_sdk`
  # cdylib is built, stripped, and scanned store-clean by ix's
  # `nix/packages/workspace-sdks.nix`, then uploaded to R2 with `wrangler`.
  #
  # The URL + SRI live here next to the consumer rather than in flake.lock, so a
  # routine SDK bump is: re-publish the wheel to R2 and edit this catalog. Each
  # key embeds the wheel's nix-store hash so distinct builds never collide.
  #
  # Only x86_64-linux is published today; the darwin SDK wheel build path does
  # not yet exist in indexable-inc/ix (its sdks are linux-only), so a darwin
  # entry is added once that lands.
  catalog = {
    x86_64-linux = {
      url = "https://pub-c52bf5a1e3db4628aaf57fe94cb5de10.r2.dev/wheel/ix-sdk/w3mxsrmhkgvbgfg9nq9d408sa1xqfb7y/ix_sdk-0.1.0-cp313-abi3-manylinux_2_34_x86_64.whl";
      hash = "sha256-UhhiTB/vy6y2UZAx/z19KxkUKyKscxEJcPeYWVgQp0I=";
    };
  };

  inherit (pkgs.stdenv.hostPlatform) system;
  entry = catalog.${system} or null;
in
if entry == null then
  # Eval-safe placeholder: `packages.<unsupported>.ix-sdk-python` still
  # evaluates (so flake eval and x86_64-linux CI are unaffected), but realizing
  # it fails loudly instead of silently guessing a wheel. Reject the fallback.
  pkgs.runCommand "ix-sdk-python-unsupported-${system}" { } ''
    echo "ix-sdk-python: no prebuilt ix_sdk wheel published for ${system} (only x86_64-linux so far)." >&2
    echo "Build + publish the wheel for this platform to the R2 bucket ix-sdk-artifacts and add it to packages/ix-sdk-python/default.nix." >&2
    exit 1
  ''
else
  let
    wheel = pkgs.fetchurl { inherit (entry) url hash; };

    package =
      pkgs.runCommand "ix-sdk-python-0.1.0"
        {
          inherit wheel;
          nativeBuildInputs = [ python3 ];
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
        '';

    # Defends the load-bearing claim: the prebuilt cdylib actually imports under
    # index's nixpkgs interpreter, and the surface we depend on is present.
    importTest = pkgs.runCommand "ix-sdk-python-import" { nativeBuildInputs = [ python3 ]; } ''
      export PYTHONPATH="${package}/${python3.sitePackages}"
      python3 - <<'PY'
      import ix_sdk
      assert ix_sdk.__version__, "missing __version__"
      for name in ("Client", "Group", "GroupMember"):
          assert hasattr(ix_sdk, name), f"missing ix_sdk.{name}"
      for method in ("create_group", "add_group_member", "create", "branches"):
          assert hasattr(ix_sdk.Client, method), f"missing Client.{method}"
      print("ix_sdk", ix_sdk.__version__, "imported; group + lifecycle surface present")
      PY
      touch "$out"
    '';
  in
  package.overrideAttrs (old: {
    passthru = (old.passthru or { }) // {
      tests = (old.passthru.tests or { }) // {
        import = importTest;
      };
    };
  })
