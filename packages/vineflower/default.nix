{
  fetchurl,
  jdk,
  lib,
  stdenvNoCC,
}:

let
  version = "1.12.0";
  jar = fetchurl {
    url = "https://github.com/Vineflower/vineflower/releases/download/${version}/vineflower-${version}.jar";
    hash = "sha256-Hfz+l0OVc0+kZ85iBmHHYj0FuoNnDeBSmx+9Y/9Ui50=";
  };
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
