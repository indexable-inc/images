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
    # First-build placeholder. Run `nix build .#gate` and replace with the
    # `got: sha256-...` value from the hash-mismatch error.
    hash = lib.fakeHash;
  };

  # Same placeholder pattern — buildGoModule prints the actual vendor hash
  # on first build under "got: sha256-...".
  vendorHash = lib.fakeHash;

  subPackages = [ "cmd/gate" ];

  # Strip debug info and stamp the upstream version into the binary so
  # `gate --version` reports the same string as the source tag.
  ldflags = [
    "-s"
    "-w"
    "-X go.minekube.com/gate/pkg/internal/buildinfo.Version=${version}"
  ];

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
