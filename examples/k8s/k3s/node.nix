/**
One k3s cluster node, either role.

Everything a server and an agent share: the k3s service itself, the join
token, the runtime node-ip handoff, the inter-node ports, and the liveness
check. Role-specific config (API server, workload, join address) lives in
server.nix and agent.nix.
*/
{
  config,
  ix,
  lib,
  pkgs,
  ...
}: let
  ports = import ./ports.nix;
in {
  services.k3s = {
    enable = true;
    # One shared join secret baked into both roles. `token` lands in the
    # world-readable nix store, which is fine for demo VMs on a private
    # east-west group; use `tokenFile` fed by `deployment.secrets` for
    # anything real.
    token = "k8s-k3s-example";
    # Everything containerd needs, preloaded from the store on every node
    # (the scheduler may place pods anywhere): k3s's bundled images (pause,
    # coredns, ...) plus the workload's. Nothing is pulled from a registry;
    # the guests have no internet egress anyway.
    images = [
      config.services.k3s.package.airgap-images
      (import ./image.nix {inherit ix lib pkgs;})
    ];
  };

  # k3s autodetects its node IP from the default route, but these guests are
  # east-west only with no default route, so derive the address from the
  # routing table and hand it over via k3s's own config file, read on startup
  # and merged under the eval-time flags. The value only exists at runtime,
  # so no eval-time renderer can produce this file; it is one scalar key.
  systemd.services.k3s = {
    path = [
      pkgs.iproute2
      pkgs.gnused
      pkgs.coreutils
    ];
    preStart = ''
      node_ip=$(ip -4 -o addr show scope global | sed -n 's|.*inet \([0-9.]*\)/.*|\1|p' | head -n1)
      if [ -z "$node_ip" ]; then
        echo "k3s: no global IPv4 address found" >&2
        exit 1
      fi
      mkdir -p /etc/rancher/k3s
      printf 'node-ip: %s\n' "$node_ip" > /etc/rancher/k3s/config.yaml
    '';
  };

  ix.networking.expose = {
    k3s-flannel-vxlan = {
      port = ports.flannelVxlan;
      protocol = "udp";
      description = "flannel VXLAN overlay (pod traffic between nodes)";
    };
    k3s-kubelet = {
      port = ports.kubelet;
      description = "kubelet API (kubectl logs/exec reach the node here)";
    };
    # kube-proxy programs the whoami Service's NodePort (workload.nix) on
    # every node, server and agents alike; open it on every VM so a curl
    # lands regardless of which node it hits.
    whoami-nodeport = {
      port = ports.nodePort;
      description = "whoami Service (NodePort, answers on every node)";
    };
  };

  ix.healthChecks.k3s-active = {
    description = "k3s service is active";
    unit = "k3s";
  };
}
