<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="a gateway load-balancing three api VMs that share a redis cache">
  </picture>
</p>

# Fleet microservices

A three-tier service — edge proxy, replicated stateless API, shared cache —
as four short Nix files. The pieces you would reach to Kubernetes for are all
here, declared next to the software they describe:

| You'd use in Kubernetes    | Here                                                    |
| -------------------------- | ------------------------------------------------------- |
| Deployment with 3 replicas | three `api-*` VMs from one module in [`default.ix`](default.ix) |
| Service discovery / DNS    | `ix.endpointOf nodes.cache "redis"` by VM name          |
| `httpGet` readiness probe  | `ix.healthChecks.ready.http = { port; path; }`          |
| `tcpSocket` probe          | `ix.healthChecks.cache-reachable.tcp = { host; port; }` |
| Ingress / LoadBalancer     | the `gateway` VM's nginx upstream pool                  |

Unlike a Kubernetes manifest (or a Nomad job), the VM definition *is* the
machine definition: the same file that names the api VMs also configures
nginx, and the gateway enumerates them at eval time — add an api VM and the
upstream pool and per-VM probes grow to match, before anything deploys.

## Run

```sh
ix apply .#cache .#api-0 .#api-1 .#api-2 .#gateway
```

In that order: each layer's health checks probe the one below it. Need the
source first? `git clone https://github.com/indexable-inc/index` and run it
from `examples/fleet/microservices`.

## Shape

- [`default.ix`](default.ix) — the VMs: `cache`, three `api-*` VMs built
  from one module, and `gateway`, wired together through `nodes`.
- [`cache.nix`](cache.nix) — redis, exposed to the group as `redis`, probed
  with a one-line `tcp.port` check.
- [`api.nix`](api.nix) — nginx standing in for a real service; each VM
  reports its own name. Declares an `http` readiness probe on `/healthz`
  and a cross-VM `tcp` probe that the cache is reachable.
- [`gateway.nix`](gateway.nix) — nginx upstream pool built by discovering
  every `api-*` VM at eval time, plus one generated `http` probe per api
  VM, so a failing probe names the exact VM the gateway cannot reach.

## Verify

```sh
# Round-robin through the api VMs via the gateway:
ix shell gateway -- curl --silent http://127.0.0.1:8080/  # {"service":"api","node":"api-1"}

# Journals from the cache VM:
ix shell cache -- journalctl -u redis-cache -n 50
```

## Scale

Add `api-3` to the list in [`default.ix`](default.ix). The new VM joins the
east-west group, inherits the same probes, and appears in the gateway's
upstream pool and per-VM checks — one line, and the whole topology follows.
