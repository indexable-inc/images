# health checks

A health check proves one of your image's services is ready, not just that systemd launched it. Declare checks under `ix.healthChecks.<name>`; `ix up` enforces them after activation. A check either runs inside the VM (`from = "guest"`, the default) or on the caller (`from = "host"`). It passes when its command exits 0 within the configured attempts.

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
| `description` | string | the attr name | label shown in `ix up` health output (`platform.nix:24`) |
| `unit` | string or null | `null` | sugar: probe a systemd unit (see below) (`platform.nix:30`) |
| `http` | `{ port; path ? "/"; host ? "127.0.0.1"; }` or null | `null` | sugar: HTTP GET probe, unhealthy on any >= 400 status (see below) |
| `tcp` | `{ port; host ? "127.0.0.1"; }` or null | `null` | sugar: TCP connect probe (see below) |
| `command` | non-empty list of strings | (none; required unless a sugar is set) | the argv to run (`platform.nix:64`) |
| `from` | `"guest"` or `"host"` | `"guest"` | where the command runs (`platform.nix:47`) |
| `timeoutSec` | positive int | `30` | per-attempt timeout (`platform.nix:79`) |
| `attempts` | positive int | `30` | max attempts before the check fails (`platform.nix:85`) |
| `intervalSec` | unsigned int | `2` | seconds to wait between failed attempts (`platform.nix:91`) |
| `requiresIpv4` | bool | `false` | gate the check until the node has a public IPv4 (`platform.nix:97`) |

Set exactly one of `unit`, `http`, `tcp`, or `command`. Setting more than one sugar, or a sugar plus an explicit `command` (where `command` is not the one the sugar derives), is rejected at evaluation time (`platform.nix:190`, `:384`).

`requiresIpv4` is only valid on `from = "host"` checks, and the node must be created with `deployment.ipv4 = true`; a guest check that sets it is rejected at eval time (`platform.nix:183`, `:377`). Use it for public-reachability probes that connect to the node's assigned IPv4.

## The `unit:` sugar

The overwhelmingly common check is "is this systemd unit running?". Instead of writing the full `systemctl` argv, set `unit`:

```nix
ix.healthChecks.nginx.unit = "nginx";
```

This desugars to `command = [ "systemctl" "is-active" "--quiet" "nginx.service" ]` (`platform.nix:12`, `:114`). A bare name gets the `.service` suffix; pass an explicit `foo.socket` or `foo.timer` to probe another unit type (`platform.nix:12`). The derived command is an `mkDefault`, so a real `command` you set wins; but setting both is flagged as a conflict rather than silently honoring one (`platform.nix:114`, `:190`).

## The `http:` and `tcp:` sugars

The Kubernetes `httpGet` and `tcpSocket` probes, as one-attr checks. `http` proves an HTTP endpoint answers with a non-error status; `tcp` proves something is accepting connections on a port:

```nix
# GET http://127.0.0.1:8080/healthz must return < 400.
ix.healthChecks.ready.http = { port = 8080; path = "/healthz"; };

# A peer node's postgres accepts TCP connections (cross-node probe).
ix.healthChecks.db-reachable.tcp = { host = "db"; port = 5432; };
```

`http = { port; path ? "/"; host ? "127.0.0.1"; }` desugars to `curl --fail --silent --show-error http://<host>:<port><path>` — `--fail` makes any >= 400 response unhealthy. `tcp = { port; host ? "127.0.0.1"; }` desugars to `nc -z <host> <port>`. Both pin their probe binary (curl / netcat) into the image's `environment.systemPackages` with store paths in the argv, so the probe works on a minimal image with nothing extra on `PATH`.

Point `host` at a peer node's east-west hostname (pair well with `ix.endpointOf`, see [networking.md](networking.md)) to probe that a dependency is reachable *from this node*. These sugars are guest-only: they run inside the VM with store-pinned binaries, so `from = "host"` combined with `http`/`tcp` is rejected at eval time — write an explicit `command` with tools from your own `PATH` for host-side probes.

## Guest vs host

`from = "guest"` (default) runs the argv inside the VM. Use it for anything observable from inside the node: a unit is active, a port is listening, a database accepts connections.

```nix
# Guest: the nginx unit is active inside the VM.
ix.healthChecks.nginx.unit = "nginx";
```

`from = "host"` runs the command on your machine. `ix up` injects the node facts documented in [environment.md](environment.md). Use host checks for public reachability, DNS, and gateway paths. The tool you call must be on your own `PATH`.

```nix
# Host: the node is reachable at its public subdomain over TLS.
ix.healthChecks.public = {
  from = "host";
  command = [ "curl" "-fsS" "https://$IX_NODE_SUBDOMAIN/health" ];
};
```

## How and when they run

Each check is attempted up to `attempts` times, `intervalSec` apart, with each attempt bounded by `timeoutSec`. The first successful attempt passes. If none do, `ix up` fails and reports the check output.

## See also

- [fleet.md](fleet.md): plans, nodes, and the deploy commands that run checks.
- [services.md](services.md): the service modules whose readiness you are proving.
- [networking.md](networking.md): `ix.networking.expose` and the ports a check probes.
- [environment.md](environment.md): the `IX_NODE_*` env vars host checks receive.
