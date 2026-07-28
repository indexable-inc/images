# `ssh-hosts`: list configured SSH host aliases (from ~/.ssh/config and its
# Includes) plus recently used ssh targets. Built straight from the single-file
# Rust source with the Nix-provided rustc, so it does not depend on the local
# rustup toolchain. `stdenv` (not the no-cc runCommand) supplies the `cc` linker
# and macOS SDK that rustc needs; the wrapper puts `sqlite3` on PATH for the
# history query.
{
  lib,
  stdenv,
  rustc,
  makeWrapper,
  sqlite,
}:
stdenv.mkDerivation {
  pname = "ssh-hosts";
  version = "0.1.0";
  src = lib.fileset.toSource {
    root = ./.;
    fileset = ./ssh-hosts.rs;
  };

  nativeBuildInputs = [
    rustc
    makeWrapper
  ];
  strictDeps = true;
  dontConfigure = true;

  buildPhase = ''
    # shell
    runHook preBuild
    rustc -O --edition 2024 ssh-hosts.rs -o ssh-hosts
    runHook postBuild
  '';

  doCheck = true;
  checkPhase = ''
    # shell
    runHook preCheck
    rustc --test --edition 2024 ssh-hosts.rs -o ssh-hosts-tests
    ./ssh-hosts-tests
    runHook postCheck
  '';

  installPhase = ''
    # shell
    runHook preInstall
    mkdir -p "$out/bin"
    install -m755 ssh-hosts "$out/bin/.ssh-hosts-unwrapped"
    makeWrapper "$out/bin/.ssh-hosts-unwrapped" "$out/bin/ssh-hosts" \
      --prefix PATH : ${lib.makeBinPath [sqlite]}
    runHook postInstall
  '';

  meta = {
    description = "List configured SSH host aliases and recently used ssh targets";
    mainProgram = "ssh-hosts";
  };
}
