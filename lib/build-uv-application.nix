{ uvLockFor }:

/**
  Build a Python application from a uv project.

  Dependency hashes come from `uv.lock`, so callers update dependencies with
  `uv lock` and do not maintain a separate Nix dependency hash. Locked
  distributions are fetched into a wheelhouse, installed offline into a virtual
  environment, and the local project is built as a wheel before installation.
  Type checking runs by default with `ty` after install against the installed
  virtual environment, matching `writePythonApplication`.

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
  - `check`, `pythonPlatform`, `typeCheckPaths`, `typeCheckArgs`: ty knobs.
  - `extraNativeBuildInputs`: extra packages on PATH for the build.
  - `runtimeLibraryInputs`: shared libraries made visible to binary wheels.
  - `fetcherOpts`: per-package fetcher overrides for locked distributions.
  - `meta`: standard derivation meta.
*/
pkgs:
{
  pname,
  version ? "0.0.0",
  # Pass `srcRoot = ./.` for a standard uv project (pyproject.toml + src/ +
  # uv.lock at the root); pass `src` directly to provide a custom fileset.
  srcRoot ? null,
  src ?
    let
      inherit (pkgs) lib;
    in
    if srcRoot != null then
      lib.fileset.toSource {
        root = srcRoot;
        fileset = lib.fileset.unions [
          (srcRoot + "/pyproject.toml")
          (srcRoot + "/src")
          (srcRoot + "/uv.lock")
        ];
      }
    else
      throw "buildUvApplication: pass `srcRoot` for a standard uv project layout or `src` for a custom one",
  python ? pkgs.python3,
  mainProgram ? pname,
  groups ? [ ],
  dependencyGroups ? groups,
  extras ? [ ],
  dev ? false,
  allGroups ? false,
  allExtras ? false,
  exportFlags ? [ ],
  pipInstallFlags ? [ ],
  buildFlags ? [ ],
  check ? true,
  pythonPlatform ? "linux",
  typeCheckPaths ? [ "." ],
  extraPaths ? [ ],
  typeCheckArgs ? [ ],
  extraNativeBuildInputs ? [ ],
  runtimeLibraryInputs ? [ ],
  fetcherOpts ? { },
  meta ? { },
}:
let
  inherit (pkgs) lib;

  uvLock = uvLockFor pkgs;
  uvWheelhouse = uvLock.buildWheelhouse {
    uvRoot = src;
    inherit fetcherOpts python;
  };
  pythonExecutable = lib.getExe python;
  groupFlags = lib.concatMap (group: [
    "--group"
    group
  ]) dependencyGroups;
  extraFlags = lib.concatMap (extra: [
    "--extra"
    extra
  ]) extras;
  extraSearchPathArgs = lib.concatMap (path: [
    "--extra-search-path"
    path
  ]) extraPaths;
  tyCheckArgs = [
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
  exportArgs = [
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
  pipInstallArgs = [
    "--offline"
    "--no-index"
    "--find-links"
    "${uvWheelhouse}"
    "--requirements"
    "requirements.txt"
  ]
  ++ pipInstallFlags;
  runtimeLibraryPath = lib.makeLibraryPath runtimeLibraryInputs;
  buildArgs = [
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

  nativeBuildInputs = [
    pkgs.makeWrapper
    pkgs.uv
    python
  ]
  ++ extraNativeBuildInputs;

  nativeInstallCheckInputs = [ pkgs.ty ];

  dontConfigure = true;
  dontBuild = true;
  doInstallCheck = check;

  installPhase = ''
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

    test -x "$out/venv/bin/${mainProgram}"
    makeWrapper "$out/venv/bin/${mainProgram}" "$out/bin/${mainProgram}" ${
      lib.optionalString (
        runtimeLibraryInputs != [ ]
      ) "--prefix LD_LIBRARY_PATH : ${lib.escapeShellArg runtimeLibraryPath}"
    }

    runHook postInstall
  '';

  installCheckPhase = ''
    runHook preInstallCheck

    ty check --python "$out/venv/bin/python" ${lib.escapeShellArgs tyCheckArgs}

    runHook postInstallCheck
  '';

  passthru = {
    inherit uvWheelhouse;
  };

  meta = meta // {
    mainProgram = meta.mainProgram or mainProgram;
  };
})
