# Daily Python Scraper

A daily Python job on ix.

It defines a uv project packaged with
[`ix.buildUvApplication`](../../lib/build-uv-application.nix), runs it as a
`systemd` oneshot service on a persistent daily timer, writes Parquet under
`/var/lib/daily-scraper/parquet`, and can sync the result to S3.

The Python stays ordinary Python. The ix-specific parts are
[`package.nix`](package.nix) and [`service.nix`](service.nix).

## Run

```sh
# From the index repo root.
nix run .#python-daily-scraper-up
```

## Shape

- [`pyproject.toml`](pyproject.toml), [`uv.lock`](uv.lock), and [`src/`](src/)
  are the Python project.
- [`default.nix`](default.nix) defines one ix fleet node.
- [`service.nix`](service.nix) owns the concrete service config, hardening,
  timer, and optional S3 sync.
- [`package.nix`](package.nix) packages the uv project as a store executable.

## S3 Output

The example leaves S3 sync disabled. [`service.nix`](service.nix) reads a
`dailyScraper` module argument, so a fleet can enable S3 without forking the
service module. The fleet declares the secret once, then the VM module consumes
the generated runtime file reference:

```nix
{
  secrets = {
    provider = {
      type = "vaultwarden";
      mountRoot = "/run/secrets";
      collection = "production";
    };
    "daily-scraper/aws.env".key = "daily-scraper/aws-env";
  };

  nodes.scraper.modules = [
    (
      { secretRefs, ... }:
      {
        _module.args.dailyScraper.s3 = {
          uri = "s3://andrew-scraper-output/github";
          deleteRemoved = true;
          awsEnvironmentFile = secretRefs."daily-scraper/aws.env";
        };
      }
    )
  ];
};
```

The AWS file is read at service start through `LoadCredential`, so the keys are
kept out of the Nix store. Its contents use systemd `EnvironmentFile` syntax:

```ini
AWS_ACCESS_KEY_ID=...
AWS_SECRET_ACCESS_KEY=...
AWS_REGION=us-east-1
```

The generated fleet plan carries the provider-facing key and the VM-facing path:

```json
{
  "secrets": {
    "provider": {
      "type": "vaultwarden",
      "mountRoot": "/run/secrets",
      "collection": "production"
    },
    "values": {
      "daily-scraper/aws.env": {
        "key": "daily-scraper/aws-env",
        "path": "/run/secrets/daily-scraper/aws.env"
      }
    }
  }
}
```

## Swap In Your Script

Keep [`service.nix`](service.nix) and [`package.nix`](package.nix), then replace
the Python module and dependencies. The service already handles timer catch-up,
durable VM state, journald logs, and an S3 sync step that runs only after the
scraper succeeds.
