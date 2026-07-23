<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="three workers resolving one web VM by name inside the fleet-hello east-west group">
  </picture>
</p>

# Fleet hello

What does a Kubernetes Service plus a three-replica Deployment look like as
one Nix file? This is the smallest multi-VM example: one `web` VM serving a
static page and three interchangeable `worker` VMs that resolve it by name
with `ix.endpointOf nodes.web "http"` and probe it as their health check — an
`httpGet`-style probe declared as `http = { host; port; }`, no hand-written
curl.

## Run

```sh
ix apply .#web .#worker-0 .#worker-1 .#worker-2
```

Apply `web` first so the workers' health checks have something to reach.
Need the source first? `git clone https://github.com/indexable-inc/index`
and run it from `examples/fleet/hello`.

## Shape

- [`default.ix`](default.ix) defines the VMs: one `web` and three `worker`s
  built from the same module, all in one east-west group. The workers take
  `nodes = web.nixosConfigurations`, which is how the peer reference below
  resolves.
- [`web.nix`](web.nix) runs nginx and declares `ix.networking.expose.http`,
  which opens the firewall, registers the port claim, and names the endpoint
  workers resolve. Its readiness is a one-line `http.port` probe.
- [`worker.nix`](worker.nix) resolves that endpoint and probes it with
  `http = { host = web.host; port = web.port; }` — the platform derives the
  curl command and keeps the probe binary in the image.

## Verify

```sh
ix shell worker-0 -- curl --fail http://web:8080/
```

Workers are named `worker-0` through `worker-2`; each reaches `web` by its
VM name over the east-west network.

## Scale

One more worker is one more name in [`default.ix`](default.ix)'s worker
list (and one more `ix apply` target). It joins the group and picks up the
same health check; nothing else changes.
