{
  stdenvNoCC,
  src,
}:

stdenvNoCC.mkDerivation {
  pname = "tonbo-artifacts";
  version = "e16636b0e5ce";

  inherit src;

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
