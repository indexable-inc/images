{lib}:
/**
Build-time strict type-and-annotation gate for a Python source tree.

Mirrors the `pyChecker = "zuban"` path of `ix.buildUvApplication`
(lib/build/uv-application.nix): it runs `zuban check --strict` (correctness,
including disallow-untyped-defs) plus `ruff check --select ANN` (explicit
annotations) over the sources. Factored out so the PyO3 packages -- which build
their wheel from a cdylib the rust workspace already compiled, not through
`buildUvApplication` -- enforce the same gate as a `passthru.tests` derivation
without each re-deriving the invocation.

zuban needs the third-party imports (polars, numpy, ...) resolvable, so the
check runs against a `python3.withPackages` env carrying them; the package's
own `.pyi` stubs travel with the source tree. The checker runs from inside the
copied tree because zuban resolves the package layout relative to the cwd.

Arguments:
- `pkgs`: the package set (first positional).
- `pname`: derivation name stem (`<pname>-py-strict`).
- `pythonSrc`: the Python source tree to check (e.g. a package's `python/`).
- `checkPaths`: paths under `pythonSrc` to check (default `[ "." ]`, the whole
  tree), matching `uv-application.nix`'s `typeCheckPaths` default.
- `pythonPackages`: `ps: [ ... ]` selecting the third-party deps the sources
  import, so zuban can resolve them (default none).
- `python`: the interpreter (default `pkgs.python3`).
- `pythonVersion`, `pythonPlatform`: passed through to both tools, exactly as
  `uv-application.nix` passes them to zuban (default `"3.11"` / `"linux"`).
*/
pkgs: {
  pname,
  pythonSrc,
  checkPaths ? ["."],
  pythonPackages ? (_ps: []),
  python ? pkgs.python3,
  pythonVersion ? "3.11",
  pythonPlatform ? "linux",
}: let
  checkEnv = python.withPackages pythonPackages;
  paths = lib.escapeShellArgs checkPaths;
  # Mirror uv-application.nix's zuban strictCheckArgs: --strict plus the version
  # and platform. The interpreter is the with-packages env so imports resolve.
  zubanArgs = lib.escapeShellArgs [
    "--strict"
    "--python-executable"
    "${checkEnv}/bin/python"
    "--python-version"
    pythonVersion
    "--platform"
    pythonPlatform
  ];
in
  pkgs.runCommand "${pname}-py-strict"
  {
    strictDeps = true;
    nativeBuildInputs = [
      pkgs.zuban
      pkgs.ruff
      checkEnv
    ];
    meta.description = "Strict type + annotation check (zuban --strict + ruff ANN) for ${pname}";
  }
  ''
    cp -R ${pythonSrc} src
    chmod -R u+w src
    cd src
    zuban check ${zubanArgs} ${paths}
    ruff check --select ANN ${paths}
    mkdir -p "$out"
  ''
