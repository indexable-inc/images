{lib}: let
  enums = {
    class = [
      "deployed-service"
      "distributed-cli"
      "base-image"
      "dev-tool"
      "cache-only"
    ];
    environment = [
      "production"
      "staging"
      "development"
      "none"
    ];
    exposure = [
      "internet"
      "internal"
      "local"
      "none"
    ];
    criticality = [
      "critical"
      "high"
      "medium"
      "low"
    ];
  };

  requireString = field: value:
    assert lib.assertMsg (
      builtins.isString value && value != ""
    ) "security root `${field}` must be a non-empty string"; value;

  requireEnum = field: value:
    assert lib.assertMsg (
      builtins.elem value enums.${field}
    ) "security root `${field}` must be one of: ${lib.concatStringsSep ", " enums.${field}}"; value;

  mkRoot = {
    attr,
    name,
    class,
    owner,
    environment,
    exposure,
    criticality,
    slaHours,
  }:
    assert lib.assertMsg (
      builtins.isInt slaHours && slaHours > 0
    ) "security root `slaHours` must be a positive integer"; {
      attr = requireString "attr" attr;
      name = requireString "name" name;
      class = requireEnum "class" class;
      owner = requireString "owner" owner;
      environment = requireEnum "environment" environment;
      exposure = requireEnum "exposure" exposure;
      criticality = requireEnum "criticality" criticality;
      inherit slaHours;
    };
in {
  inherit enums mkRoot;
}
