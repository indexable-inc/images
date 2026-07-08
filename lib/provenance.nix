# Eval-provenance walker: map every deployed config file of an evaluated
# home-manager / nix-darwin configuration back to the nix expression that
# defined it (#2414).
#
# The module system already records where every option definition came from
# (`definitionsWithLocations`, the same machinery behind nixpkgs
# `meta.position`); it just is not surfaced anywhere. This walker surfaces
# it: for each file-materializing option (`home.file`, `xdg.configFile`,
# `launchd.agents`/`daemons`, `environment.etc`) it emits one manifest entry
# per deployed path:
#
#   { file, line, rev, drv, source, definitions, settings }
#
# `file:line` is the most specific definition site: per-key
# `builtins.unsafeGetAttrPos` when the attrset was written literally,
# degrading to the defining module file (line null) for computed attrs
# (e.g. a `mapAttrs'` wiring hop). `definitions` keeps every site in
# definition order, so a file wired through several modules shows the whole
# chain.
#
# When a wiring module renders `programs.<x>.settings` into a file option
# (home-manager's htop module materializing `xdg.configFile."htop/htoprc"`,
# say), the entry additionally records the user's `programs.<x>.settings`
# definition sites under `settings`. The link is exact, not heuristic: the
# wiring module both declares the settings option and defines the file
# entry, so a definition site that appears in the settings option's
# `declarations` marks the hop.
#
# Pure eval, zero IFD. Store-path context is discarded from every recorded
# string: the manifest names sources and drvs, it must not retain them.
{lib}: let
  discard = builtins.unsafeDiscardStringContext;

  # A definition site: `pos` (from `builtins.unsafeGetAttrPos`) is trusted
  # only when it lands in the module file that made the definition, i.e. the
  # attr key was written literally there. Computed attrs (a `mapAttrs'`
  # wiring hop, an imported attrset) carry positions pointing into whatever
  # code constructed them (nixpkgs lib internals, another file), so they
  # degrade to the defining module file with no line.
  site = defFile: pos:
    if pos != null && pos.file == defFile
    then {
      file = discard pos.file;
      inherit (pos) line;
    }
    else {
      file = discard defFile;
      line = null;
    };

  # Definition sites of key `name` inside an attrsOf option: one
  # {file, line} per module that defines the key.
  keySites = option: name:
    lib.concatMap (
      def:
        lib.optional (builtins.isAttrs def.value && def.value ? ${name})
        (site def.file (builtins.unsafeGetAttrPos name def.value))
    )
    option.definitionsWithLocations;

  sourcePayload = source:
    if source == null
    then {
      source = null;
      drv = null;
    }
    else {
      source = discard (toString source);
      drv =
        if lib.isDerivation source
        then discard source.drvPath
        else null;
    };

  # Walk one attrsOf file option into raw entries. `path` maps a key plus
  # its merged config value to the deployed path; `source` extracts the
  # store payload backing it (null for options with no single payload,
  # e.g. launchd plists rendered by the wiring module).
  walkOption = {
    options,
    config,
  }: {
    optionPath,
    path,
    source ? _name: _value: null,
    enabled ? value: value.enable or true,
  }: let
    option = lib.attrByPath optionPath null options;
    cfg = lib.attrByPath optionPath {} config;
  in
    if !(builtins.isAttrs option) || !(option ? definitionsWithLocations)
    then []
    else
      lib.concatMap (
        name: let
          value = cfg.${name};
          sites = keySites option name;
        in
          if !(enabled value) || sites == []
          then []
          else [
            ({
                path = path name value;
                inherit sites;
              }
              // sourcePayload (source name value))
          ]
      ) (builtins.attrNames cfg);

  # Manifest keys are $HOME-relative for user files and absolute for system
  # files, matching what `whence` reconstructs from its argument.
  relativeTo = base: p:
    lib.removePrefix "/" (lib.removePrefix (toString base) (toString p));

  # Every `programs.<name>.settings`-style option that has definitions:
  # the user's definition sites plus the option's declaration files (the
  # wiring modules), used below to link settings onto file entries.
  settingsIndex = options:
    if !(options ? programs) || !(builtins.isAttrs options.programs)
    then []
    else
      lib.concatMap (
        name: let
          group = options.programs.${name};
          settings = group.settings or null;
        in
          if
            lib.isOption group
            || !(builtins.isAttrs settings)
            || !(settings ? definitionsWithLocations)
            || settings.definitionsWithLocations == []
          then []
          else [
            {
              option = "programs.${name}.settings";
              declarations = map (decl: discard (toString decl)) settings.declarations;
              definitions =
                map (
                  def: let
                    names =
                      if builtins.isAttrs def.value
                      then builtins.attrNames def.value
                      else [];
                    pos =
                      if names == []
                      then null
                      else builtins.unsafeGetAttrPos (builtins.head names) def.value;
                  in
                    site def.file pos
                )
                settings.definitionsWithLocations;
            }
          ]
      ) (builtins.attrNames options.programs);

  mergeEntry = prev: entry:
    if prev == null
    then entry
    else
      entry
      // {
        sites = lib.unique (prev.sites ++ entry.sites);
        source =
          if entry.source != null
          then entry.source
          else prev.source;
        drv =
          if entry.drv != null
          then entry.drv
          else prev.drv;
      };
