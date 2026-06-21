{
  jdk25,
  lib,
  stdenv,
}:

let
  fs = lib.fileset;
in
stdenv.mkDerivation {
  pname = "minecraft-hot-reload-agent";
  version = "0.1.0";
  src = fs.toSource {
    root = ./.;
    fileset = fs.intersection (fs.gitTracked ./.) (
      fs.unions [
        ./MANIFEST.MF
        ./src
      ]
    );
  };

  strictDeps = true;
  nativeBuildInputs = [ jdk25 ];

  buildPhase = ''
    # shell
    runHook preBuild

    mkdir -p classes
    javac --release 21 -d classes $(find src -name '*.java' -print)
    jar cfm minecraft-hot-reload-agent.jar MANIFEST.MF -C classes .

    runHook postBuild
  '';

  installPhase = ''
    # shell
    runHook preInstall

    install -Dm0644 minecraft-hot-reload-agent.jar \
      "$out/share/minecraft-hot-reload-agent/minecraft-hot-reload-agent.jar"

    runHook postInstall
  '';

  meta = {
    description = "Minecraft development hot-reload Java agent";
    license = lib.licenses.mit;
  };
}
