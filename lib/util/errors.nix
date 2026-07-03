{lib}: let
  inherit (lib) concatStringsSep;
  inherit (builtins) elem;
in {
  /**
  Validate that `value` is one of `valid`. Returns `value` unchanged on
  success; throws naming the option and listing every valid alternative
  on failure.

  Use at option boundaries where `lib.types.enum` is the right shape but
  the consumer wants the value back as an expression (a `let` binding,
  a default that derives from another option, etc).

  Arguments:
  - `name`: human-readable option path used in the error message
    (for example `"languages.rust.channel"`).
  - `value`: the value supplied by the caller.
  - `valid`: list of accepted values.

  Returns the validated `value`.
  */
  assertEnum = {
    name,
    value,
    valid,
  }:
    if elem value valid
    then value
    else throw "ix: invalid ${name} = ${toString value}. Valid values: ${concatStringsSep ", " valid}.";

  /**
  Look up `name` in `args` and return the value. Throws naming the
  helper and the missing argument when absent, so a caller that drops
  a required field gets a fixable error at the call site instead of
  `attribute 'version' missing` from inside the helper's body.

  Use at every helper boundary where the argument has no sensible
  default (version pins, vendor selection, target architecture).

  `context` should name the helper (for example
  `"ix.languages.go.toolchain"`).

  Arguments:
  - `context`: helper path for the error message.
  - `args`: the caller's argument attrset.
  - `name`: required attribute name.
  */
  requireArg = {
    context,
    args,
    name,
  }:
    args.${
      name
    } or (throw ''
      ix: ${context}
        Missing required argument '${name}'.
    '');

  /**
  Look up `key` in `attrset` and return the value. Throws with the list
  of available keys when the lookup misses, so a typo in a catalog key
  or a missing entry produces a fixable error instead of `attribute
  'foo' missing` from somewhere deep in eval.

  `context` should name the lookup surface (for example
  `"services.minecraft.modCatalog"`).
  */
  requireAttr = {
    context,
    attrset,
    key,
  }:
    attrset.${
      key
    } or (throw ''
      ix: ${context}
        Missing key '${toString key}'. Available: ${concatStringsSep ", " (lib.attrNames attrset)}.
    '');
}