in {
  # File collectors for an evaluated home-manager configuration. Later
  # collectors are more specific (an xdg entry re-walks the home.file entry
  # the xdg module wired for it), so their sites land after the hop's in the
  # merged chain and win the primary file:line slot.
  homeCollectors = {
    options,
    config,
  }: let
    collect = walkOption {inherit options config;};
    home = config.home.homeDirectory;
  in
    collect {
      optionPath = ["home" "file"];
      path = _name: value: relativeTo home value.target;
      source = _name: value: value.source or null;
    }
    # xdg.configFile targets are already normalized relative to the home
    # directory (home-manager's fileType anchors them at xdg.configHome and
    # re-relativizes), so the entry merges with the home.file entry the xdg
    # module wires for it: user site + wiring hop end up in one chain.
    ++ collect {
      optionPath = ["xdg" "configFile"];
      path = _name: value: relativeTo home value.target;
      source = _name: value: value.source or null;
    }
    # home-manager on darwin: agents are written per attr name under the
    # user's LaunchAgents directory.
    ++ collect {
      optionPath = ["launchd" "agents"];
      path = name: _value: "Library/LaunchAgents/${name}.plist";
    };

  # File collectors for an evaluated nix-darwin configuration.
  darwinCollectors = {
    options,
    config,
  }: let
    collect = walkOption {inherit options config;};
    label = name: value: value.serviceConfig.Label or "org.nixos.${name}";
  in
    collect {
      optionPath = ["environment" "etc"];
      path = _name: value: "/etc/${value.target}";
      source = _name: value: value.source or null;
    }
    ++ collect {
      optionPath = ["launchd" "agents"];
      path = name: value: "/Library/LaunchAgents/${label name value}.plist";
      enabled = _value: true;
    }
    ++ collect {
      optionPath = ["launchd" "daemons"];
      path = name: value: "/Library/LaunchDaemons/${label name value}.plist";
      enabled = _value: true;
    };

  # Fold collector entries into the manifest attrset:
  #   { version; files.<deployed path> = { file; line; rev; drv; source;
  #     definitions; settings; }; }
  # `rev` is the configuration flake's revision, copied onto every entry so
  # `whence` prints which checkout defined a file.
  manifestFor = {
    options,
    entries,
    rev ? null,
  }: let
    settingsList = settingsIndex options;
    byPath =
      lib.foldl' (
        acc: entry:
          acc // {${entry.path} = mergeEntry (acc.${entry.path} or null) entry;}
      ) {}
      entries;
    finish = entry: let
      primary =
        lib.findFirst (site: site.line != null) (lib.head entry.sites)
        (lib.reverseList entry.sites);
      linked =
        lib.filter (
          chain: lib.any (site: lib.elem site.file chain.declarations) entry.sites
        )
        settingsList;
    in {
      inherit (primary) file line;
      inherit rev;
      inherit (entry) source drv;
      definitions = entry.sites;
      settings = map (chain: {inherit (chain) option definitions;}) linked;
    };
  in {
    version = 1;
    files = lib.mapAttrs (_path: finish) byPath;
  };
}
