/**
Hermetic smoke test for `ix.buildNpmDist` (lib/build/npm-dist.nix), exposed as
the `npm-dist-smoke` flake check. Builds a sample distribution whose
"binaries" are a bash stub, then asserts both halves of the contract:

- layout: publish order (platform packages before the wrapper), the wrapper's
  `bin` + exact-pinned `optionalDependencies` + `npmDist` config, the platform
  packages' `os`/`cpu` gating, the win32 package's `.exe` binary, and the
  unscoped naming scheme (`<name>-<platform>`);
- behavior: an npm-like install tree is assembled in the sandbox and the Node
  shim is driven end-to-end — argv and stdout forwarding, exit-code
  propagation, and the two failure modes (platform package missing because
  optional dependencies were skipped; running on a platform the dist does not
  package).

The sample keys one platform entry to the build host's `process.platform`/
`process.arch` pair so the shim's `require.resolve` finds a runnable stub, so
the same check exercises the real resolution path on every system CI evaluates.
*/
{
  buildNpmDist,
  pkgs,
  writeBashApplication,
}: let
  inherit (pkgs) lib;
  hostOs =
    if pkgs.stdenv.hostPlatform.isDarwin
    then "darwin"
    else "linux";
  hostCpu =
    if pkgs.stdenv.hostPlatform.isAarch64
    then "arm64"
    else "x64";
  hostKey = "${hostOs}-${hostCpu}";

  # Stand-in "compiled binary": echoes its argv back and exits with
  # `$STUB_EXIT`, which is exactly what the shim assertions below observe.
  stub = writeBashApplication pkgs {
    name = "npm-dist-stub";
    text = ''
      for arg in "$@"; do
        printf 'stub-argv:%s\n' "$arg"
      done
      exit "''${STUB_EXIT:-0}"
    '';
  };

  scopedSample = buildNpmDist pkgs {
    # astlog-ignore: pname-with-version (npm package coordinate data, not a derivation)
    name = "@npm-dist-smoke/demo";
    version = "1.2.3";
    description = "buildNpmDist smoke-test sample";
    license = "MIT";
    binName = "demo";
    repository = "https://github.com/indexable-inc/index";
    platforms = {
      "${hostKey}".binary = lib.getExe stub;
      win32-x64.binary = lib.getExe stub;
    };
  };

  # Unscoped variant: only the naming scheme differs, so only the layout is
  # asserted for it.
  unscopedSample = buildNpmDist pkgs {
    # astlog-ignore: pname-with-version (npm package coordinate data, not a derivation)
    name = "demo-cli";
    version = "1.2.3";
    description = "buildNpmDist smoke-test sample (unscoped)";
    license = "MIT";
    platforms = {
      "${hostKey}" = {
        binary = lib.getExe stub;
        libc = "musl";
      };
    };
  };
