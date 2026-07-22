/**
The demo workload, declared the same way as the machines that run it.

`services.k3s.manifests` is nixpkgs' typed path into the k3s auto-deploy
directory: the Deployment and Service below are plain Nix values and the
module renders the YAML. The pod image (image.nix) is preloaded into
containerd on every node (node.nix), so the manifest never touches a
registry.
*/
{
  ix,
  lib,
  pkgs,
  ...
}: let
  ports = import ./ports.nix;
  replicas = 2;
  appLabels."app.kubernetes.io/name" = "whoami";
  whoamiImage = import ./image.nix {inherit ix lib pkgs;};
in {
  services.k3s = {
    manifests.whoami.content = [
      {
        apiVersion = "apps/v1";
        kind = "Deployment";
        metadata.name = "whoami";
        spec = {
          inherit replicas;
          selector.matchLabels = appLabels;
          template = {
            metadata.labels = appLabels;
            spec.containers = [
              {
                name = "whoami";
                # Never pulled: every node already imported the image into
                # containerd from the store (node.nix).
                image = "${whoamiImage.imageName}:${whoamiImage.imageTag}";
                imagePullPolicy = "Never";
                env = [
                  {
                    name = "POD_NAME";
                    valueFrom.fieldRef.fieldPath = "metadata.name";
                  }
                  {
                    name = "NODE_NAME";
                    valueFrom.fieldRef.fieldPath = "spec.nodeName";
                  }
                ];
                ports = [{containerPort = ports.app;}];
                readinessProbe.httpGet = {
                  path = "/";
                  port = ports.app;
                };
              }
            ];
          };
        };
      }
      {
        apiVersion = "v1";
        kind = "Service";
        metadata.name = "whoami";
        spec = {
          type = "NodePort";
          selector = appLabels;
          ports = [
            {
              port = ports.app;
              targetPort = ports.app;
              inherit (ports) nodePort;
            }
          ];
        };
      }
    ];
  };
}
