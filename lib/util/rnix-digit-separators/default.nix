# Patch the `rnix` crate inside a rust tool's vendored cargo dependencies so
# the tool lexes underscore digit separators in nix numeric literals
# (`1_000`, `1_000.000_1`, `2.5e1_0`) -- the dialect the patched nix lexer
# accepts (packages/nix/nix/patches/0014-libexpr-accept-underscore-digit-
# separators-in-numeri.patch). alejandra, statix, and deadnix all parse nix
# through rnix, and none of them can format or lint a tree using separators
# until their tokenizer takes them; their package dirs under packages/nix/
# apply this to the nixpkgs builds.
#
# Mechanics: nixpkgs vendors each tool's locked dependencies into a
# fixed-output vendor dir. Older vendorer generations copied it to a writable
# `$cargoDepsCopy`; the current one points cargo's `.cargo/config.toml`
# straight at the read-only store dir and exports only `$cargoDeps`. Handle
# both from `preBuild` (after every cargo-setup variant, before cargo reads a
# line of the crate): patch the writable copy when one exists, otherwise make
# our own copy of the store vendor dir, patch that, and repoint the cargo
# config -- either way no new fixed-output hash. The
# tokenizer moved across rnix releases, so the patch is selected by the
# vendored version, and an unknown version fails the build with instructions
# (a nixpkgs bump onto a new rnix minor adds a flavor here, not silence).
# The `.cargo-checksum.json` files the nixpkgs vendorers write carry no
# per-file hashes (`"files": {}`), so the edit needs no checksum rewrite; the
# guard keeps that assumption loud instead of latent.
#
# Semantics mirror the flex patch exactly: one or more `_` between two
# digits of the integer part, the fraction, or the exponent; never leading
# (`_1_000` stays an identifier) or trailing (`1_` still lexes as `1` then
# `_`). Token text keeps the separators, so a formatter passes them through
# verbatim.
tool:
tool.overrideAttrs (old: {
  preBuild =
    (old.preBuild or "")
    + ''
      if [ -n "''${cargoDepsCopy:-}" ]; then
        rnixVendor="$cargoDepsCopy"
      else
        rnixVendor="$NIX_BUILD_TOP/rnix-digit-separators-vendor"
        cp -r --reflink=auto \
          "''${cargoDeps:?rnix-digit-separators: build has neither cargoDepsCopy nor cargoDeps}" \
          "$rnixVendor"
        chmod -R u+w "$rnixVendor"
        for rnixCargoConfig in .cargo/config.toml .cargo/config; do
          if [ -f "$rnixCargoConfig" ]; then
            sed -i "s|$cargoDeps|$rnixVendor|g" "$rnixCargoConfig"
          fi
        done
      fi
      patchedRnix=0
      for rnixDir in "$rnixVendor"/rnix-*; do
        [ -d "$rnixDir" ] || continue
        version=$(basename "$rnixDir")
        case "$version" in
          rnix-0.10.*) rnixPatch=${./rnix-0.10.patch} ;;
          rnix-0.11.* | rnix-0.12.*) rnixPatch=${./rnix-0.11-0.12.patch} ;;
          rnix-0.13.* | rnix-0.14.*) rnixPatch=${./rnix-0.13-0.14.patch} ;;
          *)
            echo "rnix-digit-separators: no patch flavor for vendored $version;" >&2
            echo "add one under lib/util/rnix-digit-separators/" >&2
            exit 1
            ;;
        esac
        if grep -F '"src/tokenizer.rs"' "$rnixDir/.cargo-checksum.json" >/dev/null 2>&1; then
          echo "rnix-digit-separators: vendored $version records per-file hashes;" >&2
          echo "teach lib/util/rnix-digit-separators to rewrite .cargo-checksum.json" >&2
          exit 1
        fi
        patch --silent -p1 -d "$rnixDir" < "$rnixPatch"
        patchedRnix=1
      done
      if [ "$patchedRnix" = 0 ]; then
        echo "rnix-digit-separators: no vendored rnix-* crate found in $rnixVendor;" >&2
        echo "did ${old.pname or "the tool"} stop parsing nix with rnix?" >&2
        exit 1
      fi
    '';
})
