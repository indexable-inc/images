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
  is re-exported as `ix.wrapPackage.options` (raw tree) and
  `ix.wrapPackage.optionsDoc` (flat, JSON-able), so every field's type, default,
  and description are introspectable:

      nix eval .#lib.wrapPackage.options.resources.description
      nix eval --json .#lib.wrapPackage.optionsDoc

  `ix.wrapPackage` is a functor attrset: call it as `ix.wrapPackage pkgs { ... }`
  and read `ix.wrapPackage.options` for the schema. Argument documentation lives
  only in the `mkOption` descriptions below; this comment covers behaviour.
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
          # The default is computed in `config` below, so introspection would
          # show the option as required without this `defaultText`.
          defaultText = lib.literalExpression "package.meta.mainProgram";
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
        # `types.raw`, not `types.attrs`: these are opaque bags (derivations,
        # functions, nested test attrsets) passed through untouched, and `raw`
        # rejects a second definition instead of silently shallow-merging it.
        passthru = mkOption {
          type = types.raw;
          default = { };
          description = "Extra `passthru` attributes merged onto the wrapper derivation (`unwrapped` is always added).";
        };
        meta = mkOption {
          type = types.raw;
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
            {
              # Names the caller's definitions in module errors ("The option
              # `x' does not exist. Definition values: - In `ix.wrapPackage
              # args'"), which otherwise cite `<unknown-file>`.
              _file = "ix.wrapPackage args";
              config = args;
            }
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
      # The wrapper is written through an UNQUOTED heredoc so `$out` can expand
      # at build time; anything meant to reach the wrapper verbatim must be
      # escaped against that pass ($, backtick, backslash).
      escapeHeredoc = lib.replaceStrings [ "\\" "$" "`" ] [ "\\\\" "\\$" "\\`" ];
      # Caller `env` values are literals: single-quote them for the runtime
      # shell AND heredoc-escape them, so `$`, quotes, and backticks survive
      # both the build-time heredoc and the runtime `sh` parse. A resource env
      # var of the same name wins, matching the old `cfg.env // resourceEnv`.
      literalEnvLines = lib.mapAttrsToList (
        name: value: "export ${name}=${escapeHeredoc (lib.escapeShellArg value)}"
      ) (removeAttrs cfg.env (lib.attrNames resourceEnv));
      # Resource env values reference `$out` (build-time expansion), so they
      # render unescaped through a double-quoted export.
      resourceEnvLines = lib.mapAttrsToList (name: value: "export ${name}=\"${value}\"") resourceEnv;
      envLines = literalEnvLines ++ resourceEnvLines;
      finalPathSuffix = cfg.pathSuffix ++ lib.optionals (!cfg.isCross) cfg.nativePathSuffix;
      # `\$PATH` keeps the expansion at runtime: the operator's own PATH stays
      # first (their `nix`, `home-manager`, ... win) and the suffix only appends.
      # An unescaped `$PATH` would bake the build sandbox PATH into the wrapper.
      pathLine = lib.optionalString (finalPathSuffix != [ ]) ''
        export PATH="\$PATH:${lib.makeBinPath finalPathSuffix}"
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
  # Flat `[ { name; type; description; default?; ... } ]` view of the same
  # schema for whole-surface queries (`nix eval --json .#lib.wrapPackage.optionsDoc`);
  # submodule fields appear as `resources.<name>.*` entries. Internal module
  # plumbing (`_module.*`) is filtered the same way nixosOptionsDoc filters it.
  optionsDoc = lib.filter (opt: opt.visible && !opt.internal) (lib.optionAttrSetToDocList schema);
}
