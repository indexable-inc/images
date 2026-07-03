{
  uvLockFor,
  # Shared ruff selector (ANN explicit-annotations + TID251 no-typing.cast),
  # injected by lib/default.nix so every Python gate enforces the same policy.
  ruffAnnArgs,
}:
/**
Build a Python application from a uv project.

Dependency hashes come from `uv.lock`, so callers update dependencies with
`uv lock` and do not maintain a separate Nix dependency hash. Locked
distributions are fetched into a wheelhouse, installed offline into a virtual
environment, and the local project is built as a wheel before installation.
Type checking runs by default with `zuban check --strict` plus `ruff check
--select ANN` after install against the installed virtual environment, matching
`writePythonApplication`. Set `pyChecker` to "ty" or "mypy" to switch.

The default path supports registry packages with `wheels` or `sdist` entries
in `uv.lock`. Projects that use a non-uv build backend may need to pass a
`python` with that backend available and add `--no-build-isolation` through
`buildFlags`.

Arguments:
- `pname`, `version`: derivation identity.
- `src`: project root containing `pyproject.toml` and `uv.lock`.
- `python`: Python interpreter used for the virtual environment.
- `mainProgram`: executable to expose under `$out/bin`.
- `groups`, `extras`: uv dependency groups and extras to install.
- `dev`, `allGroups`, `allExtras`: dependency selection shortcuts.
- `exportFlags`, `pipInstallFlags`, `buildFlags`: extra uv flags.
- `check`, `pyChecker`, `pythonPlatform`, `typeCheckPaths`, `typeCheckArgs`:
  type-check knobs. `pyChecker` is "zuban" (default), "ty" (legacy), or "mypy";
  "zuban"/"mypy" run that checker `--strict` plus `ruff check --select ANN`.
- `extraNativeBuildInputs`: extra packages on PATH for the build.
- `runtimeLibraryInputs`: shared libraries made visible to binary wheels.
- `fetcherOpts`: per-package fetcher overrides for locked distributions.
- `meta`: standard derivation meta.
*/
pkgs: {
  pname,
  version ? "0.0.0",
  # Pass `srcRoot = ./.` for a standard uv project (pyproject.toml + src/ +
  # uv.lock at the root); pass `src` directly to provide a custom fileset.
  srcRoot ? null,
  src ? let
    inherit (pkgs) lib;
  in
    if srcRoot != null
    then
      lib.fileset.toSource {
        root = srcRoot;
        fileset = lib.fileset.unions [
          (srcRoot + "/pyproject.toml")
          (srcRoot + "/src")
          (srcRoot + "/uv.lock")
        ];
      }
    else throw "buildUvApplication: pass `srcRoot` for a standard uv project layout or `src` for a custom one",
  python ? pkgs.python3,
  mainProgram ? pname,
  groups ? [],
  dependencyGroups ? groups,
  extras ? [],
  dev ? false,
  allGroups ? false,
  allExtras ? false,
  exportFlags ? [],
  pipInstallFlags ? [],
  buildFlags ? [],
  check ? true,
  # Build-time Python checker, one of "zuban" (default) | "ty" | "mypy".
  # "zuban"/"mypy" run that checker in `--strict` mode (correctness) plus
  # `ruff check --select ANN` (explicit annotations); "ty" is the older gradual
  # checker, kept selectable for a package with a deliberate reason. Switching a
  # package between checkers is a one-word change here.
  pyChecker ? "zuban",
  pythonPlatform ? "linux",
  typeCheckPaths ? ["."],
  extraPaths ? [],
  typeCheckArgs ? [],
  extraNativeBuildInputs ? [],
  runtimeLibraryInputs ? [],
  fetcherOpts ? {},
  meta ? {},
}: let
  inherit (pkgs) lib;

  uvLock = uvLockFor pkgs;
  uvWheelhouse = uvLock.buildWheelhouse {
    uvRoot = src;
    inherit fetcherOpts python;
  };
  pythonExecutable = lib.getExe python;
  groupFlags =
    lib.concatMap (group: [
      "--group"
      group
    ])
    dependencyGroups;
  extraFlags =
    lib.concatMap (extra: [
      "--extra"
      extra
    ])
    extras;
  extraSearchPathArgs =
    lib.concatMap (path: [
      "--extra-search-path"
      path
    ])
    extraPaths;
  tyCheckArgs =
    [
      "--python-platform"
      pythonPlatform
      "--python-version"
      python.pythonVersion
      "--output-format"
      "concise"
      "--no-progress"
      "--error-on-warning"
    ]
    ++ extraSearchPathArgs
    ++ typeCheckArgs
    ++ typeCheckPaths;
  # zuban/mypy strict args. The venv interpreter is passed in the phase string
  # (it interpolates the `$out` shell variable, so it cannot go through
  # escapeShellArgs). `--strict` also enables disallow-untyped-defs in both, so
  # missing signatures fail here too.
  strictCheckArgs =
    [
      "--python-version"
      python.pythonVersion
      "--platform"
      pythonPlatform
    ]
    ++ typeCheckArgs
    ++ typeCheckPaths;
  # ruff flake8-annotations (ANN) + banned-api (TID251, no typing.cast): the
  # explicit-annotation + no-cast gate the type checkers do not own. The shared
  # selector (`ruffAnnArgs`) is injected so every Python gate stays in sync.
  ruffPhase = "ruff check ${ruffAnnArgs} ${lib.escapeShellArgs typeCheckPaths}";
  pyCheckers = {
    ty = {
      inputs = [pkgs.ty];
      phase = ''ty check --python "$out/venv/bin/python" ${lib.escapeShellArgs tyCheckArgs}'';
    };
    zuban = {
      inputs = [
        pkgs.zuban
        pkgs.ruff
      ];
      phase = ''
        zuban check --strict --python-executable "$out/venv/bin/python" ${lib.escapeShellArgs strictCheckArgs}
        ${ruffPhase}
      '';
    };
    mypy = {
      inputs = [
        pkgs.mypy
        pkgs.ruff
      ];
      phase = ''
        mypy --strict --python-executable "$out/venv/bin/python" ${lib.escapeShellArgs strictCheckArgs}
        ${ruffPhase}
      '';
    };
  };
  selectedChecker =
    pyCheckers.${pyChecker}
      or (throw "buildUvApplication: unknown pyChecker \"${pyChecker}\" (expected \"ty\", \"zuban\", or \"mypy\")");
  exportArgs =
    [
      "--frozen"
      "--no-emit-project"
      "--no-editable"
      "--format"
      "requirements.txt"
    ]
    ++ lib.optional (!dev && !allGroups) "--no-dev"
    ++ lib.optional allGroups "--all-groups"
    ++ lib.optional allExtras "--all-extras"
    ++ groupFlags
    ++ extraFlags
    ++ exportFlags;
  pipInstallArgs =
    [
      "--offline"
      "--no-index"
      "--find-links"
      "${uvWheelhouse}"
      "--requirements"
      "requirements.txt"
    ]
    ++ pipInstallFlags;
  runtimeLibraryPath = lib.makeLibraryPath runtimeLibraryInputs;
  buildArgs =
    [
      "--wheel"
      "--offline"
      "--no-index"
      "--find-links"
      "${uvWheelhouse}"
      "--python"
      pythonExecutable
      "--no-managed-python"
      "--no-python-downloads"
      "--out-dir"
      "dist"
    ]
    ++ buildFlags;
