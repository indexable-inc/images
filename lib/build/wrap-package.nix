{ lib }:

/**
  Wrap a program derivation with runtime resources, env, and PATH.

  A generic composition primitive: it takes a base `package` (any derivation
  that ships `bin/<mainProgram>`), installs a set of `resources` (other
  derivations) into the output, and generates a small `/bin/<mainProgram>`
  shell wrapper that sets env and PATH before exec'ing the real binary. It is
  not Rust- or site-specific; a "resource" unifies "install this dependency's
  files" and "point an env var at where they landed" into one concept.

  The argument surface is a typed module schema (`wrapPackageModule` below),
  resolved through `lib.evalModules` like `lib/rust/policy.nix`. This gives
  three things from one declaration: defaults, caller-arg merging, and typo
  rejection (no `freeformType`, so an unknown key throws). The evaluated schema
  is re-exported as `ix.wrapPackage.options`, so every field's type, default,
  and description are introspectable:

      nix eval .#lib.wrapPackage.options.resources.description

  `ix.wrapPackage` is a functor attrset: call it as `ix.wrapPackage pkgs { ... }`
  and read `ix.wrapPackage.options` for the schema.
*/
let
  inherit (lib) mkOption types;

  # One runtime resource: files to install into the wrapper output, plus an
  # optional env var pointed at where they land. A submodule so each field is
  # typed and documented like the top-level args.
  resourceModule = {
    options = {
      source = mkOption {
        type = types.either types.package types.path;
        description = "Derivation or path to copy runtime files from.";
      };
      from = mkOption {
        type = types.str;
        default = "";
        description = "Subdirectory within `source` to copy (default: the whole tree).";
      };
      to = mkOption {
        type = types.str;
        description = "Subdirectory within `$out` to install the resource into.";
      };
      env = mkOption {
        type = types.nullOr types.str;
        default = null;
        description = "Optional env var name the wrapper sets to the resource's install path (`$out/<to>`).";
      };
    };
  };

  wrapPackageModule =
    { config, ... }:
    {
      options = {
        package = mkOption {
          type = types.package;
          description = "The unwrapped program derivation; must ship `bin/<mainProgram>`. `pname`/`version`/`meta` are read from it.";
        };
        mainProgram = mkOption {
          type = types.str;
          description = "Name of the wrapper binary written to `$out/bin` (defaults to `package.meta.mainProgram`).";
        };
        resources = mkOption {
          type = types.attrsOf (types.submodule resourceModule);
          default = { };
          description = "Runtime resources bundled into the wrapper output and optionally exposed via env vars.";
        };
        env = mkOption {
          type = types.attrsOf types.str;
          default = { };
          description = "Literal environment variables exported by the generated wrapper before exec.";
        };
        pathSuffix = mkOption {
          type = types.listOf types.package;
          default = [ ];
          description = "Packages appended to PATH in the wrapper on every platform.";
        };
        nativePathSuffix = mkOption {
          type = types.listOf types.package;
          default = [ ];
          description = "Packages appended to PATH only for non-cross builds (host-native tools with no cross artifact).";
        };
        isCross = mkOption {
          type = types.bool;
          default = false;
          description = "Set by the cross lane; when true, `nativePathSuffix` is dropped.";
        };
        symlinks = mkOption {
          type = types.attrsOf types.str;
          default = { };
          description = "`<name> = <target>` symlinks created under `$out/bin`.";
        };
        passthru = mkOption {
          type = types.attrs;
          default = { };
          description = "Extra `passthru` attributes merged onto the wrapper derivation (`unwrapped` is always added).";
        };
        meta = mkOption {
          type = types.attrs;
          default = { };
          description = "Extra `meta` attributes merged onto the wrapper derivation over the package's own meta.";
        };
      };
      # `mainProgram` defaults to the package's own `meta.mainProgram`. Expressed
      # as a config default (not an option `default`) because it reads another
      # option; `mkDefault` lets a caller override it. Lazy, so it only throws
      # when neither the caller nor the package supplies a name.
      config.mainProgram = lib.mkDefault (
        config.package.meta.mainProgram
          or (throw "ix.wrapPackage: `mainProgram` is unset and `package` has no `meta.mainProgram`")
      );
    };

  # Evaluate once with no caller config to expose the typed schema for
  # introspection. Reading `.options` never forces `.config`, so the required
  # `package` option needs no value here.
  schema = (lib.evalModules { modules = [ wrapPackageModule ]; }).options;

  build =
    pkgs: args:
    let
      cfg =
        (lib.evalModules {
          modules = [
            wrapPackageModule
            { config = args; }
          ];
        }).config;

      inherit (cfg) mainProgram;

      resourceList = lib.attrValues cfg.resources;
      renderResourceCopy =
        resource:
        let
          source = "${resource.source}/${resource.from}";
          targetDir = "$out/${resource.to}";
        in
        ''
          mkdir -p "${targetDir}"
          cp -R ${lib.escapeShellArg source}/. "${targetDir}/"
        '';
      # A resource that names an `env` var exposes its install path to the wrapper,
      # so the program finds its bundled files without the package restating paths.
      resourceEnv = lib.listToAttrs (
        lib.concatMap (
          resource: lib.optional (resource.env != null) (lib.nameValuePair resource.env "$out/${resource.to}")
        ) resourceList
      );
      # Resource env values reference `$out` (build-time expansion), so both maps
      # render through the same double-quoted export.
      envLines = lib.mapAttrsToList (name: value: "export ${name}=\"${value}\"") (cfg.env // resourceEnv);
      finalPathSuffix = cfg.pathSuffix ++ lib.optionals (!cfg.isCross) cfg.nativePathSuffix;
      pathLine = lib.optionalString (finalPathSuffix != [ ]) ''
        export PATH="$PATH:${lib.makeBinPath finalPathSuffix}"
      '';
      symlinkLines = lib.mapAttrsToList (
        name: target: "ln -s ${lib.escapeShellArg target} \"$out/bin/${name}\""
      ) cfg.symlinks;
    in
    pkgs.runCommand "${cfg.package.pname}-${cfg.package.version}"
      {
        strictDeps = true;
        passthru = cfg.passthru // {
          unwrapped = cfg.package;
        };
        meta =
          (cfg.package.meta or { })
          // cfg.meta
          // {
            inherit mainProgram;
          };
      }
      ''
        mkdir -p "$out/bin"
        ${lib.concatMapStrings renderResourceCopy resourceList}
        cp ${lib.getExe cfg.package} "$out/bin/.${mainProgram}-unwrapped"
        chmod 0755 "$out/bin/.${mainProgram}-unwrapped"
        cat > "$out/bin/${mainProgram}" <<EOF
        #!/bin/sh
        ${lib.concatStringsSep "\n" envLines}
        ${pathLine}
        exec "$out/bin/.${mainProgram}-unwrapped" "\$@"
        EOF
        chmod 0755 "$out/bin/${mainProgram}"
        ${lib.concatStringsSep "\n" symlinkLines}
      '';
in
{
  __functor = _self: build;
  options = schema;
}
