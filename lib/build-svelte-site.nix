{
  bunLockFor,
  writeNushellApplication,
}:

/**
  Build and run a Svelte/Vite site from a locked JavaScript workspace.

  The build runs from a pure source closure and installs the static output
  under `$out/<installDir>`. The preview command serves that immutable output
  with miniserve. The dev-server command runs from a mutable checkout so Vite
  can write `node_modules`, caches, and HMR state outside the Nix store.

  Arguments:
  - `pname`, `version`: derivation identity.
  - `src`: project root containing `package.json` and the selected lockfile.
  - `packageManager`: `npm` or `bun`.
  - `buildScript`: package script for the production build.
  - `buildFlags`: arguments passed to the build script after `--`.
  - `preBuild`: shell code to run before the production build.
  - `distDir`: relative build output path inside `src`.
  - `installDir`: path under `$out` where built assets are installed.
  - `installFlags`: extra Bun install flags when `packageManager = "bun"`.
  - `extraNativeBuildInputs`: extra packages on PATH for the build.
  - `serve`: static preview settings, including `name`, `host`, `port`, and
    `routePrefix`.
  - `devServer`: checkout dev-server settings, including `name`,
    `checkoutSubdir`, `host`, `port`, and `autoInstall`.
  - `meta`: standard derivation meta.
*/
pkgs:
{
  pname,
  version ? "0.0.0",
  src,
  packageManager ? "npm",
  buildScript ? "build",
  buildFlags ? [ ],
  preBuild ? "",
  distDir ? "dist",
  installDir ? "share/${pname}",
  installFlags ? [ ],
  extraNativeBuildInputs ? [ ],
  serve ? { },
  devServer ? { },
  meta ? { },
}:
let
  inherit (pkgs) lib;

  validPackageManagers = [
    "bun"
    "npm"
  ];
  checkedPackageManager =
    if builtins.elem packageManager validPackageManagers then
      packageManager
    else
      throw "ix.buildSvelteSite.packageManager must be one of ${lib.concatStringsSep ", " validPackageManagers}; got ${packageManager}";

  packageManagers = {
    bun =
      let
        bunLock = bunLockFor pkgs;
        bunNodeModules = bunLock.buildNodeModules {
          bunRoot = src;
          inherit installFlags;
          derivationArgs = {
            strictDeps = true;
          };
        };
      in
      {
        buildCommandPrefix = [
          "bun"
          "run"
        ];
        derivationAttrs = {
          inherit bunNodeModules;
          configurePhase = ''
            runHook preConfigure

            patchShebangs .
            ln -s ${bunNodeModules}/node_modules node_modules
            export PATH="$PWD/node_modules/.bin:$PATH"

            runHook postConfigure
          '';
          passthru = {
            inherit bunNodeModules;
          };
        };
        devInstallCommand = [
          "bun"
          "install"
          "--frozen-lockfile"
        ];
        devRunCommandPrefix = [
          "bun"
          "run"
        ];
        nativeBuildInputs = [
          pkgs.bun
          bunLock.nodeCompat
        ];
        runtimeInputs = [ pkgs.bun ];
      };

    npm =
      let
        npmDeps = pkgs.importNpmLock.buildNodeModules {
          npmRoot = src;
          inherit (pkgs) nodejs;
          derivationArgs = {
            strictDeps = true;
          };
        };
      in
      {
        buildCommandPrefix = [
          "npm"
          "run"
        ];
        derivationAttrs = {
          inherit npmDeps;
          passthru = {
            inherit npmDeps;
          };
        };
        devInstallCommand = [
          "npm"
          "ci"
        ];
        devRunCommandPrefix = [
          "npm"
          "run"
        ];
        nativeBuildInputs = [
          pkgs.nodejs
          pkgs.importNpmLock.linkNodeModulesHook
        ];
        runtimeInputs = [ pkgs.nodejs ];
      };
  };
  manager = packageManagers.${checkedPackageManager};

  buildCommand =
    manager.buildCommandPrefix
    ++ [
      buildScript
    ]
    ++ lib.optional (buildFlags != [ ]) "--"
    ++ buildFlags;

  staticSite = pkgs.stdenvNoCC.mkDerivation (
    _:
    {
      inherit
        pname
        version
        src
        meta
        ;

      strictDeps = true;
      nativeBuildInputs = manager.nativeBuildInputs ++ extraNativeBuildInputs;

      buildPhase = ''
        runHook preBuild
        ${preBuild}
        ${lib.escapeShellArgs buildCommand}
        runHook postBuild
      '';

      installPhase = ''
        runHook preInstall
        mkdir -p "$out"/${lib.escapeShellArg installDir}
        cp -R ${lib.escapeShellArg (distDir + "/.")} "$out"/${lib.escapeShellArg installDir}/
        runHook postInstall
      '';
    }
    // manager.derivationAttrs
  );

  serveDefaults = {
    enable = true;
    name = pname;
    host = "127.0.0.1";
    port = 8080;
    routePrefix = null;
    spa = true;
    extraFlags = [ ];
  };
  serveConfig = serveDefaults // serve;
  serveArgs =
    lib.optionals serveConfig.spa [
      "--index"
      "index.html"
      "--spa"
    ]
    ++ [
      "--interfaces"
      serveConfig.host
      "--port"
      (toString serveConfig.port)
    ]
    ++ lib.optionals (serveConfig.routePrefix != null && serveConfig.routePrefix != "") [
      "--route-prefix"
      serveConfig.routePrefix
    ]
    ++ serveConfig.extraFlags
    ++ [ "${staticSite}/${installDir}" ];
  serveWrapperFlags = lib.concatMapStringsSep " " (
    arg: "--add-flag ${lib.escapeShellArg arg}"
  ) serveArgs;
  servePackage =
    pkgs.runCommand "${pname}-serve"
      {
        nativeBuildInputs = [ pkgs.makeBinaryWrapper ];
        strictDeps = true;
        meta = {
          description = "Serve the ${pname} Svelte build on ${serveConfig.host}:${toString serveConfig.port}";
          mainProgram = serveConfig.name;
        };
      }
      ''
        mkdir -p "$out/bin"
        makeWrapper ${lib.getExe pkgs.miniserve} "$out/bin"/${lib.escapeShellArg serveConfig.name} \
          ${serveWrapperFlags}
      '';

  devDefaults = {
    enable = true;
    name = "${pname}-dev";
    script = "dev";
    checkoutSubdir = null;
    host = "127.0.0.1";
    port = 5173;
    autoInstall = true;
    extraArgs = [ ];
  };
  devConfig = devDefaults // devServer;
  devRunPrefix = manager.devRunCommandPrefix ++ [
    devConfig.script
    "--"
  ];
  devServerPackage = writeNushellApplication pkgs {
    inherit (devConfig) name;
    inherit (manager) runtimeInputs;
    meta.description = "Run the ${pname} Svelte dev server from a mutable checkout";
    text = ''
      const checkout_subdir = ${builtins.toJSON devConfig.checkoutSubdir}
      const install_argv = ${builtins.toJSON manager.devInstallCommand}
      const run_prefix = ${builtins.toJSON devRunPrefix}
      const auto_install = ${builtins.toJSON devConfig.autoInstall}
      const extra_args = ${builtins.toJSON devConfig.extraArgs}

      def project-root [explicit_root: any] {
        if $explicit_root != null {
          $explicit_root | path expand
        } else {
          let cwd = (pwd)
          if ($checkout_subdir == null) or ($checkout_subdir == "") {
            $cwd
          } else {
            let candidate = ($cwd | path join $checkout_subdir)
            if ($candidate | path exists) { $candidate } else { $cwd }
          }
        }
      }

      def run-step [argv: list<string>] {
        let command = ($argv | first)
        let rest = ($argv | skip 1)
        ^$command ...$rest
      }

      def main [
        --root: path
        --host: string = ${builtins.toJSON devConfig.host}
        --port: int = ${toString devConfig.port}
        --install
        --skip-install
        ...args: string
      ] {
        let root = (project-root $root)
        cd $root

        let package_json = ($root | path join "package.json")
        if not ($package_json | path exists) {
          error make { msg: $"no package.json found in ($root)" }
        }

        let node_modules = ($root | path join "node_modules")
        if (not $skip_install) and ($install or ($auto_install and not ($node_modules | path exists))) {
          run-step $install_argv
        }

        let dev_args = ["--host", $host, "--port", ($port | into string)] ++ $extra_args ++ $args
        let argv = $run_prefix ++ $dev_args
        let command = ($argv | first)
        let rest = ($argv | skip 1)
        exec $command ...$rest
      }
    '';
  };

  passthru =
    (staticSite.passthru or { })
    // {
      inherit staticSite;
    }
    // lib.optionalAttrs serveConfig.enable {
      serve = servePackage;
    }
    // lib.optionalAttrs devConfig.enable {
      devServer = devServerPackage;
    };
in
pkgs.runCommand "${pname}-${version}"
  {
    strictDeps = true;
    inherit passthru;
    meta =
      meta
      // lib.optionalAttrs serveConfig.enable {
        mainProgram = meta.mainProgram or serveConfig.name;
      };
  }
  ''
    mkdir -p "$out"
    cp -R -L --no-preserve=mode,ownership ${staticSite}/. "$out"/

    ${lib.optionalString serveConfig.enable ''
      mkdir -p "$out/bin"
      ln -s ${lib.escapeShellArg "${servePackage}/bin/${serveConfig.name}"} "$out/bin"/${lib.escapeShellArg serveConfig.name}
    ''}

    ${lib.optionalString devConfig.enable ''
      mkdir -p "$out/bin"
      ln -s ${lib.escapeShellArg "${devServerPackage}/bin/${devConfig.name}"} "$out/bin"/${lib.escapeShellArg devConfig.name}
    ''}
  ''