in
  pkgs.runCommand "npm-dist-smoke"
  {
    __structuredAttrs = true;
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.gnugrep
      pkgs.jq
      pkgs.nodejs
    ];
    inherit hostKey scopedSample unscopedSample;
  }
  ''
    fail() {
      echo "npm-dist-smoke: $1" >&2
      exit 1
    }

    wrapper="$scopedSample/packages/npm-dist-smoke-demo"
    platform="$scopedSample/packages/npm-dist-smoke-demo-$hostKey"
    win32="$scopedSample/packages/npm-dist-smoke-demo-win32-x64"

    # ── layout: publish order lists every platform package before the wrapper
    [ "$(tail -n 1 "$scopedSample/publish-order")" = "packages/npm-dist-smoke-demo" ] \
      || fail "wrapper is not last in publish-order: $(cat "$scopedSample/publish-order")"
    grep -qx "packages/npm-dist-smoke-demo-$hostKey" "$scopedSample/publish-order" \
      || fail "publish-order is missing the $hostKey platform package"

    # ── layout: wrapper manifest wiring
    [ "$(jq -r '.bin.demo' "$wrapper/package.json")" = "bin/demo.js" ] \
      || fail "wrapper bin entry is wrong"
    [ "$(jq -r ".optionalDependencies[\"@npm-dist-smoke/demo-$hostKey\"]" "$wrapper/package.json")" = "1.2.3" ] \
      || fail "wrapper does not pin the platform package at the exact version"
    [ "$(jq -r '.npmDist.binName' "$wrapper/package.json")" = "demo" ] \
      || fail "wrapper npmDist.binName is wrong"
    [ "$(jq -r ".npmDist.platforms[\"$hostKey\"]" "$wrapper/package.json")" = "@npm-dist-smoke/demo-$hostKey" ] \
      || fail "wrapper npmDist.platforms[$hostKey] is wrong"
    [ -f "$wrapper/README.md" ] || fail "wrapper README.md is missing"

    # ── layout: platform package gating + binaries
    [ "$(jq -c '.os' "$platform/package.json")" = "[\"''${hostKey%-*}\"]" ] \
      || fail "platform package os gating is wrong"
    [ "$(jq -c '.cpu' "$platform/package.json")" = "[\"''${hostKey#*-}\"]" ] \
      || fail "platform package cpu gating is wrong"
    [ -x "$platform/bin/demo" ] || fail "platform binary bin/demo is missing"
    [ "$(jq -c '.os' "$win32/package.json")" = '["win32"]' ] \
      || fail "win32 package os gating is wrong"
    [ -f "$win32/bin/demo.exe" ] || fail "win32 binary bin/demo.exe is missing"

    # ── layout: unscoped naming (`<name>-<platform>`) + libc narrowing
    [ -f "$unscopedSample/packages/demo-cli-$hostKey/bin/demo-cli" ] \
      || fail "unscoped platform package layout is wrong"
    [ "$(jq -c '.libc' "$unscopedSample/packages/demo-cli-$hostKey/package.json")" = '["musl"]' ] \
      || fail "unscoped platform package libc field is wrong"

    # ── behavior: drive the shim through an npm-like install tree
    export HOME="$TMPDIR/home"
    mkdir -p "$HOME"
    modules="$TMPDIR/install/node_modules/@npm-dist-smoke"
    mkdir -p "$modules"
    cp -R "$wrapper" "$modules/demo"
    cp -R "$platform" "$modules/demo-$hostKey"
    chmod -R u+w "$modules"
    shim="$modules/demo/bin/demo.js"

    forwarded=$(node "$shim" hello "two words")
    echo "$forwarded" | grep -qx 'stub-argv:hello' || fail "argv[0] was not forwarded: $forwarded"
    echo "$forwarded" | grep -qx 'stub-argv:two words' || fail "quoted argv was not forwarded: $forwarded"

    rc=0
    STUB_EXIT=7 node "$shim" >/dev/null || rc=$?
    [ "$rc" = 7 ] || fail "exit code was not forwarded (got $rc, want 7)"

    rm -rf "$modules/demo-$hostKey"
    rc=0
    node "$shim" >/dev/null 2>"$TMPDIR/missing.err" || rc=$?
    [ "$rc" = 1 ] || fail "missing platform package should exit 1 (got $rc)"
    grep -q 'optionalDependency' "$TMPDIR/missing.err" \
      || fail "missing platform package error does not mention optionalDependency: $(cat "$TMPDIR/missing.err")"

    jq '.npmDist.platforms = {"win32-x64": "@npm-dist-smoke/demo-win32-x64"}' \
      "$modules/demo/package.json" > "$TMPDIR/patched.json"
    mv "$TMPDIR/patched.json" "$modules/demo/package.json"
    rc=0
    node "$shim" >/dev/null 2>"$TMPDIR/unsupported.err" || rc=$?
    [ "$rc" = 1 ] || fail "unsupported platform should exit 1 (got $rc)"
    grep -q "unsupported platform $hostKey" "$TMPDIR/unsupported.err" \
      || fail "unsupported platform error is wrong: $(cat "$TMPDIR/unsupported.err")"

    mkdir -p "$out"
    echo "npm-dist smoke passed on $hostKey" > "$out/result"
  ''
