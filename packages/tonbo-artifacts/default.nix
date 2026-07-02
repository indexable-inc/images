{
  ix,
  stdenvNoCC,
  fetchurl,
}:

let
  # Version + URL and SRI hash live in the sibling pins.json, never inline
  # (repo policy). Bump with `nix run .#update`.
  pin = ix.pins.loadPin ./pins.json "artifacts";
  inherit (pin) version;
in
stdenvNoCC.mkDerivation {
  pname = "tonbo-artifacts";
  inherit version;

  src = fetchurl { inherit (pin) url hash; };

  dontUnpack = true;
  dontBuild = true;
  strictDeps = true;

  installPhase = ''
    # shell
    runHook preInstall

    install -Dm755 "$src" "$out/bin/artifacts"

    runHook postInstall
  '';

  meta = {
    description = "Tonbo Artifacts CLI";
    homepage = "https://artifacts.tonbo.io/docs/overview/";
    mainProgram = "artifacts";
    platforms = [ "x86_64-linux" ];
  };
}
