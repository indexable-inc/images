# Strict type-check gate for the public Python SDK (ix-sdk), part of the
# repo-wide migration to strict checking (ENG-3131).
#
# This package ships on PyPI built with setuptools (a single large
# `ix_sdk/__init__.py` plus the `_ix_sdk.pyi` native stub), so it does not go
# through `ix.buildUvApplication`/`writePythonApplication` and has no `pyChecker`
# knob to flip. Instead, `strictCheck` runs the same two gates those helpers run
# in their "zuban" mode directly over the in-repo sources:
#   - `zuban check --strict` (correctness; --strict also enables
#     disallow-untyped-defs, so missing signatures fail too), and
#   - `ruff check --select ANN` (explicit annotations: ANN001/ANN201/ANN401...).
# The invocation mirrors lib/util/writers.nix's `strictPhase` and
# lib/build/uv-application.nix's zuban branch: same `--strict
# --python-executable --python-version --platform` shape, pinned to the
# interpreter (`python`) and platform a Linux consumer sees.
#
# Wired into CI as the `sdk-python-strict` check (tests/default.nix ->
# lib/per-system.nix), so a type or annotation regression in the shipped SDK
# fails the build.
{
  ix,
  lib,
  pkgs,
  # Interpreter whose version/stdlib stubs the checks resolve against. Defaults
  # to the same `pkgs.python3` that packages/ix-sdk-python builds for (3.13,
  # matching the cp313 abi3 wheel); the SDK's pyproject requires >=3.10.
  python ? pkgs.python3,
  # Platform the strict check assumes; the SDK ships for Linux and macOS, and CI
  # gates on Linux, mirroring the writers/uv-application default.
  pythonPlatform ? "linux",
}: let
  # Only the package sources matter to the checkers; keep the filtered source
  # tight so unrelated edits (LICENSE, pyproject) do not rebuild the check.
  src = lib.fileset.toSource {
    root = ./.;
    fileset = ./ix_sdk;
  };
  pythonExecutable = lib.getExe python;
in {
  # Strict correctness + annotation gate over `ix_sdk`. Runs from the package
  # root so `zuban` discovers `ix_sdk` as a package (resolving the colocated
  # `_ix_sdk.pyi` stub for the native extension). The root is a read-only
  # `/nix/store` path, so ruff runs with `--no-cache` to avoid writing a
  # `.ruff_cache` into cwd.
  strictCheck =
    pkgs.runCommand "ix-sdk-python-strict"
    {
      inherit src;
      nativeBuildInputs = [
        pkgs.zuban
        pkgs.ruff
        python
      ];
      strictDeps = true;
      meta = {
        description = "Strict zuban + ruff ANN type check over the public ix-sdk Python sources";
        homepage = "https://github.com/indexable-inc/index";
      };
    }
    ''
      cd "$src"
      zuban check --strict \
        --python-executable ${lib.escapeShellArg pythonExecutable} \
        --python-version ${python.pythonVersion} \
        --platform ${pythonPlatform} \
        ix_sdk
      ruff check --no-cache ${ix.ruffAnnArgs} ix_sdk
      touch "$out"
    '';
}
