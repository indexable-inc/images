<p align="center">
  <picture>
    <source media="(prefers-color-scheme: dark)" srcset="assets/hero-dark.svg">
    <img src="assets/hero.svg" width="720" alt="a k3s server and two agents forming a kubernetes cluster whose manifests and images all come from the nix store">
  </picture>
</p>

# Kubernetes on Nix

A real Kubernetes cluster - one k3s server, two agents - where every layer is
the same language. The machines are NixOS modules, the Deployment and Service
are Nix values that nixpkgs' k3s module renders to YAML, and the pod image is
a `dockerTools` build preloaded into containerd. Nothing pulls from a
registry; the entire cluster, workload included, ships from the nix store.

| Usually                            | Here                                                       |
| ---------------------------------- | ---------------------------------------------------------- |
| kubeadm / cloud control plane      | `services.k3s.enable = true` in [`node.ix`](node.ix)     |
| `kubectl apply -f app.yaml`        | `services.k3s.manifests` in [`workload.ix`](workload.ix) |
| `docker push` + registry pull      | `services.k3s.images` preloads [`image.ix`](image.ix)    |
| airgap image bundles               | `k3s.airgap-images` from the store                         |
| joining nodes with a token by hand | `serverAddr` from the server peer's node attrs             |

The wiring is the cluster definition: [`default.ix`](default.ix) lists the
VMs, [`agent.ix`](agent.ix) derives the join address from the server VM's
east-west hostname at eval time, and the Deployment references
the pod image by the exact `imageName:imageTag` of the derivation every node
imports - a drifted tag fails the repo's eval tests before anything boots.

## Run

```sh
# From this directory (examples/k8s/k3s in the index repo).
ix apply .#k3s-server .#k3s-agent-0 .#k3s-agent-1
```

Need the repo first? `git clone https://github.com/indexable-inc/index`.

All three VMs boot in parallel; agents retry the join until the API
answers, and the server's checks report Ready only once every named node
joined, the `whoami` Deployment rolled out, and its NodePort answers.

## Shape

- [`default.ix`](default.ix) - the wiring: `k3s-server` and two agents, one
  east-west group, each wired as the other's peers through `mkVm`'s `nodes`.
- [`node.ix`](node.ix) - what every cluster node shares: the k3s service,
  the join token, the runtime node-ip handoff, image preload, inter-node
  ports (flannel VXLAN, kubelet), a liveness check.
- [`server.ix`](server.ix) - the control plane: API port, extras disabled,
  the cluster-wide readiness probes (`kubectl wait` over the wired VM
  list, rollout status, NodePort http).
- [`agent.ix`](agent.ix) - join the server by its east-west hostname.
- [`workload.ix`](workload.ix) - the Deployment and NodePort Service as
  Nix values; the k3s module renders the YAML.
- [`image.ix`](image.ix) / [`whoami_serve.py`](whoami_serve.py) - the pod
  image, built by `dockerTools`: a tiny http server reporting its pod and
  node via the downward API.
- [`ports.ix`](ports.ix) - every port, stated once.

## Verify

```sh
# Real kubectl against a real cluster:
ix shell k3s-server -- k3s kubectl get nodes -o wide
ix shell k3s-server -- k3s kubectl get pods -o wide

# The Service answers on every node's port 30080 and names the pod behind it:
ix shell k3s-agent-0 -- curl --silent http://127.0.0.1:30080/  # {"pod":"whoami-...","node":"k3s-agent-1"}

ix shell k3s-server -- journalctl -u k3s -n 50
```

## Scale

Another agent is one line in [`default.ix`](default.ix): add
`agent("k3s-agent-2")` to the server's peers. New agents join with the same
token, preload the same images, and appear by name in the server's
`nodes-ready` check automatically. Raise `replicas` in
[`workload.ix`](workload.ix) to spread more pods across them.

## Why k3s and not upstream Kubernetes

k3s is CNCF-conformant Kubernetes, not a subset: same API, same kubectl,
same manifests. What it changes is shape. Upstream Kubernetes is six
coordinating daemons plus a PKI between them, wired imperatively by
kubeadm; k3s is one binary reading one config file, which maps 1:1 onto a
NixOS module. nixpkgs agrees: `services.k3s` is actively maintained and
carries the typed options every row of the table above leans on
(`manifests`, `images`, `charts`), while the legacy `services.kubernetes`
module has been slated for extraction since 2021 ([nixpkgs#115179]). HA
needs no external etcd either: `clusterInit = true` on the first server
switches the embedded datastore from sqlite to etcd, and further servers
join it.

Shipping the workload entirely from the store is a demo virtue, not the
operating model at scale. Past a handful of apps, preloading every image
on every node bloats each node's closure, and deploying through
`services.k3s.manifests` couples an app deploy to a rebuild of the server
VM. The split that scales: nix owns the machines and the cluster substrate
(k3s itself, join wiring, ports, system add-ons, image builds), the
Kubernetes API owns workload churn.

[nixpkgs#115179]: https://github.com/NixOS/nixpkgs/issues/115179
