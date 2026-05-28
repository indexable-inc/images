{
  lib,
  pkgs,
  writeNushellApplication,
}:
let
  providerDefaults = {
    type = "runtime-directory";
    mountRoot = "/run/secrets";
  };

  isSafeRelativeSegment =
    segment: segment != "" && segment != "." && segment != ".." && !(lib.hasInfix "/" segment);

  validateReferenceName =
    name:
    let
      segments = lib.splitString "/" name;
    in
    assert lib.assertMsg (lib.all isSafeRelativeSegment segments)
      "secret reference '${name}' must be a relative path with no empty, '.', or '..' segments";
    name;

  normalizeValue =
    provider: name: value:
    let
      checkedName = validateReferenceName name;
      attrs =
        if builtins.isAttrs value then
          value
        else
          {
            key = value;
          };
      key = attrs.key or checkedName;
      path = attrs.path or "${provider.mountRoot}/${checkedName}";
    in
    assert lib.assertMsg (
      builtins.isString key && key != ""
    ) "secret reference '${checkedName}' must set a non-empty provider key";
    assert lib.assertMsg (
      builtins.isString path && lib.hasPrefix "/" path
    ) "secret reference '${checkedName}' path must be absolute";
    attrs
    // {
      inherit key path;
    };

  normalize =
    spec:
    let
      provider = providerDefaults // (spec.provider or spec.backend or { });
      rawValues =
        spec.values or (removeAttrs spec [
          "backend"
          "provider"
          "values"
        ]);
      values = lib.mapAttrs (normalizeValue provider) rawValues;
    in
    assert lib.assertMsg (
      builtins.isString provider.type && provider.type != ""
    ) "secret provider type must be a non-empty string";
    assert lib.assertMsg (
      builtins.isString provider.mountRoot && lib.hasPrefix "/" provider.mountRoot
    ) "secret provider mountRoot must be an absolute path";
    {
      inherit provider values;
      refs = lib.mapAttrs (_name: value: value.path) values;
      plan = {
        inherit provider values;
      };
    };

  rbwCheckCommand =
    args:
    let
      inherit (args) provider values;
      rbwProgram = args.rbwProgram or (lib.getExe pkgs.rbw);
      command = writeNushellApplication pkgs {
        name = "check-secret-refs";
        text = ''
          def check-secret [key: string, field: any, folder: any] {
            let folder_args = if ($folder | is-empty) { [] } else { [--folder $folder] }
            let value_args = if ($field | is-empty) { [--raw] } else { [--field $field] }
            ^${rbwProgram} get ...$folder_args ...$value_args $key | ignore
          }

          def --wrapped main [...names: string] {
            let requested = if ($names | is-empty) {
              ${builtins.toJSON (lib.attrNames values)}
            } else {
              $names
            }
            let known = ${builtins.toJSON (lib.attrNames values)}
            let keys = ${builtins.toJSON (lib.mapAttrs (_name: value: value.key) values)}
            let fields = ${builtins.toJSON (lib.mapAttrs (_name: value: value.field or null) values)}
            let folders = ${
              builtins.toJSON (lib.mapAttrs (_name: value: value.folder or provider.folder or null) values)
            }

            for name in $requested {
              if not ($name in $known) {
                error make { msg: $"unknown secret ref ($name)" }
              }
              let key = ($keys | get $name)
              let field = ($fields | get $name)
              let folder = ($folders | get $name)
              check-secret $key $field $folder
            }
          }
        '';
        meta.description = "Fail fast when declared ${provider.type} secret refs are missing";
      };
    in
    command
    // {
      passthru = (command.passthru or { }) // {
        secretPlan = {
          inherit provider values;
        };
        inherit rbwProgram;
      };
    };

  rbwMaterializeCommand =
    args:
    let
      inherit (args) provider values;
      rbwProgram = args.rbwProgram or (lib.getExe pkgs.rbw);
      command = writeNushellApplication pkgs {
        name = "materialize-secret-refs";
        text = ''
          def read-secret [key: string, field: any, folder: any] {
            let folder_args = if ($folder | is-empty) { [] } else { [--folder $folder] }
            if ($field | is-empty) {
              ^${rbwProgram} get ...$folder_args $key
            } else {
              ^${rbwProgram} get ...$folder_args --field $field $key
            }
          }

          def --wrapped main [...names: string] {
            let requested = if ($names | is-empty) {
              ${builtins.toJSON (lib.attrNames values)}
            } else {
              $names
            }
            let known = ${builtins.toJSON (lib.attrNames values)}
            let keys = ${builtins.toJSON (lib.mapAttrs (_name: value: value.key) values)}
            let paths = ${builtins.toJSON (lib.mapAttrs (_name: value: value.path) values)}
            let fields = ${builtins.toJSON (lib.mapAttrs (_name: value: value.field or null) values)}
            let folders = ${
              builtins.toJSON (lib.mapAttrs (_name: value: value.folder or provider.folder or null) values)
            }

            for name in $requested {
              if not ($name in $known) {
                error make { msg: $"unknown secret ref ($name)" }
              }

              let key = ($keys | get $name)
              let path = ($paths | get $name)
              let field = ($fields | get $name)
              let folder = ($folders | get $name)
              mkdir ($path | path dirname)
              read-secret $key $field $folder | save --force $path
              chmod 0600 $path
            }
          }
        '';
        meta.description = "Materialize declared ${provider.type} secret refs through rbw";
      };
    in
    command
    // {
      passthru = (command.passthru or { }) // {
        secretPlan = {
          inherit provider values;
        };
        inherit rbwProgram;
      };
    };

  nomadEnvTemplates =
    values:
    lib.mapAttrsToList (name: value: {
      source = value.path;
      destination = "secrets/${name}";
      env = value.format or "env";
    }) values;

  renderNomadTemplate = template: ''
    template {
      source      = ${builtins.toJSON template.source}
      destination = ${builtins.toJSON template.destination}
      env         = ${if template.env == "env" then "true" else "false"}
    }
  '';

  renderNomadJob =
    {
      name,
      datacenters ? [ "dc1" ],
      group ? name,
      task ? name,
      driver ? "docker",
      image,
      envSecretRefs ? { },
      config ? { },
    }:
    let
      configJson = builtins.toJSON ({ inherit image; } // config);
      templates = nomadEnvTemplates envSecretRefs;
      job = pkgs.writeText "nomad-${name}.hcl" ''
        job ${builtins.toJSON name} {
          datacenters = ${builtins.toJSON datacenters}

          group ${builtins.toJSON group} {
            task ${builtins.toJSON task} {
              driver = ${builtins.toJSON driver}

              config = ${configJson}

        ${lib.concatStringsSep "\n" (map renderNomadTemplate templates)}
            }
          }
        }
      '';
    in
    job
    // {
      passthru = (job.passthru or { }) // {
        inherit
          config
          datacenters
          driver
          group
          image
          name
          task
          templates
          ;
      };
    };

  nomadRunCommand =
    {
      name,
      secretSet,
      job,
      nomadProgram ? "nomad",
      rbwProgram ? lib.getExe pkgs.rbw,
      run ? false,
    }:
    let
      checkSecrets = rbwCheckCommand (secretSet.plan // { inherit rbwProgram; });
      materializeSecrets = rbwMaterializeCommand (secretSet.plan // { inherit rbwProgram; });
      action = if run then "run" else "validate";
      command = writeNushellApplication pkgs {
        name = "nomad-${name}-secrets-${action}";
        runtimeInputs = [
          checkSecrets
          materializeSecrets
        ];
        text = ''
          def --wrapped main [...args] {
            check-secret-refs
            materialize-secret-refs
            ^${nomadProgram} job ${action} ${job} ...$args
          }
        '';
        meta.description = "Check Vaultwarden refs, materialize files, then nomad job ${action}";
      };
    in
    command
    // {
      passthru = (command.passthru or { }) // {
        inherit
          action
          checkSecrets
          job
          materializeSecrets
          nomadProgram
          rbwProgram
          secretSet
          ;
      };
    };

  renderKubernetesExternalSecret =
    {
      name,
      namespace ? "default",
      secretStoreRef,
      values,
    }:
    (pkgs.formats.json { }).generate "kubernetes-${name}-external-secret.json" {
      apiVersion = "external-secrets.io/v1";
      kind = "ExternalSecret";
      metadata = {
        inherit name namespace;
      };
      spec = {
        inherit secretStoreRef;
        target.name = name;
        data = lib.mapAttrsToList (refName: value: {
          secretKey = value.kubernetesKey or refName;
          remoteRef.key = value.key;
        }) values;
      };
    };
in
{
  inherit
    nomadEnvTemplates
    nomadRunCommand
    normalize
    rbwCheckCommand
    rbwMaterializeCommand
    renderKubernetesExternalSecret
    renderNomadJob
    ;

  providers.vaultwarden = {
    inherit rbwCheckCommand rbwMaterializeCommand;
  };

  consumers = {
    vm.refs = secretSet: secretSet.refs;
    nomad = {
      envTemplates = nomadEnvTemplates;
      runCommand = nomadRunCommand;
      renderJob = renderNomadJob;
    };
    kubernetes.renderExternalSecret = renderKubernetesExternalSecret;
  };
}
