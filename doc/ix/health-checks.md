# health checks

A health check is a command that proves one of your image's services is actually ready, not just that systemd launched it. You declare checks in a NixOS module under `ix.healthChecks.<name>`, and the fleet runs them after a deploy: `ix-fleet --plan plan.json health`, and automatically as the post-deploy wait inside `up`, `replace`, and `switch`. A check either runs inside the VM (`from = "guest"`, the default) to prove the service is live locally, or on your own machine (`from = "host"`) to prove the node is reachable from outside. A check passes when its command exits 0; it is retried up to `attempts` times until it does, or the deploy fails.

## Declaring a check

Each entry under `ix.healthChecks` is a submodule (defined in `lib/image/platform.nix:20`):

```nix
ix.healthChecks.api = {
  command = [ "curl" "-fsS" "http://localhost:8080/health" ];
};
```

### Fields

| field | type | default | meaning |
| --- | --- | --- | --- |
| `description` | string | the attr name | label shown in fleet health output (`platform.nix:24`) |
| `unit` | string or null | `null` | sugar: probe a systemd unit (see below) (`platform.nix:30`) |
| `command` | non-empty list of strings | (none; required unless `unit` set) | the argv to run (`platform.nix:64`) |
| `from` | `"guest"` or `"host"` | `"guest"` | where the command runs (`platform.nix:47`) |
| `timeoutSec` | positive int | `30` | per-attempt timeout (`platform.nix:79`) |
| `attempts` | positive int | `30` | max attempts before the check fails (`platform.nix:85`) |
| `intervalSec` | unsigned int | `2` | seconds to wait between failed attempts (`platform.nix:91`) |
| `requiresIpv4` | bool | `false` | gate the check until the node has a public IPv4 (`platform.nix:97`) |

Set exactly one of `unit` or `command`. Setting both (where `command` is not the one `unit` derives) is rejected at evaluation time (`platform.nix:190`, `:384`).

`requiresIpv4` is only valid on `from = "host"` checks, and the node must be created with `deployment.ipv4 = true`; a guest check that sets it is rejected at eval time (`platform.nix:183`, `:377`). Use it for public-reachability probes that connect to the node's assigned IPv4.

## The `unit:` sugar

The overwhelmingly common check is "is this systemd unit running?". Instead of writing the full `systemctl` argv, set `unit`:

```nix
ix.healthChecks.nginx.unit = "nginx";
```

This desugars to `command = [ "systemctl" "is-active" "--quiet" "nginx.service" ]` (`platform.nix:12`, `:114`). A bare name gets the `.service` suffix; pass an explicit `foo.socket` or `foo.timer` to probe another unit type (`platform.nix:12`). The derived command is an `mkDefault`, so a real `command` you set wins; but setting both is flagged as a conflict rather than silently honoring one (`platform.nix:114`, `:190`).

## Guest vs host

`from = "guest"` (default) runs the argv inside the VM through the SDK exec channel (`__init__.py:600`). Use it for anything observable from inside the node: a unit is active, a port is listening, a database accepts connections.

```nix
# Guest: the nginx unit is active inside the VM.
ix.healthChecks.nginx.unit = "nginx";
```

`from = "host"` runs the command on your machine as a subprocess (`__init__.py:630`). Before running, the fleet injects the node's facts as environment variables and `$VAR`-substitutes them into your argv (`__init__.py:563`, `:579`, `:628`): `IX_NODE`, `IX_NODE_NAME`, `IX_NODE_IMAGE`, `IX_NODE_STATUS`, `IX_NODE_IPV6`, and, when the node has reported them, `IX_NODE_IPV4`, `IX_NODE_SUBDOMAIN`, `IX_NODE_REGION`. Use host checks for what only an outside observer can see: public reachability, DNS, the gateway path. The tool you call must be on your own `PATH`.

```nix
# Host: the node is reachable at its public subdomain over TLS.
ix.healthChecks.public = {
  from = "host";
  command = [ "curl" "-fsS" "https://$IX_NODE_SUBDOMAIN/health" ];
};
```

## How and when they run

Each check is attempted up to `attempts` times, `intervalSec` apart, with each attempt bounded by `timeoutSec` (`__init__.py:599`). The first exit-0 attempt passes; if none do, the check fails and reports the last command output (`__init__.py:653`). Run all checks for a plan with `ix-fleet --plan plan.json health`, or let them run automatically as the readiness wait at the end of `up`, `replace`, and `switch` (`__init__.py:869`, `:879`, `:889`). `--plan` is required (`__init__.py:1054`).

## See also

- [fleet.md](fleet.md): plans, nodes, and the deploy commands that run checks.
- [services.md](services.md): the service modules whose readiness you are proving.
- [networking.md](networking.md): `ix.networking.expose` and the ports a check probes.
- [environment.md](environment.md): the `IX_NODE_*` env vars host checks receive.
