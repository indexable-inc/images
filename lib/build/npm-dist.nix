{errors}:
/**
Bundle cross-compiled native binaries as a publishable npm distribution, the
esbuild/turborepo pattern: one platform package per target carrying just the
compiled binary (gated by npm's `os`/`cpu` fields so an install pulls exactly
one), plus a wrapper package with the user-facing name whose `bin` entry is a
small static Node shim ([`npm-dist-shim.js`](npm-dist-shim.js)) that resolves
the installed platform package through the wrapper's `optionalDependencies`
and runs the real binary, forwarding argv, stdio, the exit code, and fatal
signals. `npx <name>` then works on every packaged platform with no
postinstall step and no network fetch beyond npm itself.

Pair it with a cross-compiled unit graph (`rustWorkspace.unitsFor { target }`
in index, or a consumer's cross lanes) to publish one CLI for Linux, macOS,
and Windows from a single Linux builder.

Arguments (`buildNpmDist pkgs { ... }`):
- `name`: npm name of the wrapper package (what users `npx`), plain
  (`"my-cli"`) or scoped (`"@org/my-cli"`). Each platform package is named
  `"<name>-<platform>"`, e.g. `"my-cli-linux-x64"`.
- `version`: semver stamped into every package; the wrapper pins each
  platform package at exactly this version.
- `binName`: the command the wrapper's `bin` exposes. Defaults (via `null`)
  to `name` without any scope prefix.
- `platforms`: `{ "<os>-<cpu>" = { binary; libc ? null; }; }` keyed by Node's
  `process.platform`-`process.arch` pair (`linux-x64`, `darwin-arm64`,
  `win32-x64`, ...). `binary` is the compiled executable for that target
  (`lib.getExe <cross package>`); a `win32-*` binary installs with the `.exe`
  suffix the shim spawns. `libc` optionally narrows a Linux package to
  `"glibc"` or `"musl"` installs; leave it unset for a static binary that
  runs under either.
- `description`, `license` (SPDX id string), and optional `homepage` /
  `repository` (URL string): npm metadata stamped into every package.json.

Returns a derivation laying out ready-to-`npm publish` package directories:
  $out/packages/<dir>/    one directory per npm package (platforms + wrapper)
  $out/publish-order      newline-separated `packages/<dir>` lines, platform
                          packages first and the wrapper last, so publishing
                          in file order never exposes a wrapper whose pinned
                          platform packages are not on the registry yet
passthru: `wrapperName`, `wrapperDir`, `binName`, `packages` (publish-ordered
`[ { name; dir; } ]` with `dir` relative to `$out`).
*/
pkgs: {
  name,
  version,
  description,
  license,
  platforms,
  # `null` = the unscoped tail of `name` (defaults cannot reach the `let`
  # below, so the fallback is computed there).
  binName ? null,
  homepage ? null,
  repository ? null,
}: let
  inherit (pkgs) lib;

  # The command the wrapper's `bin` exposes (`binName`, defaulted to the
  # unscoped tail of `name` — arg defaults cannot reach this `let`).
  command =
    if binName == null
    then lib.last (lib.splitString "/" name)
    else binName;

  # Node `process.platform` / `process.arch` values a platform key may use.
  # The enum is what keeps a typo like `windows-x64` or `x86_64` from shipping
  # a platform package npm would never select.
  nodePlatforms = [
    "aix"
    "darwin"
    "freebsd"
    "linux"
    "openbsd"
    "sunos"
    "win32"
  ];
  nodeCpus = [
    "arm"
    "arm64"
    "ia32"
    "loong64"
    "mips64el"
    "ppc64"
    "riscv64"
    "s390x"
    "x64"
  ];

  # Directory name for a package inside `$out/packages/`: npm names are unique
  # per dist, so stripping the scope marker keeps them unique too.
  dirFor = packageName: lib.replaceStrings ["@" "/"] ["" "-"] packageName;

  sharedMetadata =
    {
      inherit version description license;
    }
    // lib.optionalAttrs (homepage != null) {inherit homepage;}
    // lib.optionalAttrs (repository != null) {
      repository = {
        type = "git";
        # npm normalizes repository URLs to the `git+<url>.git` form on
        # publish (and warns about anything else); emit that form directly so
        # published manifests match the source exactly.
        url =
          if lib.hasPrefix "git+" repository
          then repository
          else "git+${repository}${lib.optionalString (!lib.hasSuffix ".git" repository) ".git"}";
      };
    };

  platformEntries =
    lib.mapAttrsToList (
      key: spec: let
        parts = lib.splitString "-" key;
        os =
          assert lib.assertMsg (builtins.length parts == 2)
          "ix.buildNpmDist: platform key `${key}` must be `<os>-<cpu>` (Node `process.platform`-`process.arch`, e.g. `linux-x64`)";
            errors.assertEnum {
              name = "ix.buildNpmDist.platforms.${key} (os)";
              value = builtins.head parts;
              valid = nodePlatforms;
            };
        cpu = errors.assertEnum {
          name = "ix.buildNpmDist.platforms.${key} (cpu)";
          value = lib.last parts;
          valid = nodeCpus;
        };
        libc = spec.libc or null;
        packageName = "${name}-${key}";
      in {
        inherit key packageName;
        inherit (spec) binary;
        dir = dirFor packageName;
        # Windows executables must carry the `.exe` suffix; the shim appends
        # it when spawning on win32.
        executable = "${command}${lib.optionalString (os == "win32") ".exe"}";
        packageJson =
          {
            name = packageName;
            description = "${description} (${key} binary for the ${name} npm package)";
            os = [os];
            cpu = [cpu];
            # Yarn PnP keeps packages zipped by default; the binary must be a
            # real file on disk to exec.
            preferUnplugged = true;
          }
          // sharedMetadata
          // lib.optionalAttrs (libc != null) {
            libc = [
              (errors.assertEnum {
                name = "ix.buildNpmDist.platforms.${key} (libc)";
                value = libc;
                valid = [
                  "glibc"
                  "musl"
                ];
              })
            ];
          };
      }
    )
    platforms;

  wrapperDir = dirFor name;
  wrapperPackageJson =
    {
      inherit name;
      bin = {
        "${command}" = "bin/${command}.js";
      };
      # Exact-version pins: the shim's platform resolution and the wrapper are
      # published as one unit, so a range would let npm mix a new wrapper with
      # an old binary (or the reverse).
      optionalDependencies = lib.listToAttrs (
        map (entry: lib.nameValuePair entry.packageName version) platformEntries
      );
      # The `node:`-prefixed core-module specifiers the shim uses need >= 16.
      engines.node = ">=16";
      # Read back by the shim at run time: the one place the command name and
      # the platform -> package map live.
      npmDist = {
        binName = command;
        platforms = lib.mapAttrs' (
          key: _spec: lib.nameValuePair key "${name}-${key}"
        )
        platforms;
      };
    }
    // sharedMetadata;

  wrapperReadme = pkgs.writeText "npm-dist-readme" ''
    # ${name}

    ${description}

    ```console
    npx ${name}${lib.optionalString (command != name) "  # runs `${command}`"}
    ```

    Installing `${name}` pulls in exactly one of the platform packages below
    via `optionalDependencies` (npm matches their `os`/`cpu` fields); the
    `${command}` command is a small Node shim that runs that package's native
    binary directly.

    ${lib.concatMapStringsSep "\n" (entry: "- `${entry.packageName}`") (
      lib.sortOn (entry: entry.key) platformEntries
    )}
    ${lib.optionalString (repository != null) ''

      Binaries are cross-compiled and published from [the source repository](${repository}).
    ''}'';

  writePackageJson = json: "jq . ${pkgs.writeText "npm-dist-package.json" (builtins.toJSON json)}";

  installPlatform = entry: ''
    mkdir -p "$out/packages/${entry.dir}/bin"
    ${writePackageJson entry.packageJson} > "$out/packages/${entry.dir}/package.json"
    install -m 0755 ${lib.escapeShellArg entry.binary} \
      "$out/packages/${entry.dir}/bin/${entry.executable}"
    echo "packages/${entry.dir}" >> "$out/publish-order"
  '';
