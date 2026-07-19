# claude-code-rainbow: proof that we can produce a *visibly customized* Claude
# Code binary through a chain of independently-cacheable Nix derivations, one
# per "mapping" (patch), without recompiling Bun.
#
# WHY this works at all (the load-bearing fact): Claude Code ships as a prebuilt
# Bun single-file executable with the minified app JS embedded as text inside
# the Mach-O/ELF, plus a trailer Bun appends. Bun does NOT fatally integrity-
# check that bundle, so an EQUAL-LENGTH, in-place byte swap leaves every offset
# (and the trailer) intact and the CLI still runs. We never rebuild Bun; we just
# rewrite bytes. See ./patch-binary.py for the two gates every rule must pass
# (equal length + a pinned occurrence count that fails loudly on version drift).
#
# Each mapping is its OWN `runCommand` derivation, folded so mapping N+1 consumes
# mapping N's output. That way every layer caches independently: editing the
# color map does not invalidate the banner layer.
{
  lib,
  ix,
  stdenv,
  fetchurl,
  runCommand,
  python3,
  autoPatchelfHook,
  darwin,
}: let
  # Single source of truth for the pin: reuse the stock package's manifest so
  # the rainbow build tracks the exact same version/hash Anthropic shipped.
  manifest = lib.importJSON ../claude-code/manifest.json;
  inherit (manifest) version;
  inherit (stdenv.hostPlatform) system;
  target =
    manifest.platforms.${system}
      or (throw "claude-code-rainbow: no prebuilt binary for ${system}");

  # The raw Bun binary, fetched directly (same bytes as the stock package's
  # `nativeBinary`). Depending on the fetch rather than the wrapped
  # `claude-code` package keeps each mapping derivation tiny: its only input is
  # the binary itself, nothing else in the wrapper closure.
  nativeBinary = fetchurl {
    urls = [
      "https://downloads.claude.ai/claude-code-releases/${version}/${target.slug}/claude"
      "https://storage.googleapis.com/claude-code-dist-86c565f3-f756-42ad-8dfa-d59b1c096819/claude-code-releases/${version}/${target.slug}/claude"
    ];
    inherit (target) hash;
  };

  patcher = ./patch-binary.py;

  # One cacheable derivation per mapping. `input` is a store path to a single
  # binary file (the fetched binary, or the previous mapping's output); `$out`
  # is likewise a single patched binary file. `dontStrip`/`dontFixup` because
  # stripping rewrites the file length and corrupts the Bun trailer.
  applyMapping = {
    name,
    mapping,
    input,
  }:
    runCommand "claude-code-rainbow-${name}"
    {
      nativeBuildInputs = [python3];
      dontStrip = true;
      dontFixup = true;
    }
    ''
      python3 ${patcher} ${input} ${mapping} $out
    '';

  # The rainbow chain. Order is irrelevant to correctness (finds and replaces
  # are disjoint), but the fold makes the caching layers explicit:
  #   nativeBinary -> banner -> colors -> final
  mappings = [
    {
      name = "banner";
      mapping = ./mappings/banner.json;
    }
    {
      name = "colors";
      mapping = ./mappings/colors.json;
    }
  ];

  patched =
    lib.foldl'
    (input: m:
      applyMapping {
        inherit (m) name mapping;
        inherit input;
      })
    nativeBinary
    mappings;
in
  # Same unfree binary as the stock package, so wrap identically:
  # `allowVendoredUnfree` strips `meta.license` so the per-system flake package
  # set (evaluated without `allowUnfree`) can still build this.
  ix.allowVendoredUnfree (stdenv.mkDerivation (finalAttrs: {
    pname = "claude-code-rainbow";
    inherit version;

    dontUnpack = true;
    # Stripping corrupts the Bun trailer; keep the patched bytes verbatim.
    dontStrip = true;
    strictDeps = true;

    # Any byte change invalidates the upstream Developer-ID signature, and
    # Apple Silicon SIGKILLs a Mach-O whose CodeDirectory hashes no longer
    # match. `autoSignDarwinBinariesHook` re-signs the patched binary ad-hoc
    # during fixup (dropping the now-meaningless hardened-runtime + Developer-ID
    # signature), which is what makes the patched CLI runnable again.
    nativeBuildInputs =
      lib.optional stdenv.hostPlatform.isElf autoPatchelfHook
      ++ lib.optional stdenv.hostPlatform.isDarwin darwin.autoSignDarwinBinariesHook;

    installPhase = ''
      runHook preInstall
      install -D -m755 ${patched} $out/bin/claude
      runHook postInstall
    '';

    # Proof gate: the patched binary must still run AND must actually contain
    # the rainbow bytes (new banner present, old banner gone, a rainbow hex
    # present). Runs natively; the prebuilt CLI only needs DISABLE_UPDATES=1 to
    # print its version offline.
    doInstallCheck = true;
    # Proof gate. The byte-level grep proof runs everywhere (in-sandbox): the
    # rainbow banner must be present, the stock banner gone, and a rainbow hex
    # present. The runtime `--version` smoke runs in-check on Linux, where the
    # raw patched binary needs no signature. On darwin we skip the in-sandbox
    # exec: AMFI refuses to launch a Mach-O that was ad-hoc re-signed inside
    # this same sandboxed build (SIGTRAP), even though the signature is valid
    # and the binary runs fine outside the sandbox. Runtime is proven
    # out-of-band there (see the report / `nix run .#claude-code-rainbow`).
    installCheckPhase =
      ''
        runHook preInstallCheck

        echo "== rainbow byte proof =="
        new=$(grep -c "Rainbow!!! Claude Code" "$out/bin/claude" || true)
        old=$(grep -c "Welcome to Claude Code" "$out/bin/claude" || true)
        hex=$(grep -c "rgb(255,20,255)" "$out/bin/claude" || true)
        echo "new banner count: $new"
        echo "old banner count: $old"
        echo "rainbow rgb count: $hex"
        [ "$new" -ge 1 ] || { echo "FAIL: new rainbow banner not present" >&2; exit 1; }
        [ "$old" -eq 0 ] || { echo "FAIL: original banner bytes still present ($old)" >&2; exit 1; }
        [ "$hex" -ge 1 ] || { echo "FAIL: rainbow rgb not present" >&2; exit 1; }
      ''
      + lib.optionalString (!stdenv.hostPlatform.isDarwin) ''
        echo "== version smoke =="
        DISABLE_UPDATES=1 $out/bin/claude --version
      ''
      + lib.optionalString stdenv.hostPlatform.isDarwin ''
        echo "== version smoke: skipped in darwin sandbox =="
        echo "AMFI blocks exec of a just-ad-hoc-signed Mach-O inside the build"
        echo "sandbox; run 'DISABLE_UPDATES=1 result/bin/claude --version' after build."
      ''
      + ''
        runHook postInstallCheck
      '';

    passthru = {
      # Each mapping layer, exposed so the derivation graph is inspectable.
      inherit nativeBinary;
      rainbowLayers = patched;
    };

    meta = {
      description = "Claude Code with a visibly rainbow'd banner and theme, patched via equal-length byte swaps";
      homepage = "https://www.anthropic.com/claude-code";
      # Stripped by `ix.allowVendoredUnfree` above; kept honest here.
      license = lib.licenses.unfree;
      mainProgram = "claude";
      platforms = builtins.attrNames manifest.platforms;
      sourceProvenance = [lib.sourceTypes.binaryNativeCode];
    };
  }))
