{
  lib,
  # Shared ruff selector (ANN explicit-annotations + TID251 no-typing.cast),
  # injected by lib/default.nix so every Python gate enforces the same policy.
  ruffAnnArgs,
}: let
  /**
  Package a Python entrypoint as a standalone executable.

  Wraps `src` in a launcher script that prepends `runtimeInputs` to PATH
  and runs the file under `python`. When `check` is true (default), the
  derivation also runs `zuban check --strict` + `ruff check --select ANN` over
  `src` during the build, so type and annotation regressions fail the build
  instead of surfacing at runtime.

  Arguments:
  - `name`: derivation name and `/bin/<name>` executable.
  - `src`: a path or store path containing the Python entrypoint.
  - `args`: literal argv prefix prepended to user args at runtime.
  - `runtimeInputs`: extra packages prepended to PATH at runtime.
  - `python`: Python interpreter package. Defaults to `pkgs.python314`.
  - `check`, `pyChecker`, `pythonPlatform`: type-check knobs. `pyChecker` is
    "zuban" (default), "ty" (legacy), or "mypy"; "zuban"/"mypy" run that checker
    `--strict` plus `ruff check --select ANN`.
  - `extraPaths`: extra import roots for the checker.
  - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writePythonApplication = pkgs: {
    name,
    src,
    args ? [],
    runtimeInputs ? [],
    python ? pkgs.python314,
    check ? true,
    # "zuban" (default), "ty" (legacy), or "mypy". "zuban"/"mypy" run that
    # checker `--strict` plus `ruff check --select ANN`. See buildUvApplication.
    pyChecker ? "zuban",
    pythonPlatform ? "linux",
    extraPaths ? ["${python}/${python.sitePackages}"],
    meta ? {},
  }: let
    runtimePath = lib.makeBinPath ([python] ++ runtimeInputs);
    srcPath = src;
    argv = builtins.toJSON (["${srcPath}"] ++ args);
    extraSearchPathArgs =
      lib.concatMap (path: [
        "--extra-search-path"
        path
      ])
      extraPaths;
    tyCheckArgs =
      [
        "check"
        "--python"
        (lib.getExe python)
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
      ++ ["${src}"];
    ruffAnnPhase = "${lib.getExe' pkgs.ruff "ruff"} check ${ruffAnnArgs} ${lib.escapeShellArg "${src}"}";
    # zuban/mypy resolve the interpreter's own packages from
    # `--python-executable`, so MYPYPATH must carry only genuinely-extra import
    # roots. Forwarding the interpreter's site-packages (the default of
    # `extraPaths`) is both redundant and harmful: pointing MYPYPATH at a real
    # site-packages dir makes zuban drop the stdlib typeshed, so e.g.
    # `Path.__truediv__` widens to `Any` and trips `no-any-return`. Drop the
    # default site-packages; keep any caller-added roots.
    strictMypyPaths = lib.filter (p: p != "${python}/${python.sitePackages}") extraPaths;
    mypyPathPrefix = lib.optionalString (
      strictMypyPaths != []
    ) "MYPYPATH=${lib.escapeShellArg (lib.concatStringsSep ":" strictMypyPaths)} ";
    # `zuban` needs the `check` subcommand; `mypy` is invoked directly. Both
    # accept --strict / --python-executable / --python-version / --platform.
    #
    # zuban only discovers source files at or below its working directory, so a
    # bare absolute store path (the usual single-file `src`) yields "No Python
    # files found to check". Run the checker from the src's parent directory
    # and pass the basename; this is equally valid for a directory `src` (zuban
    # walks the named dir). ruff resolves absolute paths fine, so it keeps the
    # full path and runs from the build's default cwd.
    strictPhase = checker: subcommand: ''
      ( cd ${lib.escapeShellArg (builtins.dirOf "${src}")} && \
        ${mypyPathPrefix}${lib.getExe' checker (lib.getName checker)} ${subcommand}--strict \
          --python-executable ${lib.escapeShellArg (lib.getExe python)} \
          --python-version ${python.pythonVersion} --platform ${pythonPlatform} \
          ${lib.escapeShellArg (builtins.baseNameOf "${src}")} )
      ${ruffAnnPhase}
    '';
    pyCheckers = {
      ty = "${lib.getExe pkgs.ty} ${lib.escapeShellArgs tyCheckArgs}";
      zuban = strictPhase pkgs.zuban "check ";
      mypy = strictPhase pkgs.mypy "";
    };
    checkerPhase =
      pyCheckers.${pyChecker}
          or (throw "writePythonApplication: unknown pyChecker \"${pyChecker}\" (expected \"ty\", \"zuban\", or \"mypy\")");
  in
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text = ''
        #!${lib.getExe python}
        import os
        import runpy
        import sys

        runtime_path = ${builtins.toJSON runtimePath}
        ambient_path = os.environ.get("PATH", "")
        os.environ["PATH"] = runtime_path + ((":" + ambient_path) if ambient_path else "")
        # Nix Python does not consistently inherit a platform CA bundle; urllib needs this for HTTPS updaters.
        ca_bundle = "${pkgs.cacert}/etc/ssl/certs/ca-bundle.crt"
        os.environ.setdefault("SSL_CERT_FILE", ca_bundle)
        os.environ.setdefault("REQUESTS_CA_BUNDLE", os.environ["SSL_CERT_FILE"])
        sys.argv = ${argv} + sys.argv[1:]
        runpy.run_path("${srcPath}", run_name="__main__")
      '';
      checkPhase = lib.optionalString check checkerPhase;
      meta =
        meta
        // {
          mainProgram = meta.mainProgram or name;
        };
    };

  /**
  Package a Nushell command as a standalone executable.

  Generates a Nu script that prepends `runtimeInputs` to PATH while
  preserving the ambient PATH, then runs `text` as the body. With
  `check` left on (default), nushell's `--ide-check` parses the
  generated script during the build so syntax errors fail the build
  rather than reaching the user.

  Arguments:
  - `name`: derivation name and `/bin/<name>` executable.
  - `runtimeInputs`: packages prepended to PATH for the script body.
  - `text`: the Nu script body. A leading `#!/usr/bin/env nu` line is
    stripped before splicing.
  - `check`: run `nu --ide-check` at build time.
  - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writeNushellApplication = pkgs: {
    name,
    runtimeInputs ? [],
    text,
    check ? true,
    meta ? {},
  }: let
    scriptBody = lib.removePrefix "#!/usr/bin/env nu\n" text;
    runtimePath = lib.makeBinPath ([pkgs.nushell] ++ runtimeInputs);
  in
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text =
        ''
          #!${lib.getExe pkgs.nushell}
          let runtime_path = "${runtimePath}" | split row ":"
          let ambient_path = $env.PATH? | default []
          $env.PATH = $runtime_path ++ (if ($ambient_path | describe) == "string" { $ambient_path | split row ":" } else { $ambient_path })

        ''
        + scriptBody;
      checkPhase = lib.optionalString check ''
        ${lib.getExe pkgs.nushell} --no-config-file --no-std-lib --ide-check 100 "$target"
      '';
      meta =
        meta
        // {
          mainProgram = meta.mainProgram or name;
        };
    };

  /**
  Package a Bash script as a standalone executable.

  Nushell (`writeNushellApplication`) is the default for repo commands; this
  is the one sanctioned escape hatch for scripts that must be bash, such as
  exec-style toolchain wrappers and POSIX process-control idioms (setsid,
  flock, `exec "$@"`). The generated script runs under `set -euo pipefail`
  with `runtimeInputs` prepended to PATH, and the build runs `bash -n` plus
  shellcheck so a syntax error or a shellcheck-class bug fails the
  derivation instead of surfacing at runtime.

  Arguments:
  - `name`: derivation name and `/bin/<name>` executable.
  - `runtimeInputs`: packages prepended to PATH for the script body.
  - `text`: the bash script body. No shebang or `set` line; the wrapper
    supplies both.
  - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writeBashApplication = pkgs: {
    name,
    runtimeInputs ? [],
    text,
    meta ? {},
  }:
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text =
        ''
          #!${pkgs.runtimeShell}
          set -euo pipefail
        ''
        + lib.optionalString (runtimeInputs != []) ''
          export PATH=${lib.makeBinPath runtimeInputs}''${PATH:+:$PATH}
        ''
        + text;
      checkPhase = ''
        # shell
        runHook preCheck
        ${lib.getExe' pkgs.bash "bash"} -n "$target"
        ${lib.getExe pkgs.shellcheck} --shell=bash --severity=warning "$target"
        runHook postCheck
      '';
      meta =
        meta
        // {
          mainProgram = meta.mainProgram or name;
        };
    };

  /**
  Package a single-file Rust command as a standalone executable.

  This is for small std-only operational helpers. Use a normal workspace crate
  when the command needs crates.io dependencies or tests.
  */
  writeRustApplication = pkgs: {
    name,
    text,
    edition ? "2024",
    runtimeInputs ? [],
    rustToolchain ? pkgs.rustc,
    meta ? {},
  }:
    pkgs.runCommand name
    {
      inherit text;
      nativeBuildInputs =
        [
          rustToolchain
          pkgs.stdenv.cc
        ]
        ++ lib.optional (runtimeInputs != []) pkgs.makeWrapper;
      passAsFile = ["text"];
      meta =
        meta
        // {
          mainProgram = meta.mainProgram or name;
        };
    }
    (
      ''
        mkdir -p "$out/bin"
        rustc --edition ${edition} -O --crate-name ${
          lib.replaceStrings ["-"] ["_"] name
        } -o "$out/bin/${name}" "$textPath"
      ''
      + lib.optionalString (runtimeInputs != []) ''
        wrapProgram "$out/bin/${name}" \
          --prefix PATH : ${lib.makeBinPath runtimeInputs}
      ''
    );

  /**
  Package a process-compose specification as a `nix run` application.

  Generates a checked YAML config from Nix values and wraps
  `process-compose` in a foreground command. By default the wrapper disables
  the TUI in config and disables dotenv injection plus the process-compose
  HTTP control server through CLI flags, so application ports remain
  available and logs stay in the caller's terminal.

  Arguments:
  - `name`: derivation name and `/bin/<name>` executable.
  - `processes`: attrset assigned to `processes` in the generated config.
  - `settings`: extra top-level process-compose config fields.
  - `runtimeInputs`: packages prepended to PATH before process-compose runs.
  - `processComposeArgs`: argv inserted before user-provided args.
  - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writeProcessComposeApplication = pkgs: {
    name,
    processes,
    settings ? {},
    runtimeInputs ? [],
    processComposeArgs ? [
      "--no-server"
      "--ordered-shutdown"
      "--disable-dotenv"
    ],
    meta ? {},
  }: let
    format = pkgs.formats.yaml {};
    xdgConfigHome =
      pkgs.runCommand "${name}-process-compose-xdg-config"
      {
        strictDeps = true;
      }
      ''
        mkdir -p "$out/process-compose"
      '';
    config = format.generate "${name}.process-compose.yaml" (
      {
        version = "0.5";
        is_tui_disabled = true;
      }
      // settings
      // {
        inherit processes;
      }
    );
    processComposeArgsText = lib.escapeShellArgs processComposeArgs;
    package = writeNushellApplication pkgs {
      inherit name;
      runtimeInputs = [pkgs.process-compose] ++ runtimeInputs;
      meta =
        meta
        // {
          mainProgram = meta.mainProgram or name;
        };
      text = ''
        # nu
        const process_compose_args = ${builtins.toJSON processComposeArgs}

        def main [...args: string] {
          $env.XDG_CONFIG_HOME = "${xdgConfigHome}"
          exec process-compose --config ${config} ...$process_compose_args ...$args
        }
      '';
    };
    dryRun =
      pkgs.runCommand "${name}-process-compose-dry-run"
      {
        nativeBuildInputs = [pkgs.process-compose];
        strictDeps = true;
      }
      ''
        process-compose --config ${config} ${processComposeArgsText} --dry-run
        mkdir -p "$out"
      '';
  in
    package.overrideAttrs (old: {
      passthru =
        (old.passthru or {})
        // {
          inherit config;
          tests =
            (old.passthru.tests or {})
            // {
              inherit dryRun;
            };
        };
    });
in {
  inherit
    writePythonApplication
    writeNushellApplication
    writeBashApplication
    writeRustApplication
    writeProcessComposeApplication
    ;
}
