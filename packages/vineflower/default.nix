{
  fetchurl,
  ix,
  jdk,
  lib,
  stdenvNoCC,
}:

let
  # Version + URL and SRI hash live in the sibling pins.json, never inline
  # (repo policy). Bump with `nix run .#update`.
  pin = ix.pins.loadPin ./pins.json "vineflower";
  inherit (pin) version;
  jar = fetchurl { inherit (pin) url hash; };
in
stdenvNoCC.mkDerivation {
  pname = "vineflower";
  inherit version;

  src = jar;

  dontUnpack = true;
  strictDeps = true;
  nativeBuildInputs = [ jdk ];

  installPhase = ''
    # shell
    runHook preInstall
    mkdir -p $out/share/java $out/bin
    cp ${jar} $out/share/java/vineflower.jar
    cat > $out/bin/vineflower <<EOF
    #!${stdenvNoCC.shell}
    exec ${jdk}/bin/java -jar $out/share/java/vineflower.jar "\$@"
    EOF
    chmod +x $out/bin/vineflower
    runHook postInstall
  '';

  meta = {
    description = "Modern fork of Fernflower, the actively maintained Java decompiler";
    homepage = "https://vineflower.org/";
    license = lib.licenses.asl20;
    mainProgram = "vineflower";
    platforms = lib.platforms.unix;
  };
}
