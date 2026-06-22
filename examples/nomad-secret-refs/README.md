# Nomad Secret Refs

One pure secret declaration feeds two consumers: a fast `rbw` preflight and a
Nomad job that imports the same secret file as task environment.

## Shape

[`ix.nix`](ix.nix) declares a Vaultwarden provider and one secret ref:

```nix
secretSet = ix.secrets.normalize {
  provider = {
    type = "vaultwarden";
    client = "rbw";
    server = "https://vaultwarden.internal.example";
    mountRoot = "/run/ix-secrets";
    folder = "production";
  };

  values."daily-scraper/aws.env" = {
    key = "daily-scraper/aws-env";
    field = "notes";
    format = "env";
  };
};
```

`secretSet.refs."daily-scraper/aws.env"` is the VM or workload-facing path:
`/run/ix-secrets/daily-scraper/aws.env`. The provider key is
`daily-scraper/aws-env`, and `field = "notes"` means the rbw adapter writes the
item notes into the runtime file. Consumers live under `ix.secrets.consumers`:
VM modules read refs, Nomad renders template stanzas, and Kubernetes can render
an ExternalSecret-style manifest.

## Fail Fast

`checkSecrets` is a generated command from
`ix.secrets.providers.vaultwarden.rbwCheckCommand`. It runs `rbw get --raw` for
every declared key. It fails before a deploy reaches Nomad if Vaultwarden does
not contain the item, if `rbw` is not logged in, or if the operator points at
the wrong folder. When a ref sets `field`, the preflight checks that exact rbw
field before Nomad sees the job. `materializeSecrets` then writes the requested
field to the declared runtime path with mode `0600`.

`validateNomadJob` composes the provider and consumer pieces:

```sh
check-secret-refs
materialize-secret-refs
nomad job validate /nix/store/...-nomad-daily-scraper.hcl
```

The helper defaults to an ambient `nomad` binary because nixpkgs marks Nomad's
current license as unfree. A production flake can pass `nomadProgram =
lib.getExe pkgs.nomad` from an allowed package set. Switch that final verb to
`run` when the deployment pipeline should submit the job instead of only
validating it.

[`ix.nix`](ix.nix) also exposes `buildCheck`, a pure derivation backed
by Nix assertions over structured command metadata. It verifies that the provider
adapter calls the real `pkgs.rbw` binary, carries the `production` folder and
`notes` field, and renders the Nomad template path the materializer will create
at runtime.

## Nomad Consumer

The rendered Nomad job uses the same ref:

```hcl
template {
  source      = "/run/ix-secrets/daily-scraper/aws.env"
  destination = "secrets/aws.env"
  env         = true
}
```

That keeps secret bytes out of Nix. The `rbw` materializer writes the source
file just before validation or submission, then Nomad loads it into the task
environment.

## Kubernetes Consumer

The same provider keys can feed a Kubernetes manifest:

```json
{
  "apiVersion": "external-secrets.io/v1",
  "kind": "ExternalSecret",
  "metadata": {
    "name": "daily-scraper-aws",
    "namespace": "batch"
  }
}
```

That renderer is intentionally provider-shaped data, not a cluster client. A
real deploy can pipe the JSON through `kubectl apply` after its own provider
controller exists.

## Bad Fit If

This example assumes file-shaped runtime secrets. It is a poor fit for a
consumer that needs to query Vaultwarden directly from inside the task. In that
case the task gets provider credentials too, which is a wider trust boundary
than a short-lived materialized file.
