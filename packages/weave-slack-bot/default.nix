{
  lib,
  makeWrapper,
  python3,
  stdenvNoCC,
}: let
  fs = lib.fileset;
  src = fs.toSource {
    root = ./.;
    fileset = fs.unions [
      ./weave_slack_bot.py
      ./tests
    ];
  };
  python = python3.withPackages (ps: [
    ps.aiohttp
    ps.slack-sdk
  ]);
in
  stdenvNoCC.mkDerivation {
    pname = "weave-slack-bot";
    version = "0.1.0";
    inherit src;

    nativeBuildInputs = [makeWrapper];
    nativeCheckInputs = [python];
    strictDeps = true;
    doCheck = true;

    checkPhase = ''
      runHook preCheck
      ${python}/bin/python3 -m unittest discover -s tests -p 'test_*.py'
      runHook postCheck
    '';

    installPhase = ''
      runHook preInstall
      install -Dm755 weave_slack_bot.py $out/libexec/weave-slack-bot.py
      makeWrapper ${python}/bin/python3 $out/bin/weave-slack-bot \
        --add-flags "$out/libexec/weave-slack-bot.py"
      runHook postInstall
    '';

    meta = {
      description = "Durable Slack Socket Mode bridge for a Weave agent";
      license = lib.licenses.mit;
      mainProgram = "weave-slack-bot";
      platforms = lib.platforms.linux;
    };
  }
