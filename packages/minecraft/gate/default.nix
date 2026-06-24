# Gate Minecraft proxy. https://gate.minekube.com/
{
  lib,
  buildGoModule,
  fetchFromGitHub,
}:

let
  version = "0.66.13";
in
buildGoModule {
  pname = "gate";
  inherit version;

  src = fetchFromGitHub {
    owner = "minekube";
    repo = "gate";
    rev = "v${version}";
    hash = "sha256-j+RdX2IfP1ILw6/HodRHM2dh5pQfln11EWKZNDrVOqY=";
  };

  vendorHash = "sha256-X3B+S2QG2WCsSderL2XwVQjLJDDP+bh6DQqe4kwjEcQ=";

  # Upstream's Dockerfile builds the root `gate.go` rather than `cmd/gate`,
  # and stamps the version into `pkg/version.Version`.
  subPackages = [ "." ];

  ldflags = [
    "-s"
    "-w"
    "-X go.minekube.com/gate/pkg/version.Version=${version}"
  ];

  # The `go.minekube.com/geyserlite` transitive dep embeds prebuilt
  # per-arch binaries that are not in the module tarball, so `go mod vendor`
  # fails resolving its `assets/geyserlite-*` embed pattern. Use the
  # module proxy path (no `vendor/` materialization) to fetch dependencies
  # the way upstream's `go build` does.
  proxyVendor = true;

  doCheck = false;
  strictDeps = true;

  meta = {
    description = "Minecraft Java + Bedrock proxy by Minekube, Go-native peer to BungeeCord and Velocity";
    homepage = "https://gate.minekube.com/";
    license = lib.licenses.asl20;
    mainProgram = "gate";
    platforms = lib.platforms.linux ++ lib.platforms.darwin;
    sourceProvenance = [ lib.sourceTypes.fromSource ];
  };
}
