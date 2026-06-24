{ lib }:
let
  segment =
    name: value:
    assert lib.assertMsg (
      !lib.hasInfix "/" value
    ) "Vaultwarden ${name} segment must not contain '/': ${value}";
    value;

  pattern = "^bw://[^/]+/[^/]+/[^/]+$";

  mkRef =
    {
      folder,
      item,
      field,
    }:
    "bw://${segment "folder" folder}/${segment "item" item}/${segment "field" field}";
in
{
  inherit pattern mkRef;
  isRef = ref: builtins.match pattern ref != null;
  type = lib.types.strMatching pattern;
}