in
  assert lib.assertMsg (platforms != {})
  "ix.buildNpmDist: `platforms` must declare at least one `<os>-<cpu>` entry";
  assert lib.assertMsg (builtins.match "[A-Za-z0-9._-]+" command != null)
  "ix.buildNpmDist: `binName` (`${command}`) must be a plain command name ([A-Za-z0-9._-]+)";
    pkgs.stdenvNoCC.mkDerivation {
      pname = "npm-dist-${wrapperDir}";
      inherit version;
      __structuredAttrs = true;
      strictDeps = true;
      dontUnpack = true;
      nativeBuildInputs = [pkgs.jq];
      installPhase = ''
        # shell
        runHook preInstall
        ${lib.concatMapStringsSep "\n" installPlatform platformEntries}
        mkdir -p "$out/packages/${wrapperDir}/bin"
        ${writePackageJson wrapperPackageJson} > "$out/packages/${wrapperDir}/package.json"
        install -m 0755 ${./npm-dist-shim.js} "$out/packages/${wrapperDir}/bin/${command}.js"
        install -m 0644 ${wrapperReadme} "$out/packages/${wrapperDir}/README.md"
        echo "packages/${wrapperDir}" >> "$out/publish-order"
        runHook postInstall
      '';
      passthru = {
        wrapperName = name;
        binName = command;
        inherit wrapperDir;
        packages =
          map (entry: {
            name = entry.packageName;
            dir = "packages/${entry.dir}";
          })
          platformEntries
          ++ [
            {
              inherit name;
              dir = "packages/${wrapperDir}";
            }
          ];
      };
      meta.description = "npm distribution (platform packages + Node shim wrapper) for ${name}";
    }
