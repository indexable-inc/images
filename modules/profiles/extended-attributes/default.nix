{
  config,
  lib,
  pkgs,
  ...
}: let
  inherit
    (lib)
    mkIf
    mkOption
    types
    ;

  cfg = config.ix.extendedAttributes;

  xattrEntryType = types.submodule {
    options = {
      create = mkOption {
        type = types.bool;
        default = false;
        description = "Whether to create this path as a directory before applying extended attributes.";
      };

      attributes = mkOption {
        type = types.attrsOf types.str;
        default = {};
        description = "Extended attributes to apply to this path. Attribute names must use the `user.` namespace.";
      };
    };
  };

  pathSegments = path: lib.drop 1 (lib.splitString "/" path);
  invalidPaths = lib.filter (
    path: let
      segments = pathSegments path;
    in
      path == "" || !lib.hasPrefix "/" path || builtins.elem "" segments || builtins.elem ".." segments
  ) (lib.attrNames cfg);

  invalidAttributeNames = lib.concatLists (
    lib.mapAttrsToList (
      path: entry:
        map (name: "${path}: ${name}") (
          lib.filter (name: !(lib.hasPrefix "user." name) || name == "user.") (lib.attrNames entry.attributes)
        )
    )
    cfg
  );

  setfattr = "${pkgs.attr}/bin/setfattr";
  mkdir = "${pkgs.coreutils}/bin/mkdir";

  applyAttributeFunction = ''
    ix_set_xattr() {
      ix_xattr_path=$1
      ix_xattr_name=$2
      ix_xattr_value=$3

      ix_xattr_error=$(${setfattr} --name "$ix_xattr_name" --value "$ix_xattr_value" -- "$ix_xattr_path" 2>&1) && return 0

      case "$ix_xattr_error" in
        *"Operation not supported"* | *"Not supported"* | *"Function not implemented"*)
          printf '%s\n' "warning: filesystem does not support extended attributes on $ix_xattr_path; skipped $ix_xattr_name" >&2
          return 0
          ;;
      esac

      printf '%s\n' "$ix_xattr_error" >&2
      return 1
    }
  '';

  applyPath = path: entry: let
    escapedPath = lib.escapeShellArg path;
    create = lib.optionalString entry.create ''
      ${mkdir} -p -- ${escapedPath}
    '';
    setAttributes =
      if entry.attributes == {}
      then ":"
      else
        lib.concatStringsSep "\n" (
          lib.mapAttrsToList (
            name: value: "ix_set_xattr ${escapedPath} ${lib.escapeShellArg name} ${lib.escapeShellArg value}"
          )
          entry.attributes
        );
  in ''
    ${create}
    if [ -L ${escapedPath} ]; then
      printf '%s\n' ${lib.escapeShellArg "refusing to set extended attributes on symlink: ${path}"} >&2
      exit 1
    fi

    if [ -e ${escapedPath} ]; then
      ${setAttributes}
    fi
  '';

  applyScript = lib.concatStringsSep "\n" (
    [applyAttributeFunction] ++ lib.mapAttrsToList applyPath cfg
  );
in {
  options.ix.extendedAttributes = mkOption {
    type = types.attrsOf xattrEntryType;
    default = {};
    description = ''
      Extended attributes to apply to runtime filesystem paths during system
      activation. Keys are absolute paths. Missing paths are skipped unless
      `create` is true.

      This is metadata, not a containment boundary: use the `user.ix.*`
      namespace for ix-owned labels that runtime tools can inspect.
    '';
  };

  config = mkIf (cfg != {}) {
    assertions = [
      {
        assertion = invalidPaths == [];
        message = "ix.extendedAttributes keys must be absolute paths without empty or '..' segments: ${lib.concatStringsSep ", " invalidPaths}";
      }
      {
        assertion = invalidAttributeNames == [];
        message = "ix.extendedAttributes attribute names must use the user.* namespace: ${lib.concatStringsSep ", " invalidAttributeNames}";
      }
    ];

    environment.systemPackages = [pkgs.attr];

    system.activationScripts.ix-extended-attributes.text = applyScript;
  };
}
