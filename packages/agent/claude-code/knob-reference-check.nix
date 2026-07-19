# Drift guard for the commented knob/env reference blocks in
# packages/agent/home-manager/claude-code.nix (index#3710). The blocks
# enumerate every wrapper knob and env var with its stock default; unknown
# keys already throw inside the wrapper, so what this check adds is the
# missing direction (a knob the wrapper grew that the reference does not
# list) plus stale defaults for the features/systemTools tables and a stale
# env extraction after a version bump. Everything asserts at eval, so
# `nix flake check` (and blast-radius) goes red on drift without building
# anything.
{
  lib,
  pkgs,
  claudeCode,
  # Path to the Home Manager module carrying the reference blocks, threaded
  # by the caller (a `../` literal here would reach across directories).
  hmModule,
}: let
  # The wrapper's accepted surface: a knob is exactly a formal with a
  # default. Plumbing formals (lib, ix, stdenv, ...) have none, so the
  # distinction is mechanical, not a hand-kept list.
  wrapperArgs = builtins.functionArgs (import ./default.nix);
  acceptedKnobs = builtins.attrNames (lib.filterAttrs (_: hasDefault: hasDefault) wrapperArgs);
  acceptedFeatures = claudeCode.knobDefaults.features;
  acceptedSystemTools = claudeCode.knobDefaults.systemTools;
  manifest = lib.importJSON ./manifest.json;

  lines = lib.splitString "\n" (builtins.readFile hmModule);
  sectionBetween = name: let
    beginIdx = lib.lists.findFirstIndex (lib.hasInfix "BEGIN ${name}") null lines;
    endIdx = lib.lists.findFirstIndex (lib.hasInfix "END ${name}") null lines;
  in
    if beginIdx == null || endIdx == null
    then throw "claude-code knob reference: BEGIN/END markers for `${name}` not found in packages/agent/home-manager/claude-code.nix"
    else lib.sublist (beginIdx + 1) (endIdx - beginIdx - 1) lines;

  knobSection = sectionBetween "claude-code wrapper knob reference";

  # Reference rows are exactly `#   <key> = <value>;  <note>` (three spaces
  # after the hash); prose header lines use a single space and never match.
  entriesMatching = prefix:
    lib.concatMap (
      line: let
        m = builtins.match "[[:space:]]*#   ${prefix}([A-Za-z0-9]+) = ([^;]*);.*" line;
      in
        lib.optional (m != null) {
          name = builtins.elemAt m 0;
          value = builtins.elemAt m 1;
        }
    )
    knobSection;

  listedKnobs = map (e: e.name) (entriesMatching "");
  listedFeatures = entriesMatching "features\\.";
  listedSystemTools = entriesMatching "systemTools\\.";

  renderDefault = v:
    if v == null
    then "null"
    else if builtins.isBool v
    then lib.boolToString v
    else toString v;

  sameNames = what: listed: accepted: let
    missing = lib.subtractLists listed accepted;
    extra = lib.subtractLists accepted listed;
  in
    lib.assertMsg (missing == [] && extra == [])
    "claude-code knob reference (${what}): missing [${toString missing}]; unknown [${toString extra}]";

  sameValues = what: listed: accepted: let
    stale = lib.filter (e: renderDefault accepted.${e.name} != e.value) listed;
  in
    lib.assertMsg (stale == [])
    "claude-code knob reference (${what}): stale defaults for [${toString (map (e: e.name) stale)}]";

  # The env block enumerates a moving upstream surface (the cli.js registry
  # cross-checked against the docs), so staleness is pinned by version
  # marker rather than key set: both the block's BEGIN line and the
  # committed TSV must name the pinned CLI version.
  versionMarker = "cli.js ${manifest.version}";
  envBlockCurrent = lib.any (line: lib.hasInfix "BEGIN claude-code env reference" line && lib.hasInfix versionMarker line) lines;
  tsvCurrent = lib.hasInfix versionMarker (builtins.readFile ./env-registry.tsv);
in
  assert sameNames "wrapper knobs" listedKnobs acceptedKnobs;
  assert sameNames "features" (map (e: e.name) listedFeatures) (builtins.attrNames acceptedFeatures);
  assert sameNames "systemTools" (map (e: e.name) listedSystemTools) (builtins.attrNames acceptedSystemTools);
  assert sameValues "features" listedFeatures acceptedFeatures;
  assert sameValues "systemTools" listedSystemTools acceptedSystemTools;
  assert lib.assertMsg envBlockCurrent
  "claude-code env reference: block not marked as extracted from ${versionMarker}; re-extract after the version bump";
  assert lib.assertMsg tsvCurrent
  "claude-code env-registry.tsv: not generated from ${versionMarker}; run `nix build .#claude-code.envRegistry` and copy the result";
    pkgs.runCommand "claude-code-knob-reference-check" {} "mkdir -p $out"
