{
  stdenvNoCC,
  fetchurl,
}:

let
  version = "e16636b0e5ce";
in
stdenvNoCC.mkDerivation {
  pname = "tonbo-artifacts";
  inherit version;

  src = fetchurl {
    url = "https://artifacts.tonbo.dev/release/${version}/artifacts";
    hash = "sha256-sYSENVI+l1DOfRtpnROkPY0/hJQoOjP1EsagrXSwIWY=";
  };

  dontUnpack = true;
  dontBuild = true;
  strictDeps = true;

  installPhase = ''
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
