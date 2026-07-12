# Build efx plan IR from Nix — the terranix replacement.
#
# The nix layer keeps its old terranix role as the Turing-complete generator
# (inventory-derived records, list comprehensions, cross-file constants), but
# emits an `efx_ir::Plan` JSON document instead of terraform JSON. `efx plan
# --ir plan.json` / `efx apply --ir plan.json` consume the result; see
# packages/efx/README.md for the engine model.
#
# Two entry points:
#
# - `plan` / `effect` / `lit` / `ref`: build effects natively. Inputs are
#   scalars (auto-wrapped), explicit `lit`/`ref` values, or explicitly
#   JSON-encoded structures.
# - `fromTerranix`: translate a terranix-shaped config attrset
#   (`resource.<type>.<name> = {...}`) into a list of effects, so an existing
#   stack ports without restating every record. Terraform-interpolation
#   strings like "${cloudflare_zone.ix_dev.id}" become first-class efx
#   references; `terraform` / `provider` / `import` blocks are dropped
#   (backend state is the journal, provider auth is executor environment,
#   import is a journal-seeding question efx does not have yet).
#
# The translation is total-or-loud: any value it cannot represent faithfully
# (floats, nulls, interpolations embedded mid-string or inside structured
# values) throws at eval time instead of emitting a plan that means something
# else.
{
  lib,
  lists,
}: let
  scalarType = value:
    if builtins.isString value
    then "string"
    else if builtins.isBool value
    then "bool"
    else if builtins.isInt value
    then "int"
    else null;

  isTagged = value:
    builtins.isAttrs value
    && (builtins.attrNames value == ["literal"] || builtins.attrNames value == ["ref"]);

  # A terraform interpolation that is exactly one resource-output read.
  # Anything else containing "${" is unrepresentable and throws at the use
  # site (with the input path) rather than here. Bracket expressions rather
  # than backslash escapes: POSIX ERE (what builtins.match speaks) rejects
  # the `\{` form.
  refMatch = value:
    if builtins.isString value
    then builtins.match "[$][{]([A-Za-z0-9_]+)[.]([A-Za-z0-9_-]+)[.]([A-Za-z0-9_]+)[}]" value
    else null;

  lit = value:
    if scalarType value == null
    then throw "efx.lit: only strings, ints, and bools are literals; JSON-encode structured values explicitly (builtins.toJSON) or wire them with efx.ref"
    else {literal = value;};

  ref = effect: field: {ref = {inherit effect field;};};

  normalizeInput = effectName: key: value:
    if isTagged value
    then value
    else if scalarType value != null
    then lit value
    else throw "efx.effect: input `${key}` of `${effectName}` is structured; JSON-encode it explicitly (builtins.toJSON) or wire it with efx.ref";

  effect = {
    name,
    kind,
    executor ? kind,
    inputs ? {},
    idempotent ? true,
    rollbackHint ? null,
  }: {
    inherit name kind executor;
    inputs = lib.mapAttrs (normalizeInput name) inputs;
    meta = {
      inherit idempotent;
      rollback_hint = rollbackHint;
    };
  };

  plan = effects: let
    names = map (e: e.name) effects;
    duplicates = lists.findDuplicates names;
    known = lib.genAttrs names (_: true);
    danglingRefs =
      lib.concatMap (
        e:
          lib.concatMap (
            value:
              lib.optional ((value ? ref) && !(builtins.hasAttr value.ref.effect known))
              "`${e.name}` references unknown effect `${value.ref.effect}`"
          ) (lib.attrValues e.inputs)
      )
      effects;
  in
    if duplicates != []
    then throw "efx.plan: duplicate effect names: ${lib.concatStringsSep ", " duplicates}"
    else if danglingRefs != []
    then throw "efx.plan: ${lib.concatStringsSep "; " danglingRefs}"
    else {inherit effects;};

  # --- terranix translation --------------------------------------------------

  # `<provider>_<rest>` -> `<provider>.<rest>`; `local_file` additionally maps
  # onto the native `file.write` executor (same effect, no provider involved).
  defaultKindFor = resourceType: let
    parts = builtins.match "([a-z0-9]+)_(.+)" resourceType;
  in
    if resourceType == "local_file"
    then "file.write"
    else if parts == null
    then throw "efx.fromTerranix: cannot derive an executor kind from resource type `${resourceType}`; pass it via typeMap"
    else "${builtins.elemAt parts 0}.${builtins.elemAt parts 1}";

  # local_file's `filename` is `file.write`'s `path`.
  defaultRenames = {
    local_file.filename = "path";
  };

  translateLeaf = path: value: let
    parsed = refMatch value;
    described = lib.concatStringsSep "." path;
  in
    if isTagged value
    # A native efx value (an efx.ref to a non-terraform effect, say) embedded
    # in a ported resource passes through untouched.
    then value
    else if parsed != null
    then ref "${builtins.elemAt parsed 0}.${builtins.elemAt parsed 1}" (builtins.elemAt parsed 2)
    else if builtins.isString value && lib.hasInfix "\${" value
    then throw "efx.fromTerranix: `${described}` embeds a terraform interpolation inside a larger string; restate it as an efx input wired with efx.ref"
    else if builtins.isList value
    then
      if lib.hasInfix "\${" (builtins.toJSON value)
      then throw "efx.fromTerranix: `${described}` carries a reference inside a structured value, which the efx IR cannot express; hoist the reference to a top-level input or declare the effect natively"
      else lit (builtins.toJSON value)
    else if scalarType value != null
    then lit value
    else throw "efx.fromTerranix: `${described}` is ${builtins.typeOf value}-typed, which has no faithful IR encoding";

  # Nested attrsets flatten to dotted input keys (`account.id`,
  # `data.flags`), matching the executor contracts; lists stay JSON-encoded
  # strings the executor parses.
  flattenInputs = path: value:
    if builtins.isAttrs value && !isTagged value
    then
      lib.concatLists (
        lib.mapAttrsToList (key: flattenInputs (path ++ [key])) value
      )
    else [(lib.nameValuePair (lib.concatStringsSep "." path) (translateLeaf path value))];

  # Top-level terranix keys with no efx meaning. Dropped silently because
  # each has a documented replacement: state backends -> the journal file,
  # provider blocks -> executor environment (secrets never enter the plan),
  # import blocks -> nothing yet (efx executors reconcile against live state
  # instead of adopting it into a state file).
  droppedTopLevel = ["terraform" "provider" "import" "output" "variable" "_module"];

  fromTerranix = {
    config,
    typeMap ? {},
    renames ? {},
  }: let
    unknown =
      lib.filter (key: !(lib.elem key (droppedTopLevel ++ ["resource"])))
      (builtins.attrNames config);
    kindFor = resourceType: typeMap.${resourceType} or (defaultKindFor resourceType);
    renamesFor = resourceType: (defaultRenames // renames).${resourceType} or {};
    translateResource = resourceType: resourceName: attrs: let
      rename = renamesFor resourceType;
      inputs = lists.genAttrs' (flattenInputs [] attrs) (
        entry: entry // {name = rename.${entry.name} or entry.name;}
      );
    in
      effect {
        name = "${resourceType}.${resourceName}";
        kind = kindFor resourceType;
        inherit inputs;
      };
  in
    if unknown != []
    then throw "efx.fromTerranix: unsupported top-level keys: ${lib.concatStringsSep ", " unknown}"
    else
      lib.concatLists (
        lib.mapAttrsToList (
          resourceType: byName:
            lib.mapAttrsToList (translateResource resourceType) byName
        ) (config.resource or {})
      );
in {
  inherit effect fromTerranix lit plan ref;
}
