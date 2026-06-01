{ lib }:
let
  /**
    Package a Python entrypoint as a standalone executable.

    Wraps `src` in a launcher script that prepends `runtimeInputs` to PATH
    and runs the file under `python`. When `check` is true (default), the
    derivation also runs `ty` over `src` during the build, so type regressions
    fail the build instead of surfacing at runtime.

    Arguments:
    - `name`: derivation name and `/bin/<name>` executable.
    - `src`: a path or store path containing the Python entrypoint.
    - `args`: literal argv prefix prepended to user args at runtime.
    - `runtimeInputs`: extra packages prepended to PATH at runtime.
    - `python`: Python interpreter package. Defaults to `pkgs.python314`.
    - `check`, `pythonPlatform`: ty knobs.
    - `extraPaths`: extra import roots for ty.
    - `meta`: standard derivation meta, with `mainProgram` defaulted.
  */
  writePythonApplication =
    pkgs:
    {
      name,
      src,
      args ? [ ],
      runtimeInputs ? [ ],
      python ? pkgs.python314,
      check ? true,
      pythonPlatform ? "linux",
      extraPaths ? [ "${python}/${python.sitePackages}" ],
      meta ? { },
    }:
    let
      runtimePath = lib.makeBinPath ([ python ] ++ runtimeInputs);
      srcPath = src;
      argv = builtins.toJSON ([ "${srcPath}" ] ++ args);
      extraSearchPathArgs = lib.concatMap (path: [
        "--extra-search-path"
        path
      ]) extraPaths;
      tyCheckArgs = [
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
      ++ [ "${src}" ];
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
        sys.argv = ${argv} + sys.argv[1:]
        runpy.run_path("${srcPath}", run_name="__main__")
      '';
      checkPhase = lib.optionalString check ''
        ${lib.getExe pkgs.ty} ${lib.escapeShellArgs tyCheckArgs}
      '';
      meta = meta // {
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
  writeNushellApplication =
    pkgs:
    {
      name,
      runtimeInputs ? [ ],
      text,
      check ? true,
      meta ? { },
    }:
    let
      scriptBody = lib.removePrefix "#!/usr/bin/env nu\n" text;
      runtimePath = lib.makeBinPath ([ pkgs.nushell ] ++ runtimeInputs);
    in
    pkgs.writeTextFile {
      inherit name;
      executable = true;
      destination = "/bin/${name}";
      text = ''
        #!${lib.getExe pkgs.nushell}
        let runtime_path = "${runtimePath}" | split row ":"
        let ambient_path = $env.PATH? | default []
        $env.PATH = $runtime_path ++ (if ($ambient_path | describe) == "string" { $ambient_path | split row ":" } else { $ambient_path })

      ''
      + scriptBody;
      checkPhase = lib.optionalString check ''
        ${lib.getExe pkgs.nushell} --no-config-file --no-std-lib --ide-check 100 "$target"
      '';
      meta = meta // {
        mainProgram = meta.mainProgram or name;
      };
    };

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
  writeProcessComposeApplication =
    pkgs:
    {
      name,
      processes,
      settings ? { },
      runtimeInputs ? [ ],
      processComposeArgs ? [
        "--no-server"
        "--ordered-shutdown"
        "--disable-dotenv"
      ],
      meta ? { },
    }:
    let
      format = pkgs.formats.yaml { };
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
        runtimeInputs = [ pkgs.process-compose ] ++ runtimeInputs;
        meta = meta // {
          mainProgram = meta.mainProgram or name;
        };
        text = ''
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
            nativeBuildInputs = [ pkgs.process-compose ];
            strictDeps = true;
          }
          ''
            process-compose --config ${config} ${processComposeArgsText} --dry-run
            mkdir -p "$out"
          '';
    in
    package.overrideAttrs (old: {
      passthru = (old.passthru or { }) // {
        inherit config;
        tests = (old.passthru.tests or { }) // {
          inherit dryRun;
        };
      };
    });
in
{
  inherit
    writePythonApplication
    writeNushellApplication
    writeProcessComposeApplication
    ;
}
