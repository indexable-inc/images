# The `__ixTy` runtime behind ix2nix's emitted type checks: every converted
# module renders as `{ __dir, __importIx, __ixTy }: <body>` and calls only
# `arg` (parameter check) and `ret` (return and `as` check), passing checker
# values built from the other attributes. The import shims construct one
# runtime per module via `forModule`, so failures carry the module path next
# to the emitted `line:col` location.
#
# Force policy in `assert` mode: a check may force the checked value to weak
# head normal form and read its attribute *names*, never attribute values or
# list elements (even `drv` and `describe` probe by name, so a throwing
# attribute can never mask a diagnostic). The only force typed code adds
# over untyped code is WHNF of the annotated values themselves; element and
# field types are a future deep-checking mode's job.
{
  # "assert" runs the checks; "erase" turns both entry points into no-ops
  # (checker arguments are lazy, so erased checks cost nothing).
  mode,
}: let
  # Attr-name probe for derivations: `v.type or null` would force the value
  # of a `type` attribute, so a throwing member could mask the diagnostic.
  isDrvShaped = v: builtins.isAttrs v && v ? drvPath && v ? outPath;

  # Scalars show their value (strings truncated), so a failed refinement
  # like `Port` names the offending number, not just "int".
  describe = v: let
    t = builtins.typeOf v;
  in
    if isDrvShaped v
    then "derivation"
    else if t == "int" || t == "float" || t == "bool"
    then "${t} (${builtins.toJSON v})"
    else if t == "string"
    then "string (${builtins.toJSON (builtins.substring 0 40 v)})"
    else t;

  fieldNames = fields: builtins.concatStringsSep ", " (map (f: "`${f.name}`") fields);

  # A checker is `{ desc, check }`: `check loc v` returns true or throws a
  # positioned error. `loc` arrives as "<line>:<col> <what>" from the
  # converter; `path` comes from the importer.
  checkersFor = path: let
    fail = loc: expected: v:
      throw ".ix type error at ${path}:${loc}: expected ${expected}, got ${describe v}";
    mk = desc: pred: {
      inherit desc;
      check = loc: v: pred v || fail loc desc v;
    };
    prim = name: mk name (v: builtins.typeOf v == name);
  in {
    string = prim "string";
    int = prim "int";
    float = prim "float";
    bool = prim "bool";
    # Refinements borrowed from nixpkgs `lib.types` basics; each stays a
    # single WHNF-safe predicate.
    uint = mk "unsigned int" (v: builtins.isInt v && v >= 0);
    port = mk "port (0-65535)" (v: builtins.isInt v && v >= 0 && v <= 65535);
    path = mk "path" (
      v:
        builtins.typeOf v
        == "path"
        || (builtins.isString v && builtins.substring 0 1 v == "/")
    );
    nonEmptyString = mk "non-empty string" (v: builtins.isString v && v != "");
    func = mk "function" builtins.isFunction;
    any = {
      desc = "any";
      check = _: _: true;
    };
    drv = mk "derivation" isDrvShaped;
    enum = values: mk "one of ${builtins.toJSON values}" (v: builtins.elem v values);
    nullable = ty: {
      desc = "${ty.desc} | null";
      check = loc: v: v == null || ty.check loc v;
    };
    listOf = ty: mk "list of ${ty.desc}" builtins.isList;
    attrsOf = ty: mk "set of ${ty.desc}" builtins.isAttrs;
    req = name: ty: {
      required = true;
      inherit name ty;
    };
    opt = name: ty: {
      required = false;
      inherit name ty;
    };
    attrs = fields: let
      required = builtins.filter (f: f.required) fields;
      desc =
        if required == []
        then "set"
        else "set with ${fieldNames required}";
    in {
      inherit desc;
      check = loc: v:
        if !(builtins.isAttrs v)
        then fail loc desc v
        else let
          missing = builtins.filter (f: !(builtins.hasAttr f.name v)) required;
        in
          missing
          == []
          || throw ".ix type error at ${path}:${loc}: missing field(s) ${fieldNames missing}";
    };
  };

  assertForModule = path:
    checkersFor path
    // {
      arg = loc: ty: v: body: builtins.seq (ty.check loc v) body;
      ret = loc: ty: v: builtins.seq (ty.check loc v) v;
    };

  # Same attribute shape, no-op entry points: emitted source is identical
  # across modes, so the importer picks the cost at import time.
  eraseForModule = path:
    checkersFor path
    // {
      arg = _: _: _: body: body;
      ret = _: _: v: v;
    };
in
  if mode == "assert"
  then {forModule = assertForModule;}
  else if mode == "erase"
  then {forModule = eraseForModule;}
  else throw "ix-ty: unknown mode `${mode}`; expected \"assert\" or \"erase\""