in
  pkgs.stdenvNoCC.mkDerivation (_: {
    inherit
      pname
      version
      src
      uvWheelhouse
      ;

    strictDeps = true;

    nativeBuildInputs =
      [
        pkgs.makeWrapper
        pkgs.uv
        python
      ]
      ++ extraNativeBuildInputs;

    nativeInstallCheckInputs = selectedChecker.inputs;

    dontConfigure = true;
    dontBuild = true;
    doInstallCheck = check;

    installPhase = ''
      # shell
      runHook preInstall

      export HOME="$TMPDIR/home"
      export UV_CACHE_DIR="$TMPDIR/uv-cache"
      mkdir -p "$HOME" "$UV_CACHE_DIR" "$out/bin"

      uv export ${lib.escapeShellArgs exportArgs} --output-file requirements.txt
      ${pythonExecutable} -m venv "$out/venv"
      uv pip install ${lib.escapeShellArgs pipInstallArgs} --python "$out/venv/bin/python"
      uv build ${lib.escapeShellArgs buildArgs}
      uv pip install \
        --offline \
        --no-index \
        --find-links dist \
        --python "$out/venv/bin/python" \
        dist/*.whl

      # uv writes a `uv_cache.json` build-provenance stamp into every installed
      # package's dist-info, carrying a wall-clock timestamp. That makes the venv
      # non-reproducible (each stamp differs per build, and its hash flips its
      # dist-info RECORD line), which trips "hash mismatch importing path" on a
      # cache that holds a different variant. They are build-cache metadata with
      # no runtime role, so drop them and their RECORD entries for a bit-identical
      # install. Done in Python (removing a RECORD line keys on the exact stamp
      # path, not a hash we can know ahead of time, so substituteInPlace cannot
      # express it).
      ${pythonExecutable} ${./strip-uv-cache-stamp.py} "$out/venv"

      test -x "$out/venv/bin/${mainProgram}"
      makeWrapper "$out/venv/bin/${mainProgram}" "$out/bin/${mainProgram}" ${
        lib.optionalString (
          runtimeLibraryInputs != []
        ) "--prefix LD_LIBRARY_PATH : ${lib.escapeShellArg runtimeLibraryPath}"
      }

      runHook postInstall
    '';

    installCheckPhase = ''
      # shell
      runHook preInstallCheck

      ${selectedChecker.phase}

      runHook postInstallCheck
    '';

    passthru = {
      inherit uvWheelhouse;
    };

    meta =
      meta
      // {
        mainProgram = meta.mainProgram or mainProgram;
      };
  })
