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
  # URL path embeds the wheel's nix-store hash so distinct builds never collide.
  #
  # Published for x86_64-linux (health-checks runner) and aarch64-darwin
  # (operators run ix-fleet on their Macs). The darwin wheel repoints its one
  # nix-store dylib (libiconv) at /usr/lib so it loads off-nix; see ix's
  # workspace-sdks.nix. Other systems fall through to the loud placeholder below.
  catalog = {
    x86_64-linux = {
      url = "https://pub-559bccbc8be94bed84821cb943b580f3.r2.dev/wheel/ix-sdk/5azviwv34h5k6s63wj4pvx1y0bw5831v/ix_sdk-0.1.0-cp313-abi3-manylinux_2_34_x86_64.whl";
      hash = "sha256-jys0Rb8ZeZ7GWDIKTspuc5lt0TmkIQbBNayT0hObgQY=";
    };
    aarch64-darwin = {
      url = "https://pub-559bccbc8be94bed84821cb943b580f3.r2.dev/wheel/ix-sdk/jmj8q0gsq5lnvv8aap6j7zh874bpzqjh/ix_sdk-0.1.0-cp313-abi3-macosx_11_0_arm64.whl";
      hash = "sha256-KdfnAh0LgHcbApMUsK0x7tPrNe+ol6LIumUqfURyZn8=";
    };
  };

  inherit (pkgs.stdenv.hostPlatform) system;
  rawEntry = catalog.${system} or null;
  # Catch a careless bump (an http:// URL or a non-SRI hash) at eval time, the
  # same guard lib/util/artifacts.nix applies to its loader catalogs.
  entry =
    if rawEntry == null then
      null
    else
      assert lib.assertMsg (
        lib.hasPrefix "https://" rawEntry.url && lib.hasPrefix "sha256-" rawEntry.hash
      ) "ix-sdk-python: catalog entry for ${system} needs an https:// url and an sha256- SRI hash";
      rawEntry;
in
if entry == null then
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
else
  let
    wheel = pkgs.fetchurl { inherit (entry) url hash; };

    # `toPythonModule` stamps `pythonModule = python3` so the package composes
    # the normal way (`python3.withPackages (ps: [ ix-sdk-python ])`); without
    # it nixpkgs' `hasPythonModule` filter silently drops the package from any
    # environment. This is the repo convention (see packages/mcp).
    package = python3.pkgs.toPythonModule (
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
        ''
    );

    # The surface ix-fleet depends on, asserted once so a bad wheel fails the
    # check rather than ix-fleet at runtime.
    assertSurface = ''
      import ix_sdk
      assert ix_sdk.__version__, "missing __version__"
      for name in ("Client", "Group", "GroupMember"):
          assert hasattr(ix_sdk, name), f"missing ix_sdk.{name}"
      for method in ("create_group", "add_group_member", "create", "branches"):
          assert hasattr(ix_sdk.Client, method), f"missing Client.{method}"
      print("ix_sdk", ix_sdk.__version__, "imported; group + lifecycle surface present")
    '';

    # Import through a real `withPackages` environment, the way consumers use it,
    # so the toPythonModule wiring can't silently regress.
    importTest =
      pkgs.runCommand "ix-sdk-python-import"
        {
          pythonEnv = python3.withPackages (_: [ package ]);
        }
        ''
          "$pythonEnv/bin/python" - <<'PY'
          ${assertSurface}
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
