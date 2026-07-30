/**
VM templates: the `templates` and `instances` exports of a `default.ix`
config, rendered into VMs. RFC 0042, tracked by ix#9242.

A template is a FUNCTION from a params attrset to what `index.lib.mkVm`
returns, so nothing here interprets a template -- it calls it. That is why the
design needs no substitution language over a directory of files, and why this
file is naming and guards rather than a renderer.

Two identities, and keeping them apart is most of the work:

  - the INSTANCE name, `worker@1`: systemd's `unit@instance` spelling. It is
    what a user writes in the `instances` block, what `ix new worker 1` will
    create, and what the server will key its record on.
  - the NODE name, `worker-1`: the `nixosConfigurations` key, the guest's
    `networking.hostName`, and the OCI repository its image is pushed to.

RFC 0042 spells the two as one thing (`mkVm({ name: `worker@${instance}` })`)
and that cannot evaluate. nixpkgs types `networking.hostName` as a DNS label
(`strMatching "^$|^[[:alnum:]]([[:alnum:]_-]{0,61}[[:alnum:]])?$"`), so a node
named `worker@1` fails its own option type before any image is built --
measured, not reasoned about. In an OCI reference `@` introduces a digest, so
`registry.ix.dev/worker@1:tag` is not a repository a manifest can be pushed to
either. Nor can `-` replace `@` in the instance name: template `worker-pool`
instance `1` and template `worker` instance `pool-1` would spell one string,
which is exactly why systemd picked a separator that appears in neither half.
The two spellings are therefore forced by the constraints rather than chosen,
and this file owns the mapping between them so that no template author has to
know it.
*/
{
  lib,
  errors,
}: let
  context = "index.lib.templates";

  # systemd's separator, borrowed wholesale (`getty@tty3`), and the node
  # name's, which cannot be the same character. Bound once each so the parse
  # and the render cannot drift; a second literal is a place they can.
  instanceSeparator = "@";
  nodeSeparator = "-";

  /**
  Whether `candidate` is a name `networking.hostName` will accept, which is
  what a node name has to be: `mkVm` sets that option from it, so a node name
  the option rejects is a config that cannot evaluate.

  Public because the ix CLI needs the same answer about a `--name` before it
  starts nix, and a second implementation of this rule is a second place it can
  drift.
  */
  isNodeName = candidate: builtins.match nodeNamePattern candidate != null;

  # The rule itself, MIRRORED FROM NIXPKGS RATHER THAN DERIVED, which is a
  # liability a reviewer now carries.
  #
  # nixpkgs declares it inline on the option --
  # `types.strMatching "^$|^[[:alnum:]]([[:alnum:]_-]{0,61}[[:alnum:]])?$"` in
  # nixos/modules/tasks/network-interfaces.nix -- so the only two handles on it
  # are the option's own `type.check`, which needs an evaluated NixOS option set
  # rather than the bare `lib` this file takes, and the pattern text. If nixpkgs
  # ever grows a `lib.types.hostName`, delete this and call that instead.
  #
  # Until then the drift is caught rather than trusted: `tests/default.nix`
  # extracts this pattern from that nixpkgs file and asserts `isNodeName` agrees
  # with it verdict for verdict, so a bump that tightens the rule fails the eval
  # suite instead of leaving this guard passing a name the option then rejects.
  #
  # That failure is not hypothetical: it shipped. #4452 checked each HALF of an
  # instance name against `[[:alnum:]][[:alnum:]_-]*`, which accepted `worker@1-`
  # and `worker@a_` and any joined name over 63 characters, and the error then
  # arrived from `networking.hostName` two module layers down -- the one thing
  # the check exists to prevent. Checking each half was checking a proxy; the
  # node name is the string nixpkgs will type, so that is the string checked.
  #
  # The `^$` alternative is dropped deliberately: an empty hostname means "ask
  # DHCP for one", which a name we constructed can never want.
  nodeNamePattern = "[[:alnum:]]([[:alnum:]_-]{0,61}[[:alnum:]])?";

  # Implied by the pattern (one leading character, up to 61 middle, one
  # trailing), and deliberately NOT re-checked inside `isNodeName`: the pattern
  # already refuses anything longer, so a second length test there would let this
  # number be wrong with no check able to notice -- a constant asserted against
  # itself. It exists for the error message, the one place that has to say by HOW
  # MUCH to shorten, and a test pins it to the pattern by walking a name of
  # exactly this length and one character more.
  nodeNameMaxLength = 63;

  /**
  Join a template name and an instance id into an instance name (`worker@1`).

  The inverse of `parseInstanceName`, and public for the same reason: the ix CLI
  forms this string every time it names a record, and a second implementation of
  it is a second place the separator can be respelled. Deliberately NOT the node
  name's constructor: the node name is already `parseInstanceName`'s `node`
  field, and offering a second way to spell it invites a caller to build one
  without going through the checks.

  This direction is injective and the node name's is not, which is the whole
  reason for two spellings; `tests/default.nix` asserts both halves of that.
  */
  instanceNameOf = {
    template,
    instance,
  }: "${template}${instanceSeparator}${instance}";

  /**
  Split an instance name (`worker@1`) into the template it instantiates, the
  instance id (systemd's `%i`), and the node name its VM is created under.

  Returns `{ template; instance; node; }`. Throws, naming the offending string
  and the node name it would have rendered, on anything that is not
  `<template>@<instance>` whose joined node name `isNodeName` accepts.
  */
  parseInstanceName = name: let
    parts = lib.splitString instanceSeparator name;
    template = builtins.head parts;
    instance = builtins.elemAt parts 1;
    node = "${template}${nodeSeparator}${instance}";
    # Positive when the joined name is too long, and by how much, which is the
    # number the caller needs rather than the fact that a limit exists.
    overBy = builtins.stringLength node - nodeNameMaxLength;
    lengthOf = part: builtins.toString (builtins.stringLength part);
  in
    # The two per-half checks, and the only two that belong per half: how many
    # separators there are, and that each side has something in it. Everything
    # else about the halves is a property of the string they join into, so it is
    # checked there.
    assert lib.assertMsg (builtins.length parts == 2) ''
      ix: ${context}
        Instance name '${name}' is not '<template>${instanceSeparator}<instance>'.
        Exactly one '${instanceSeparator}', the template name before it, the instance id after, as in systemd's getty@tty3.
    '';
    assert lib.assertMsg (template != "" && instance != "") ''
      ix: ${context}
        Instance name '${name}': both halves must be non-empty.
        The template name goes before the '${instanceSeparator}' and the instance id after it.
    '';
    # Length before shape. `isNodeName` below rejects an over-long name on its
    # own, so this assert is not what makes the rule hold -- it is what makes the
    # failure actionable, being the only message that can name both halves and
    # the number of characters to cut.
    assert lib.assertMsg (overBy <= 0) ''
      ix: ${context}
        Instance name '${name}' renders the node name '${node}', which is ${lengthOf node} characters. A hostname is at most ${builtins.toString nodeNameMaxLength}.
        Shorten the template name ('${template}', ${lengthOf template}) or the instance id ('${instance}', ${lengthOf instance}) by ${builtins.toString overBy} characters between them.
    '';
    assert lib.assertMsg (isNodeName node) ''
      ix: ${context}
        Instance name '${name}' renders the node name '${node}', which is not a legal hostname.
        It has to begin and end alphanumeric, with letters, digits, '-' and '_' in between: `mkVm` sets `networking.hostName` from the node name, and nixpkgs types that option exactly that way.
    ''; {
      inherit template instance node;
    };

  /**
  Render one instance: look `name`'s template up in `templates` and call it
  with `params`.

  Returns exactly what the template's own `index.lib.mkVm` call returned, so
  every consumer of that shape (`nixosConfigurations`, `planValue`, the
  lifecycle wrappers) keeps working unchanged. This is the seam `ix new` and
  apply's per-instance re-render will go through when they exist: a recorded
  instance is a `name` plus a `params` attrset, and rendering it is this call.

  Two values are injected into the params attrset rather than read out of it,
  because they are the instance's identity and not its configuration:

  - `instance`, systemd's `%i`: the id after the `@`.
  - `name`, the node name. `index.lib.mkVm({ name, ... })` is then the whole
    of what a template writes, so the node separator stays this file's
    business. RFC 0042 has the template build the name from `instance`
    instead, which puts the same spelling in every template in the fleet and
    is how they drift.

  Params are otherwise unchecked here, deliberately. `builtins.functionArgs`
  cannot see a pattern's ellipsis (`{ a, ... }` and `{ a }` both report
  `{ a = false; }`), so an eval-side strict-params check would reject a
  template that legally accepts extra keys -- and nix already rejects an
  unexpected param naming it, and a missing required one likewise. The checked
  version of this boundary is the CLI's, pre-eval, against the JSON Schema
  ix2nix now generates from the same annotations (index#4450); the template in
  `examples/templates/workers` carries none yet, and RFC 0042's Typed params
  section states what an annotated one would and would not buy today.
  */
  renderInstance = {
    templates,
    name,
    params ? {},
  }: let
    parsed = parseInstanceName name;
    template = errors.requireAttr {
      context = "${context}.renderInstance: instance '${name}'";
      attrset = templates;
      key = parsed.template;
    };
    rendered = template ({
        inherit (parsed) instance;
        name = parsed.node;
      }
      // params);
  in
    assert lib.assertMsg (builtins.isAttrs params) ''
      ix: ${context}
        Instance '${name}': params must be an attrset of JSON-shaped values, got ${builtins.typeOf params}.
    '';
    assert lib.assertMsg (!(params ? instance) && !(params ? name)) ''
      ix: ${context}
        Instance '${name}': 'instance' and 'name' are injected from the instance name, so params must not set them.
        A params entry disagreeing with the name it was rendered under would make the config lie; rejecting is louder than picking a winner.
    '';
    assert lib.assertMsg (rendered ? nixosConfigurations) ''
      ix: ${context}
        Instance '${name}': template '${parsed.template}' returned no 'nixosConfigurations'.
        A template returns what index.lib.mkVm returns, so its body ends in an mkVm call.
    '';
    assert lib.assertMsg (builtins.attrNames rendered.nixosConfigurations == [parsed.node]) ''
      ix: ${context}
        Instance '${name}': template '${parsed.template}' rendered ${
        builtins.toString (builtins.length (builtins.attrNames rendered.nixosConfigurations))
      } VM(s) named ${
        lib.concatMapStringsSep ", " (node: "'${node}'") (builtins.attrNames rendered.nixosConfigurations)
      }, expected exactly '${parsed.node}'.
        The injected 'name' param already is that string: index.lib.mkVm({ name, ... }) is the whole fix.
        '${instanceSeparator}' names the instance, never the VM: it is not legal in a DNS label and introduces a digest in an OCI reference.
    ''; rendered;

  /**
  Render a whole config's exports: every `instances` entry through its
  template, merged with the config's own named VMs.

  Takes the value a `default.ix` returns and gives back
  `{ nixosConfigurations; instances; systemPackages; }`, so the merge and its
  collision check live here rather than in every `flake.nix` that would
  otherwise spread two attrsets together and silently let one win.

  A config exporting neither key passes through unchanged --
  `nixosConfigurations` is exactly its own and `instances` is empty -- which
  is the property that keeps every config written before this feature
  evaluating as it does today.

  What it deliberately does not do is create or destroy anything: it renders
  what the repo declares. The recorded-instance half of RFC 0042's
  reconciliation is server-side state and lives in ix, so a `default.ix`
  applied today converges its `instances` block and nothing else.
  */
  renderConfig = config: let
    templates = config.templates or {};
    declared = config.instances or {};
    named = config.nixosConfigurations or {};

    instances =
      lib.mapAttrs (
        name: params: renderInstance {inherit templates name params;}
      )
      declared;
    # Collision-free among themselves by construction: each rendered result
    # holds exactly the one node its own key names (asserted above), and the
    # keys are attribute names.
    instanceNodes = lib.concatMapAttrs (_name: vm: vm.nixosConfigurations) instances;
    # Keys, which are node names because `mkVm` keys its `nixosConfigurations`
    # by node name and a config spreads that attrset unchanged. The one way this
    # becomes a proxy for what it claims is a config that re-keys an `mkVm`
    # result by hand, putting a VM named `a` under the key `b`; then the
    # collision this compares is between keys rather than between the names two
    # guests will answer to. Nothing in the tree does that, and the rendered-node
    # assertion in `renderInstance` refuses it for instances, so a named VM is
    # the only place it could hide.
    collisions = builtins.attrNames (builtins.intersectAttrs named instanceNodes);
    nixosConfigurations = named // instanceNodes;
  in
    assert lib.assertMsg (declared == {} || templates != {}) ''
      ix: ${context}
        The config exports 'instances' (${lib.concatStringsSep ", " (builtins.attrNames declared)}) but no 'templates', so there is nothing to render them from.
    '';
    assert lib.assertMsg (collisions == []) ''
      ix: ${context}
        Instance node name(s) collide with a VM the config already names: ${lib.concatStringsSep ", " collisions}.
        Merging them would silently drop one of the two, so rename the template, the instance, or the named VM.
    ''; {
      inherit instances nixosConfigurations;
      # `<node>-system` for every node this config produces, instantiated and
      # named alike, so `nix build .#worker-1-system` exists before
      # `ix apply .#worker-1` needs it. Same spelling `mkVm` gives a named VM
      # (lib/image/fleet.nix), and lazy, so an unbuilt one costs nothing.
      systemPackages =
        lib.mapAttrs' (
          node: entry: lib.nameValuePair "${node}-system" entry.config.system.build.toplevel
        )
        nixosConfigurations;
    };
in {
  inherit
    instanceNameOf
    isNodeName
    parseInstanceName
    renderConfig
    renderInstance
    ;
  # Exported for the drift guard in `tests/default.nix`, which pins the length
  # to the pattern through this binding rather than restating 63, and for a CLI
  # that wants to say how long a name may be before offering to make one.
  inherit nodeNameMaxLength;
}
