{
  esbuild,
  lib,
  makeWrapper,
  nodejs,
  stdenvNoCC,
}:

stdenvNoCC.mkDerivation {
  pname = "htmlpage";
  version = "0.1.0";
  src = ./src;

  installPhase = ''
    # shell
    runHook preInstall
    mkdir -p $out/lib/htmlpage $out/bin
    cp -R . $out/lib/htmlpage/
    makeWrapper ${lib.getExe nodejs} $out/bin/htmlpage \
      --add-flags $out/lib/htmlpage/cli.mjs \
      --prefix PATH : ${lib.makeBinPath [ esbuild ]}
    runHook postInstall
  '';

  nativeBuildInputs = [ makeWrapper ];

  meta = {
    description = "Render a single TSX file to a self-contained HTML page";
    license = lib.licenses.mit;
    mainProgram = "htmlpage";
  };
}
