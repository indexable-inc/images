# Minimal TOML value encoding. SCALARS ONLY: this exists to encode the value
# half of a single `key = value` pair (e.g. a codex `--config a.b=1` flag),
# where the value must be one self-contained TOML literal. For generating whole
# TOML *files* from Nix, use `pkgs.formats.toml` instead, which handles tables
# and arrays too; do not grow this into a second, partial TOML serializer.
{lib}: {
  /**
  Encode a Nix scalar as the TOML literal a `key = value` line expects:
  booleans bare (`true`), strings quoted (`"x"`, via JSON which coincides
  with TOML basic strings for ordinary text), integers and floats as-is.

  Throws on anything else (lists, attrsets): a TOML array or inline table is
  not a scalar, so passing one is a caller bug, not something to silently
  stringify.
  */
  scalar = value:
    if builtins.isBool value
    then lib.boolToString value
    else if builtins.isString value
    then builtins.toJSON value
    else if builtins.isInt value || builtins.isFloat value
    then toString value
    else throw "ix.toml.scalar: not a TOML scalar: ${builtins.toJSON value}";
}
