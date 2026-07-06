# Fleet hello

The smallest multi-node fleet: one `web` node and three `worker` replicas,
the ix analog of a Kubernetes Service plus a 3-replica Deployment.

`web` serves a static page over HTTP and exposes it to the fleet's east-west
group. Each worker resolves the listener by name with
`ix.endpointOf nodes.web "http"` and proves reachability through its health
check, so the generated up wrapper only reports healthy once every worker can
reach the web node.

## Run

```sh
# From the index repo root.
nix run .#fleet-hello-up
```

## Shape

- [`ix.nix`](ix.nix) defines the fleet: one `web` node and a `worker` node
  with `replicas = 3`, all in one east-west group, with `dependsOn` so the
  web node boots first.
- [`web.nix`](web.nix) runs nginx and declares `ix.networking.expose.http`,
  which opens the firewall, registers the port claim, and names the endpoint
  workers resolve.
- [`worker.nix`](worker.nix) resolves that endpoint and curls it as its
  health check.

## Verify

```sh
ix shell worker-0 -- curl --fail http://web:8080/
```

Replicas are numbered `worker-0` through `worker-2`; each reaches `web` by
its node name over the east-west network.

## Scale

Worker count is one line: raise `worker.replicas` in [`ix.nix`](ix.nix).
Nothing else changes; new replicas join the group and pick up the same
health check.
