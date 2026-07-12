{
  lib,
  pkgs,
}: let
  noNixStoreReferencesInputs = [
    pkgs.binutils
    pkgs.coreutils
    pkgs.file
    pkgs.findutils
    pkgs.gnugrep
    pkgs.gnutar
    pkgs.gzip
    pkgs.jq
    pkgs.llvmPackages.llvm
    pkgs.patchelf
    pkgs.removeReferencesTo
    pkgs.ripgrep
    pkgs.unzip
  ];

  scanNoNixStoreReferences = {requireStaticExecutables ? false}: ''
    scan_root=$(mktemp -d)
    store_path_regex='/nix/store/[0-9a-df-np-sv-z]{32}-'
    printf '%s' "$artifacts" | jq -r '.[] | [.name, .path] | @tsv' |
      while IFS=$'\t' read -r name path; do
        artifact_root="$scan_root/$name"
        mkdir -p "$artifact_root"

        if [ -d "$path" ]; then
          cp -R "$path/." "$artifact_root/"
        else
          case "$path" in
            *.tgz|*.tar.gz)
              tar -xzf "$path" -C "$artifact_root"
              ;;
            *.whl|*.zip)
              unzip -q "$path" -d "$artifact_root"
              ;;
            *)
              cp "$path" "$artifact_root/$(basename "$path")"
              ;;
          esac
        fi

        while IFS= read -r -d $'\0' archive; do
          unpacked="$archive.unpacked"
          mkdir -p "$unpacked"
          case "$archive" in
            *.tgz|*.tar.gz)
              tar -xzf "$archive" -C "$unpacked"
              ;;
            *.whl|*.zip)
              unzip -q "$archive" -d "$unpacked"
              ;;
          esac
        done < <(find "$artifact_root" -type f \( -name "*.tgz" -o -name "*.tar.gz" -o -name "*.whl" -o -name "*.zip" \) -print0)

        while IFS= read -r -d $'\0' candidate; do
          file_type=$(file -b "$candidate")
          if strings -a "$candidate" | rg "$store_path_regex"; then
            echo "$name contains a Nix store reference in $candidate" >&2
            exit 1
          fi

          case "$file_type" in
            *ELF*)
              if [ "${lib.boolToString requireStaticExecutables}" = "true" ]; then
                if patchelf --print-interpreter "$candidate" >/dev/null 2>&1; then
                  echo "$name must be static for public distribution, but $candidate has a dynamic interpreter" >&2
                  exit 1
                fi
                if ! echo "$file_type" | grep -Eq "static|statically"; then
                  echo "$name must be statically linked for public distribution, but $candidate is $file_type" >&2
                  exit 1
                fi
              fi
              ;;
            *Mach-O*)
              load_commands=$(llvm-objdump --macho --dylibs-used --rpaths "$candidate")
              if echo "$load_commands" | rg "$store_path_regex"; then
                echo "$name must not load or search Nix store paths for public distribution" >&2
                exit 1
              fi
              ;;
          esac
        done < <(find "$artifact_root" -type f -print0)
      done
  '';

  mkNoNixStoreReferencesCheck = {
    artifacts,
    name,
    requireStaticExecutables ? false,
  }: let
    artifactsJson = (pkgs.formats.json {}).generate "${name}-artifacts.json" artifacts;
  in
    pkgs.runCommand name
    {
      __structuredAttrs = true;
      strictDeps = true;
      nativeBuildInputs = noNixStoreReferencesInputs;
    }
    ''
      set -euo pipefail

      artifacts=$(cat ${artifactsJson})

      ${scanNoNixStoreReferences {inherit requireStaticExecutables;}}

      touch "$out"
    '';
in {
  inherit mkNoNixStoreReferencesCheck noNixStoreReferencesInputs scanNoNixStoreReferences;
}
