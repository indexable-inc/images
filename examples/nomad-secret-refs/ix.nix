{ index }:

let
  ix = index.lib;
  inherit (ix) pkgs;
  inherit (pkgs) lib;
  mkSecretSet =
    mountRoot:
    ix.secrets.normalize {
      provider = {
        type = "vaultwarden";
        client = "rbw";
        server = "https://vaultwarden.internal.example";
        inherit mountRoot;
        folder = "production";
      };

      values."daily-scraper/aws.env" = {
        key = "daily-scraper/aws-env";
        field = "notes";
        format = "env";
      };
    };
  secretSet = mkSecretSet "/run/ix-secrets";

  mkNomadJob =
    refs:
    ix.secrets.consumers.nomad.renderJob {
      name = "daily-scraper";
      image = "registry.ix.dev/indexable-inc/daily-scraper:latest";
      envSecretRefs = {
        "aws.env" = refs.values."daily-scraper/aws.env";
      };
    };
  nomadJob = mkNomadJob secretSet;
  checkSecrets = ix.secrets.providers.vaultwarden.rbwCheckCommand secretSet.plan;
  materializeSecrets = ix.secrets.providers.vaultwarden.rbwMaterializeCommand secretSet.plan;
  validateNomadJob = ix.secrets.consumers.nomad.runCommand {
    name = "daily-scraper";
    inherit secretSet;
    job = nomadJob;
  };
  planValue = secretSet.plan.values."daily-scraper/aws.env";
  nomadTemplate = lib.findFirst (
    template: template.destination == "secrets/aws.env"
  ) { } nomadJob.passthru.templates;
  buildChecks = {
    checkUsesRealRbw = checkSecrets.passthru.rbwProgram == lib.getExe pkgs.rbw;
    materializerUsesRealRbw = materializeSecrets.passthru.rbwProgram == lib.getExe pkgs.rbw;
    checkTargetsProviderKey = planValue.key == "daily-scraper/aws-env";
    checkTargetsField = planValue.field == "notes";
    checkTargetsFolder = (planValue.folder or secretSet.plan.provider.folder) == "production";
    nomadReadsMaterializedFile =
      (nomadTemplate.source or null) == "/run/ix-secrets/daily-scraper/aws.env";
    nomadLoadsEnvFile = (nomadTemplate.destination or null) == "secrets/aws.env";
  };
  failedBuildChecks = lib.attrNames (lib.filterAttrs (_name: passed: !passed) buildChecks);
in
{
  inherit
    checkSecrets
    materializeSecrets
    nomadJob
    secretSet
    validateNomadJob
    ;

  inherit buildChecks;

  buildCheck =
    assert lib.assertMsg (
      failedBuildChecks == [ ]
    ) "nomad-secret-refs build checks failed: ${lib.concatStringsSep ", " failedBuildChecks}";
    pkgs.runCommand "nomad-secret-refs-build-check" { } ''
      mkdir -p "$out"
    '';

  kubernetesExternalSecret = ix.secrets.consumers.kubernetes.renderExternalSecret {
    name = "daily-scraper-aws";
    namespace = "batch";
    secretStoreRef = {
      name = "vaultwarden";
      kind = "ClusterSecretStore";
    };
    values = {
      AWS_ACCESS_KEY_ID = secretSet.values."daily-scraper/aws.env" // {
        key = "daily-scraper/aws-access-key-id";
      };
      AWS_SECRET_ACCESS_KEY = secretSet.values."daily-scraper/aws.env" // {
        key = "daily-scraper/aws-secret-access-key";
      };
    };
  };
}
