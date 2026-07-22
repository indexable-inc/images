# One statement of every port in the cluster; node.nix opens the ones every
# node listens on, server.nix and workload.nix the role-specific ones.
{
  api = 6443;
  flannelVxlan = 8472;
  kubelet = 10250;
  app = 8080;
  nodePort = 30080;
}
